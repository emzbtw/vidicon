//! `--probe-archive <stream_id>` (`devtools` only, catch-up ticket 01): asks
//! the provider how it serves its archive, and prints what it answers as
//! shapes, counts and time offsets. `--probe-tables <id,...>` prints only
//! what each channel's guide table holds, and `--probe-vod` how it serves
//! movies and series (VOD ticket 01). `--probe-alternates` counts, from the
//! database alone, how many channels have an alternate, and
//! `--probe-pause` what pausing a channel does (stream recovery ticket 01),
//! `--probe-blackout` what a blackout looks like to mpv (ticket 05),
//! and `--probe-epg` how an external guide matches (EPG sources ticket 01). Never URLs, hosts, credentials or titles, so the output can
//! be pasted anywhere. Every request goes through the client's gate (SPEC
//! 6.2).

use crate::config::{self, Provider};
use crate::import::{ChunkReader, parse_xmltv, unix_now};
use crate::store;
use crate::xtream::{Client, url_time, wall};
use reqwest::Url;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::BufReader;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

mod blackout;
mod epg;
mod pause;
mod vod;

pub use blackout::run as blackout;
pub use epg::run as epg;
pub use pause::run as pause;
pub use vod::run as vod;

const GAP: Duration = Duration::from_secs(2);

/// Each channel's `get_simple_data_table`: how many programmes, how far
/// back and ahead (on the server's clock, so a couple of hours out), and
/// how many have `has_archive`. Requests are spaced by the client's gate.
pub async fn tables(provider: Provider, ids: Vec<i64>) -> Result<(), String> {
    let client = Client::new(provider);
    let now = unix_now();
    for (n, id) in (1..).zip(ids) {
        let progs = match client.guide_table(id).await {
            Ok(body) => crate::tables::parse(&body),
            Err(e) => Err(e),
        };
        match progs {
            Err(e) => println!("channel {n}: {}", crate::xtream::redact(&e)),
            Ok(p) if p.is_empty() => println!("channel {n}: no programmes"),
            Ok(p) => {
                let first = p.iter().map(|p| p.start).min().unwrap_or(now);
                let last = p.iter().map(|p| p.stop).max().unwrap_or(now);
                let archived = p.iter().filter(|p| p.archive == Some(true)).count();
                println!(
                    "channel {n}: {} programmes, {:.1} days back, {:.1} ahead, {archived} with has_archive",
                    p.len(),
                    (now - first) as f64 / 86_400.0,
                    (last - now) as f64 / 86_400.0,
                );
            }
        }
    }
    Ok(())
}

/// For All's channels, the favourites and Recently played, how many have
/// no alternate, one, two, and three or more. No provider requests.
pub fn alternates(provider_id: &str) -> Result<(), String> {
    use store::AlternatesOf::*;
    let db = store::open_reader(&config::database_path()).map_err(|e| e.to_string())?;
    for (name, of) in [
        ("shown", Shown),
        ("favourites", Favourites),
        ("recent", Recent),
    ] {
        let t = std::time::Instant::now();
        let [none, one, two, more] =
            store::alternate_spread(&db, provider_id, of).map_err(|e| e.to_string())?;
        println!(
            "{name}: {} channels; no alternate {none}, 1: {one}, 2: {two}, 3+: {more} ({} ms)",
            none + one + two + more,
            t.elapsed().as_millis()
        );
    }
    Ok(())
}

