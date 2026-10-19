//! `--probe-pause <stream_id>` (`devtools` only, stream recovery ticket
//! 01): what mpv does when a live channel is paused for 1, 5 and 15 min and
//! resumed, once as the app runs it and once with ffmpeg's reconnect
//! options. Each try is a fresh headless mpv, so about 50 min in all, on
//! the account's one connection. Prints events, offsets and seconds only.

use crate::config::{self, Provider};
use crate::store;
use crate::xtream::Client;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{mpsc, oneshot};

const RECONNECT: &str = "--stream-lavf-o=reconnect=1,reconnect_streamed=1,reconnect_delay_max=10";
const PAUSES: [u64; 3] = [1, 5, 15];
/// Played before pausing.
const BEFORE: Duration = Duration::from_secs(20);
/// Seconds after resuming that `time-pos` is read; the last ends the try.
const AFTER: [u64; 3] = [10, 30, 60];
/// Between tries, so the provider has let the connection go.
const BETWEEN: Duration = Duration::from_secs(15);

pub async fn run(provider: Provider, mpv: &str, stream_id: i64) -> Result<(), String> {
    let url = live_url(provider, stream_id)?;
    let total: u64 = PAUSES.iter().sum::<u64>() * 2 + 12;
    println!("about {total} min; each try plays 20 s, pauses, resumes and watches 60 s");
    for (mode, extra) in [("as the app", None), ("with reconnect", Some(RECONNECT))] {
        for mins in PAUSES {
            let result = try_pause(mpv, url.as_str(), extra, Duration::from_secs(mins * 60)).await;
            println!(
                "{mode}, paused {mins} min: {}",
                result.unwrap_or_else(|e| format!("failed: {e}"))
            );
            tokio::time::sleep(BETWEEN).await;
        }
    }
    Ok(())
}

/// A live channel's stream URL, from the database.
pub(super) fn live_url(provider: Provider, stream_id: i64) -> Result<reqwest::Url, String> {
    let db = store::open_reader(&config::database_path()).map_err(|e| e.to_string())?;
    let ext: String = db
        .query_row(
            "SELECT extension FROM channel WHERE provider_id = ?1 AND stream_id = ?2",
            rusqlite::params![provider.id, stream_id],
            |r| r.get(0),
        )
        .map_err(|_| "no such channel in the database".to_string())?;
    drop(db);
    Client::new(provider).stream_url(stream_id, &ext)
}

/// An mpv on the probe socket.
pub(super) struct Mpv {
    child: tokio::process::Child,
    wr: OwnedWriteHalf,
    replies: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    pub(super) events: mpsc::UnboundedReceiver<(Instant, Value)>,
    id: i64,
}

impl Mpv {
    /// A headless mpv without the user's config, plus `extra`.
    async fn spawn_headless(mpv: &str, extra: Option<&str>) -> Result<Self, String> {
        let mut args = vec!["--no-config", "--idle=yes", "--vo=null", "--ao=null"];
        args.extend(extra);
        let mut mpv = Self::spawn(mpv, &args).await?;
        mpv.cmd(json!(["observe_property", 1, "paused-for-cache"]))
            .await?;
        Ok(mpv)
    }