pub async fn run(provider: Provider, mpv: &str, stream_id: i64) -> Result<(), String> {
    let client = Client::new(provider);
    let now = unix_now();
    let local = crate::ui::local_tm(now).tm_gmtoff;
    println!("local offset {}", offset(local));

    // 1. Account: the server's clock and zone.
    let account = client.account().await?;
    let tz = account.server_info.timezone.as_deref().unwrap_or("-");
    let server_clock = account.server_info.time_now.as_deref().and_then(wall);
    let clock_offset = server_clock.map(|t| ((t - now) as f64 / 900.0).round() as i64 * 900);
    println!(
        "account: timezone {tz}, time_now offset {}",
        clock_offset.map_or("-".into(), offset)
    );

    // 2. The channel's own guide.
    let table = client.simple_data_table(stream_id).await?;
    let pick = summarise_table(&table, now);

    // 3. How far back xmltv.php reaches.
    let guide_id = store::open_reader(&config::database_path())
        .ok()
        .and_then(|c| {
            c.query_row(
                "SELECT epg_channel_id FROM channel WHERE stream_id = ?1",
                [stream_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
        });
    xmltv_reach(&client, guide_id.as_deref(), now).await;

    // 4. Archive URL forms and start zones.
    let (start, minutes) = pick.unwrap_or_else(|| {
        println!("no past programme in the table: trying a synthetic hour 24 h back");
        ((now - 86_400) / 3600 * 3600, 60)
    });
    let mut zones = vec![("utc", 0)];
    for (name, off) in [("server", pick_offset(&table)), ("local", Some(local))] {
        if let Some(off) = off
            && !zones.iter().any(|z| z.1 == off)
        {
            zones.push((name, off));
        }
    }
    println!(
        "archive: programme starting {} h ago, {minutes} min",
        (now - start) / 3600
    );
    let mut working: Option<(&str, i64)> = None;
    for &(zone, off) in &zones {
        let s = url_time(start + off);
        let url = client.timeshift_url(stream_id, &s, minutes, "ts")?;
        let h = head(&client, url, 4 << 20, Duration::from_secs(20)).await;
        println!(
            "  /timeshift/ .ts, start in {zone} ({}): {}",
            offset(off),
            h.describe(start)
        );
        if h.ok() && working.is_none() {
            working = Some((zone, off));
        }
    }
    // Every zone answers with video, so later requests use the server's
    // clock, which the 115/125 min check below confirms.
    let (zone, off) = clock_offset.map_or(working.unwrap_or(zones[0]), |o| ("server clock", o));
    let s = url_time(start + off);
    let url = client.timeshift_php_url(stream_id, &s, minutes)?;
    let h = head(&client, url, 4 << 20, Duration::from_secs(20)).await;
    println!("  timeshift.php, start in {zone}: {}", h.describe(start));
    let url = client.timeshift_url(stream_id, &s, minutes, "m3u8")?;
    let h = head(&client, url, 1 << 20, Duration::from_secs(20)).await;
    println!(
        "  /timeshift/ .m3u8, start in {zone}: {}",
        h.describe(start)
    );

    // 5. Duration and bounds.
    let url = client.timeshift_url(stream_id, &s, 1, "ts")?;
    let h = head(&client, url, 64 << 20, Duration::from_secs(60)).await;
    println!("  1 min asked for: {}", h.describe(start));
    let old = url_time(now - 10 * 86_400 + off);
    let h = head(
        &client,
        client.timeshift_url(stream_id, &old, 60, "ts")?,
        1 << 16,
        Duration::from_secs(10),
    )
    .await;
    println!("  start 10 days back: {}", h.describe(now - 10 * 86_400));
    let soon = url_time(now + 86_400 + off);
    let h = head(
        &client,
        client.timeshift_url(stream_id, &soon, 60, "ts")?,
        1 << 16,
        Duration::from_secs(10),
    )
    .await;
    println!("  start 1 day ahead: {}", h.describe(now + 86_400));
    let h = head(
        &client,
        client.stream_url(stream_id, "ts")?,
        4 << 20,
        Duration::from_secs(20),
    )
    .await;
    println!("  live, for reference: {}", h.describe(unix_now()));

    // The zone the URL's start is read in: a start in the future is refused,
    // so a UTC clock reading `ahead` is accepted only by a server whose zone
    // is more than `ahead` east of UTC. Neighbouring readings bracket its zone.
    for ahead in [30, 90, 115, 125] {
        let t = url_time(unix_now() + ahead * 60);
        let h = head(
            &client,
            client.timeshift_url(stream_id, &t, 10, "ts")?,
            1 << 16,
            Duration::from_secs(10),
        )
        .await;
        println!("  UTC clock {ahead} min ahead: {}", h.describe(unix_now()));
    }

    // Start over: a programme that began 20 min ago and runs on past now.
    let t = url_time(unix_now() - 20 * 60 + off);
    let h = head(
        &client,
        client.timeshift_url(stream_id, &t, 60, "ts")?,
        64 << 20,
        Duration::from_secs(60),
    )
    .await;
    println!(
        "  60 min from 20 min ago: {}",
        h.describe(unix_now() - 20 * 60)
    );
    let h = head(
        &client,
        client.timeshift_url(stream_id, &t, 60, "m3u8")?,
        1 << 20,
        Duration::from_secs(20),
    )
    .await;
    println!("  the same as .m3u8: {}", h.describe(unix_now() - 20 * 60));

    // 6. mpv.
    for ext in ["ts", "m3u8"] {
        tokio::time::sleep(GAP).await;
        let url = client.timeshift_url(stream_id, &s, minutes, ext)?;
        match mpv_seek(mpv, &url, false).await {
            Ok(r) => println!("mpv .{ext}: {r}"),
            Err(e) => println!("mpv .{ext}: {}", crate::xtream::redact(&e)),
        }
    }
    Ok(())
}

/// Prints the table's shape and reach; returns a programme to replay: ended
/// over an hour ago, flagged as archived if any are, starting nearest 24 h
/// back. `(start, minutes)`.
fn summarise_table(v: &Value, now: i64) -> Option<(i64, i64)> {
    let keys = |v: &Value| {
        v.as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>().join(","))
            .unwrap_or_else(|| format!("not an object: {}", kind(v)))
    };
    println!("table: keys {}", keys(v));
    let list = v["epg_listings"].as_array()?;
    println!(
        "  {} listings, fields {}",
        list.len(),
        list.first().map_or("-".into(), keys)
    );
    let times: Vec<(i64, i64, bool)> = list
        .iter()
        .filter_map(|l| {
            let a = matches!(int(&l["has_archive"]), Some(1));
            Some((int(&l["start_timestamp"])?, int(&l["stop_timestamp"])?, a))
        })
        .collect();
    let (Some(first), Some(last)) = (
        times.iter().map(|t| t.0).min(),
        times.iter().map(|t| t.1).max(),
    ) else {
        return None;
    };
    let archived = times.iter().filter(|t| t.2).count();
    let oldest_archived = times.iter().filter(|t| t.2).map(|t| t.0).min();
    println!(
        "  reaches {:.1} h back, {:.1} h ahead; {archived} with has_archive, oldest {}",
        (now - first) as f64 / 3600.0,
        (last - now) as f64 / 3600.0,
        oldest_archived.map_or("-".into(), |t| format!(
            "{:.1} h back",
            (now - t) as f64 / 3600.0
        )),
    );
    println!(
        "  server offset from start vs start_timestamp: {}",
        pick_offset(v).map_or("-".into(), offset)
    );
    let flagged = archived > 0;
    times
        .iter()
        .filter(|t| t.1 < now - 3600 && (t.2 || !flagged))
        .min_by_key(|t| (t.0 - (now - 86_400)).abs())
        .map(|t| (t.0, (t.1 - t.0) / 60))
}

/// The server's offset, from the first listing's `start` (its wall clock)
/// against its `start_timestamp`.
fn pick_offset(v: &Value) -> Option<i64> {
    let l = v["epg_listings"].as_array()?.first()?;
    Some(wall(l["start"].as_str()?)? - int(&l["start_timestamp"])?)
}

async fn xmltv_reach(client: &Client, guide_id: Option<&str>, now: i64) {
    let (tx, rx) = mpsc::channel(16);
    let job = tokio::task::spawn_blocking(move || {
        let mut first: HashMap<String, i64> = HashMap::new();
        let mut total = 0;
        let r = parse_xmltv(
            BufReader::with_capacity(64 * 1024, ChunkReader::new(rx)),
            |p| {
                total += 1;
                let e = first.entry(p.epg_channel_id.to_owned()).or_insert(p.start);
                *e = (*e).min(p.start);
                Ok(())
            },
        );
        (r, first, total)
    });
    let fetched = client.xmltv(tx).await;
    let Ok((parsed, first, total)) = job.await else {
        return println!("xmltv: parse task failed");
    };
    if let Err(e) = fetched.and(parsed.map(|_| 0)) {
        return println!("xmltv: {}", crate::xtream::redact(&e));
    }
    let mut buckets = [0; 6];
    for &start in first.values() {
        let h = (now - start) / 3600;
        buckets[match h {
            ..1 => 0,
            1..6 => 1,
            6..24 => 2,
            24..72 => 3,
            72..168 => 4,
            _ => 5,
        }] += 1;
    }
    println!(
        "xmltv: {total} programmes, {} guide ids; reach back <1h {} | 1-6h {} | 6-24h {} | 1-3d {} | 3-7d {} | >7d {}",
        first.len(),
        buckets[0],
        buckets[1],
        buckets[2],
        buckets[3],
        buckets[4],
        buckets[5]
    );
    match guide_id.and_then(|g| first.get(g)) {
        Some(&t) => println!(
            "  this channel's guide reaches {:.1} h back",
            (now - t) as f64 / 3600.0
        ),
        None => println!("  this channel's guide id is not in it (or not stored)"),
    }
}

/// What the first bytes of an archive request were.
struct Head {
    error: Option<String>,
    timed_out: bool,
    ended: bool,
    body: Vec<u8>,
}

/// GETs `url` through the gate, keeping up to `max` bytes, for at most `wait`.
async fn head(client: &Client, url: Url, max: usize, wait: Duration) -> Head {
    let (tx, rx) = mpsc::channel::<crate::xtream::Chunk>(16);
    let mut h = Head {
        error: None,
        timed_out: false,
        ended: false,
        body: Vec::new(),
    };
    let done = tokio::time::timeout(wait, async {
        let collect = async {
            let mut rx = rx;
            while let Some(chunk) = rx.recv().await {
                match chunk {
                    Ok(b) => {
                        h.body.extend_from_slice(&b);
                        if h.body.len() >= max {
                            return;
                        }
                    }
                    Err(e) => {
                        h.error = Some(e);
                        return;
                    }
                }
            }
            h.ended = true;
        };
        tokio::join!(client.fetch(url, tx), collect).0
    })
    .await;
    match done {
        Err(_) => h.timed_out = true,
        Ok(Err(e)) if h.error.is_none() => h.error = Some(e),
        Ok(_) => {}
    }
    // A timeout drops the request before the gate records it.
    tokio::time::sleep(GAP).await;
    h
}

impl Head {
    fn ok(&self) -> bool {
        self.error.is_none() && ts_sync(&self.body).is_some()
    }

    /// One line: size, how it ended, and what the body is. For MPEG-TS, the
    /// broadcast clock (TDT) against `expected`, and the PTS span.
    fn describe(&self, expected: i64) -> String {
        if let Some(e) = &self.error {
            return crate::xtream::redact(e);
        }
        let how = if self.ended {
            "ended"
        } else if self.timed_out {
            "timed out"
        } else {
            "cut at cap"
        };
        let b = &self.body;
        let what = if b.starts_with(b"#EXTM3U") {
            playlist(&String::from_utf8_lossy(b))
        } else if let Some(o) = ts_sync(b) {
            let (tdt, span) = ts_info(&b[o..]);
            format!(
                "MPEG-TS, PTS span {}, TDT {}",
                span.map_or("-".into(), |s| format!("{s:.0} s")),
                tdt.map_or("none".into(), |t| format!(
                    "{} from expected",
                    signed_min(t - expected)
                ))
            )
        } else {
            format!("not TS or a playlist ({})", kind_bytes(b))
        };
        format!("{} KiB, {how}: {what}", b.len() / 1024)
    }
}

/// A playlist's shape, without its URLs.
fn playlist(s: &str) -> String {
    let variants = s
        .lines()
        .filter(|l| l.starts_with("#EXT-X-STREAM-INF"))
        .count();
    if variants > 0 {
        return format!("master playlist, {variants} variants");
    }
    let secs: f64 = s
        .lines()
        .filter_map(|l| {
            l.strip_prefix("#EXTINF:")?
                .split(',')
                .next()?
                .parse::<f64>()
                .ok()
        })
        .sum();
    let segs = s.lines().filter(|l| l.starts_with("#EXTINF")).count();
    let absolute = s.lines().any(|l| l.starts_with("http"));
    format!(
        "media playlist, {segs} segments, {:.1} min, ENDLIST {}, segment URLs {}",
        secs / 60.0,
        s.contains("#EXT-X-ENDLIST"),
        if absolute { "absolute" } else { "relative" }
    )
}

fn ts_sync(b: &[u8]) -> Option<usize> {
    (0..188).find(|&o| (0..3).all(|k| b.get(o + k * 188) == Some(&0x47)))
}

/// The first TDT or TOT time, and the longest PTS span of any PID.
fn ts_info(b: &[u8]) -> (Option<i64>, Option<f64>) {
    let mut tdt = None;
    let mut pts: HashMap<u16, (i64, i64)> = HashMap::new();
    for p in b.as_chunks::<188>().0 {
        if p[0] != 0x47 {
            continue;
        }
        let pid = u16::from(p[1] & 0x1f) << 8 | u16::from(p[2]);
        if pid == 0x14 && p[1] & 0x40 != 0 && tdt.is_none() {
            let payload = match (p[3] >> 4) & 3 {
                1 => Some(&p[4..]),
                3 => p.get(5 + p[4] as usize..),
                _ => None,
            };
            if let Some(pl) = payload
                && let Some(sec) = pl.get(1 + *pl.first().unwrap_or(&0) as usize..)
                && sec.len() >= 8
                && matches!(sec[0], 0x70 | 0x73)
            {
                let mjd = i64::from(sec[3]) << 8 | i64::from(sec[4]);
                let bcd = |x: u8| i64::from(x >> 4) * 10 + i64::from(x & 15);
                tdt = Some(
                    (mjd - 40_587) * 86_400 + bcd(sec[5]) * 3600 + bcd(sec[6]) * 60 + bcd(sec[7]),
                );
            }
        }
        if let Some(t) = crate::ts::frame_pts(p) {
            let e = pts.entry(pid).or_insert((t, t));
            e.1 = t;
        }
    }
    let span = pts
        .values()
        .map(|&(a, b)| (b - a) as f64 / 90_000.0)
        .filter(|s| *s >= 0.0)
        .reduce(f64::max);
    (tdt, span)
}

/// Plays `url` in a headless mpv, then seeks 300 s on and 120 s back. With
/// `resume`, loads it again with `start=600`, as resuming would.
async fn mpv_seek(mpv: &str, url: &Url, resume: bool) -> Result<String, String> {
    let sock = config::mpv_socket_path().with_file_name("probe.sock");
    if let Some(dir) = sock.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(&sock);
    let _child = tokio::process::Command::new(mpv)
        .args(["--no-config", "--idle=yes", "--vo=null", "--ao=null"])
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
    let (rd, mut wr) = conn.ok_or("no IPC socket")?.into_split();
    let mut lines = AsyncBufReader::new(rd).lines();
    let mut id = 0;
    let mut cmd = async |c: Value| -> Result<Value, String> {
        id += 1;
        let line = json!({ "command": c, "request_id": id }).to_string() + "\n";
        wr.write_all(line.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        while let Some(l) = lines.next_line().await.map_err(|e| e.to_string())? {
            let v: Value = serde_json::from_str(&l).unwrap_or_default();
            if v["request_id"] == id {
                return if v["error"] == "success" {
                    Ok(v["data"].clone())
                } else {
                    Err(v["error"].as_str().unwrap_or("?").to_owned())
                };
            }
        }
        Err("mpv closed".into())
    };
    cmd(json!(["loadfile", url.as_str(), "replace"])).await?;
    let mut t0 = None;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Ok(t) = cmd(json!(["get_property", "time-pos"])).await
            && t.is_number()
        {
            t0 = t.as_f64();
            break;
        }
    }
    let Some(t0) = t0 else {
        return Ok("never started playing in 30 s".into());
    };
    let seekable = cmd(json!(["get_property", "seekable"]))
        .await
        .unwrap_or(Value::Null);
    let duration = cmd(json!(["get_property", "duration"]))
        .await
        .unwrap_or(Value::Null);
    let r1 = cmd(json!(["seek", 300, "relative"])).await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    let t1 = cmd(json!(["get_property", "time-pos"]))
        .await
        .ok()
        .and_then(|v| v.as_f64());
    let r2 = cmd(json!(["seek", -120, "relative"])).await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    let t2 = cmd(json!(["get_property", "time-pos"]))
        .await
        .ok()
        .and_then(|v| v.as_f64());
    let mut t3 = None;
    if resume {
        cmd(json!([
            "loadfile",
            url.as_str(),
            "replace",
            -1,
            "start=600"
        ]))
        .await?;
        tokio::time::sleep(Duration::from_secs(3)).await;
        for _ in 0..40 {
            if let Ok(t) = cmd(json!(["get_property", "time-pos"])).await
                && t.is_number()
            {
                t3 = t.as_f64();
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    let _ = cmd(json!(["quit"])).await;
    let at = |t: Option<f64>| t.map_or("-".into(), |t| format!("{t:.0} s"));
    Ok(format!(
        "playing at {t0:.0} s, seekable {seekable}, duration {duration}; +300 {} -> {}; -120 {} -> {}{}",
        if r1.is_ok() { "ok" } else { "refused" },
        at(t1),
        if r2.is_ok() { "ok" } else { "refused" },
        at(t2),
        if resume {
            format!("; loaded with start=600 -> {}", at(t3))
        } else {
            String::new()
        },
    ))
}

fn int(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str()?.trim().parse().ok())
}

fn offset(secs: i64) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    format!(
        "UTC{sign}{:02}:{:02}",
        secs.abs() / 3600,
        secs.abs() % 3600 / 60
    )
}

fn signed_min(secs: i64) -> String {
    format!("{:+.1} min", secs as f64 / 60.0)
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn kind_bytes(b: &[u8]) -> &'static str {
    match b.iter().find(|c| !c.is_ascii_whitespace()) {
        None => "empty",
        Some(b'{' | b'[') => "JSON",
        Some(b'<') => "HTML or XML",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tdt_is_read() {
        // MJD 61314 is 2026-10-01; 20:05:09 in BCD.
        let mut p = vec![0x47, 0x40, 0x14, 0x10, 0x00, 0x70, 0x70, 0x05];
        p.extend([(61_314 >> 8) as u8, (61_314 & 0xff) as u8, 0x20, 0x05, 0x09]);
        p.resize(188, 0xff);
        let (tdt, _) = ts_info(&p);
        assert_eq!(tdt, Some(wall("2026-10-01 20:05:09").unwrap()));
    }
}