    pub(super) async fn spawn(mpv: &str, args: &[&str]) -> Result<Self, String> {
        let sock = config::mpv_socket_path().with_file_name("probe.sock");
        if let Some(dir) = sock.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let _ = std::fs::remove_file(&sock);
        let child = tokio::process::Command::new(mpv)
            .args(args)
            .arg(format!("--input-ipc-server={}", sock.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("spawn: {e}"))?;
        let mut conn = None;
        for _ in 0..50 {
            if let Ok(c) = UnixStream::connect(&sock).await {
                conn = Some(c);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let (rd, wr) = conn.ok_or("no IPC socket")?.into_split();
        let replies: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>> = Arc::default();
        let (tx, events) = mpsc::unbounded_channel();
        let waiting = Arc::clone(&replies);
        tokio::spawn(async move {
            let mut lines = BufReader::new(rd).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let v: Value = serde_json::from_str(&l).unwrap_or_default();
                let reply = v["request_id"]
                    .as_i64()
                    .and_then(|id| waiting.lock().unwrap().remove(&id));
                match reply {
                    Some(r) => drop(r.send(v)),
                    None if v["event"].is_string() => drop(tx.send((Instant::now(), v))),
                    None => {}
                }
            }
            let _ = tx.send((Instant::now(), json!({ "event": "mpv-exited" })));
        });
        Ok(Self {
            child,
            wr,
            replies,
            events,
            id: 0,
        })
    }

    pub(super) fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    pub(super) async fn cmd(&mut self, c: Value) -> Result<Value, String> {
        self.id += 1;
        let (tx, rx) = oneshot::channel();
        self.replies.lock().unwrap().insert(self.id, tx);
        let line = json!({ "command": c, "request_id": self.id }).to_string() + "\n";
        self.wr
            .write_all(line.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let v = tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .map_err(|_| "mpv did not answer")?
            .map_err(|_| "mpv exited")?;
        if v["error"] == "success" {
            Ok(v["data"].clone())
        } else {
            Err(v["error"].as_str().unwrap_or("?").to_owned())
        }
    }

    pub(super) async fn number(&mut self, prop: &str) -> Option<f64> {
        self.cmd(json!(["get_property", prop]))
            .await
            .ok()
            .and_then(|v| v.as_f64())
    }

    /// The events since the last call, as `<event> at <s>` from `since`.
    fn drain(&mut self, since: Instant) -> String {
        let mut out = Vec::new();
        while let Ok((at, v)) = self.events.try_recv() {
            let name = v["event"].as_str().unwrap_or("?");
            let what = match name {
                "end-file" => format!(
                    "end-file {} ({})",
                    v["reason"].as_str().unwrap_or("?"),
                    v["file_error"].as_str().unwrap_or("no error")
                ),
                "property-change" => match v["data"].as_bool() {
                    Some(on) => format!("paused-for-cache {on}"),
                    None => continue,
                },
                "playback-restart" | "idle" | "mpv-exited" | "seek" => name.to_string(),
                _ => continue,
            };
            let secs = at.saturating_duration_since(since).as_secs_f64();
            out.push(format!("{what} at {secs:.0} s"));
        }
        if out.is_empty() {
            "nothing".into()
        } else {
            out.join(", ")
        }
    }
}

async fn try_pause(
    mpv: &str,
    url: &str,
    extra: Option<&str>,
    pause: Duration,
) -> Result<String, String> {
    let mut mpv = Mpv::spawn_headless(mpv, extra).await?;
    let loaded = Instant::now();
    mpv.cmd(json!(["loadfile", url, "replace"])).await?;
    let mut started = None;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if mpv.number("time-pos").await.is_some() {
            started = Some(loaded.elapsed());
            break;
        }
    }
    let Some(started) = started else {
        return Ok(format!("never started in 30 s: {}", mpv.drain(loaded)));
    };
    tokio::time::sleep(BEFORE).await;
    let playing = mpv.drain(loaded);
    let at_pause = mpv.number("time-pos").await;
    mpv.cmd(json!(["set_property", "pause", true])).await?;
    let paused = Instant::now();
    tokio::time::sleep(pause).await;
    let during = mpv.drain(paused);
    let cache = mpv.number("demuxer-cache-duration").await;
    let resume = mpv.cmd(json!(["set_property", "pause", false])).await;
    let resumed = Instant::now();
    let mut pos = Vec::new();
    for at in AFTER {
        let wait = Duration::from_secs(at).saturating_sub(resumed.elapsed());
        tokio::time::sleep(wait).await;
        pos.push((at, mpv.number("time-pos").await));
    }
    let after = mpv.drain(resumed);
    let _ = mpv.cmd(json!(["quit"])).await;
    let s = |t: Option<f64>| t.map_or("-".into(), |t| format!("{t:.0} s"));
    let pos = pos
        .iter()
        .map(|(at, t)| format!("+{at} s {}", s(*t)))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "playing after {:.1} s; while playing: {playing}; time-pos {} at the pause; \
         while paused: {during}; {} cached at the end of it; resume {}; after: {after}; \
         time-pos {pos}",
        started.as_secs_f64(),
        s(at_pause),
        s(cache),
        if resume.is_ok() { "ok" } else { "refused" },
    ))
}
