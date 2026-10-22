//! The single external mpv, driven over JSON IPC (SPEC 9; ADR 0002).
//!
//! Spawned on first play and reused: switching is one `loadfile ... replace`.
//! mpv exiting (its window closed) shows up as the IPC socket closing; the UI
//! is told playback stopped and the next play spawns a fresh mpv. mpv's
//! pause, mute and fullscreen states are observed, so the app's controls
//! follow keys pressed inside mpv too (SPEC 9.1). Time paused since the
//! last `loadfile` is how far [`Behind`] live the picture is; going live
//! reloads the stream (ticket 14). Recording what plays is mpv's
//! `dump-cache` from the time shown, since mpv reads well ahead of it; on a
//! stop, what it had read past the time shown is cut off the file (SPEC 9.4).
//! A live channel that drops is played again a few times before the app
//! gives up (stream recovery ticket 02, SPEC 9.3).

use crate::m3u::Headers;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// How to start mpv.
pub struct Launch {
    pub program: String,
    pub extra_args: Vec<String>,
    pub socket: PathBuf,
}

/// What the Player reports back to the UI.
#[derive(Debug, PartialEq)]
pub enum Event {
    /// mpv started. Its pause, mute and fullscreen states follow.
    Started,
    /// mpv exited, e.g. its window was closed.
    Stopped,
    Paused(bool),
    Muted(bool),
    Fullscreen(bool),
    /// A stream failed to play (SPEC 9.3).
    Failed {
        title: String,
        reason: String,
    },
    /// A key bound in mpv at spawn (SPEC 9.2).
    Remote(Remote),
    /// How far behind live, after each pause change and `loadfile`.
    Behind(Behind),
    /// Recording what plays started, to this file.
    Recording(PathBuf),
    /// The recording ended: stopped, the channel switched, the stream or mpv
    /// ended, or, with mpv's reason, it failed.
    RecordingEnded(Option<String>),
    /// mpv's `time-pos`, as [`Player::position`] asked.
    Position(f64),
    /// mpv's `duration`, as [`Player::length`] asked: a movie's length
    /// (VOD ticket 04).
    Length(f64),
    /// What plays reached its end: an archive stream ran out (catch-up
    /// ticket 04). Only for the latest `loadfile`.
    Ended,
    /// Sent by the app's player task as it hands play `n` to the Player, so
    /// positions reported after it are that play's (VOD ticket 07).
    Handed(u64),
    /// A live channel dropped and is being played again; `false` once it
    /// plays, or the user did something else. Giving up is
    /// [`Event::Failed`].
    Reconnecting(bool),
}

/// The app's keys inside mpv (SPEC 9.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Remote {
    Prev,
    Next,
    Digit(u8),
    /// Back to the list: raise the app window.
    Back,
    /// `L`: go live.
    Live,
    /// `R`: record what plays, or stop.
    Record,
    /// `S`: start over what airs now (catch-up ticket 04).
    StartOver,
}

/// The app's player controls (SPEC 9.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Control {
    Pause,
    Mute,
    Fullscreen,
    /// Closes mpv as if the user closed its window.
    Stop,
    /// Reloads the channel playing at the live edge, unpaused; nothing while
    /// at live.
    Live,
    /// Back to the start of what plays: a movie or an episode (VOD ticket
    /// 07).
    Restart,
}

/// How far the picture trails the live broadcast: the time paused since the
/// stream last loaded. The streams report no live edge, so it is counted.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Behind {
    /// Paused time already over.
    settled: Duration,
    /// Paused since then, if paused now.
    paused_since: Option<Instant>,
}

impl Behind {
    fn pause(&mut self, on: bool, now: Instant) {
        match (on, self.paused_since) {
            (true, None) => self.paused_since = Some(now),
            (false, Some(since)) => {
                self.settled += now.saturating_duration_since(since);
                self.paused_since = None;
            }
            _ => {}
        }
    }

    /// A `loadfile`: back at live. A stream loaded while paused stays paused
    /// and counts from `now`, unless the load also unpauses.
    fn reload(&mut self, unpause: bool, now: Instant) {
        *self = Self {
            settled: Duration::ZERO,
            paused_since: self.paused_since.filter(|_| !unpause).map(|_| now),
        };
    }

    /// How far behind live as of `now`.
    pub fn at(&self, now: Instant) -> Duration {
        self.settled
            + self
                .paused_since
                .map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
    }

    /// Paused now, so [`Behind::at`] is growing.
    pub fn paused(&self) -> bool {
        self.paused_since.is_some()
    }
}

/// When a live channel that dropped is played again: each retry this long
/// after the drop or the retry before it failed (stream recovery ticket 02).
pub(crate) const RETRIES: [Duration; 2] = [Duration::from_secs(3), Duration::from_secs(10)];

/// mpv's own reconnect, before the app's [`RETRIES`]: it hides a closed
/// connection, coming back at the live edge (SPEC 9.3). Appended one each, so
/// a `stream-lavf-o` in the user's `mpv.conf` is kept.
pub(crate) const RECONNECT: [&str; 3] = [
    "--stream-lavf-o-append=reconnect=1",
    "--stream-lavf-o-append=reconnect_streamed=1",
    "--stream-lavf-o-append=reconnect_delay_max=10",
];

/// Properties observed at spawn; mpv reports each one's value straight away
/// and again on every change.
const OBSERVED: [&str; 3] = ["pause", "mute", "fullscreen"];

/// Asks for mpv's error messages, where a failed recording is reported: the
/// `dump-cache` reply alone says only "error running command".
const LOG_MESSAGES: &str = "{\"command\":[\"request_log_messages\",\"error\"]}\n";

/// How long a stop waits for mpv to exit before killing it.
const QUIT_WAIT: Duration = Duration::from_secs(2);

/// `(mpv key, script-message argument)`. Each key sends
/// `script-message vidicon <argument>`, which reaches every IPC client as a
/// `client-message` event. `keybind` replaces only these keys, so the user's
/// `input.conf` otherwise still applies.
const KEYS: [(&str, &str); 16] = [
    ("PGUP", "prev"),
    ("PGDWN", "next"),
    ("BS", "back"),
    ("L", "live"),
    ("R", "record"),
    ("S", "start-over"),
    ("0", "digit 0"),
    ("1", "digit 1"),
    ("2", "digit 2"),
    ("3", "digit 3"),
    ("4", "digit 4"),
    ("5", "digit 5"),
    ("6", "digit 6"),
    ("7", "digit 7"),
    ("8", "digit 8"),
    ("9", "digit 9"),
];

/// The `keybind` commands for [`KEYS`], one JSON line each.
fn keybinds() -> String {
    KEYS.iter()
        .map(|(key, arg)| {
            let cmd = format!("script-message vidicon {arg}");
            format!("{}\n", json!({ "command": ["keybind", key, cmd] }))
        })
        .collect()
}

/// The `observe_property` commands for [`OBSERVED`], one JSON line each.
fn observes() -> String {
    (1..)
        .zip(OBSERVED)
        .map(|(id, prop)| format!("{}\n", json!({ "command": ["observe_property", id, prop] })))
        .collect()
}

/// A `property-change` event, if it is one of [`OBSERVED`] with a value.
fn property(msg: &Value) -> Option<Event> {
    let on = msg["data"].as_bool()?;
    Some(match msg["name"].as_str()? {
        "pause" => Event::Paused(on),
        "mute" => Event::Muted(on),
        "fullscreen" => Event::Fullscreen(on),
        _ => return None,
    })
}

/// A `client-message` event's arguments, if they are one of ours.
fn remote(args: &Value) -> Option<Remote> {
    let args: Vec<&str> = args.as_array()?.iter().filter_map(Value::as_str).collect();
    Some(match args[..] {
        ["vidicon", "prev"] => Remote::Prev,
        ["vidicon", "next"] => Remote::Next,
        ["vidicon", "back"] => Remote::Back,
        ["vidicon", "live"] => Remote::Live,
        ["vidicon", "record"] => Remote::Record,
        ["vidicon", "start-over"] => Remote::StartOver,
        ["vidicon", "digit", d] => Remote::Digit(d.parse().ok().filter(|&d| d <= 9)?),
        _ => return None,
    })
}

pub struct Player {
    launch: Launch,
    events: mpsc::UnboundedSender<Event>,
    running: Option<Running>,
    next_request: i64,
    /// The URL, headers and title last loaded, for going live.
    current: Option<(String, Option<Headers>, String)>,
    /// The latest `loadfile`'s request id: [`Signal`]s about older ones are
    /// stale.
    load: i64,
    /// What each mpv's [`read_events`] says about the latest `loadfile`.
    signal_tx: mpsc::UnboundedSender<(i64, Signal)>,
    signals: mpsc::UnboundedReceiver<(i64, Signal)>,
    /// Reconnecting a live channel that dropped, while it does.
    reconnect: Option<Reconnect>,
    /// [`RETRIES`]; shorter in tests.
    retries: &'static [Duration],
}

/// What [`read_events`] tells the Player about the latest `loadfile`.
#[derive(Debug)]
pub enum Signal {
    /// A live channel dropped, with mpv's reason.
    Dropped(String),
    /// A retry plays.
    Back,
}

/// What [`Player::wait`] waited for, for [`Player::wake`].
#[derive(Debug)]
pub enum Wake {
    Signal(i64, Signal),
    /// A retry is due.
    Retry,
}

struct Reconnect {
    /// Retries sent so far.
    sent: usize,
    /// When the next is due; `None` while one loads.
    due: Option<Instant>,
}

struct Running {
    /// Dropping this kills mpv (see [`watch`]).
    _kill: oneshot::Sender<()>,
    ipc: OwnedWriteHalf,
    titles: Arc<Mutex<Titles>>,
    behind: Arc<Mutex<Behind>>,
    /// [`read_events`], which ends when mpv exits.
    reader: JoinHandle<()>,
    /// A recording runs; cleared by whichever side ends it, which reports
    /// [`Event::RecordingEnded`].
    recording: Arc<AtomicBool>,
    /// The recording's `dump-cache`, while one runs.
    dump: Option<Dump>,
    /// Replies [`Player::request`] and a [`Dump`] wait for, by request id.
    replies: Replies,
}

type Replies = Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>;

/// A continuous `dump-cache`: mpv answers it once the dump has ended and
/// the file is closed.
struct Dump {
    file: PathBuf,
    done: oneshot::Receiver<Value>,
}

/// How long to wait for mpv to answer a request, or to close a dump.
const REPLY_WAIT: Duration = Duration::from_secs(2);

/// Maps mpv's playlist entries back to channel titles, so an error names the
/// channel that failed even if the user has switched since.
#[derive(Default)]
struct Titles {
    alive: bool,
    by_request: HashMap<i64, String>,
    by_entry: VecDeque<(i64, String)>,
    last: String,
    /// When the latest `loadfile` was sent and what it plays, for the drop
    /// log (stream recovery ticket 01).
    loaded: Option<(Instant, &'static str)>,
    /// The latest `loadfile`'s request id, for its [`Signal`]s.
    load: i64,
    /// The latest `loadfile` has played: mpv sent `playback-restart`.
    played: bool,
    /// A `seek` since the last `playback-restart`: the next one is the
    /// seek's, not mpv reconnecting.
    seeking: bool,
    /// The latest `loadfile` is a retry, so failing to start is a drop too.
    retry: bool,
}

/// What a stream URL plays, from its path: `live`, `catch-up`, `movie` or
/// `episode`. Logged in place of the URL, which carries credentials.
fn stream_kind(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    if path.contains("/live/") {
        "live"
    } else if path.contains("/timeshift/") || path.contains("/timeshift.php") {
        "catch-up"
    } else if path.contains("/movie/") {
        "movie"
    } else if path.contains("/series/") {
        "episode"
    } else {
        "other"
    }
}

/// `loadfile`'s options: where to start, and an M3U channel's headers
/// (M3U ticket 03). mpv sets them for that file alone, so they never
/// reach the next. A header value is written `%<bytes>%<value>`, which
/// mpv takes as is, commas included; the `Origin` line, a list item in
/// `http-header-fields`, has its commas escaped too.
fn loadfile_options(start: Option<f64>, headers: Option<&Headers>) -> Option<String> {
    let quoted = |v: &str| format!("%{}%{v}", v.len());
    let mut options = Vec::new();
    if let Some(s) = start {
        options.push(format!("start={s}"));
    }
    if let Some(h) = headers {
        if let Some(agent) = &h.user_agent {
            options.push(format!("user-agent={}", quoted(agent)));
        }
        if let Some(referrer) = &h.referrer {
            options.push(format!("referrer={}", quoted(referrer)));
        }
        if let Some(origin) = &h.origin {
            let field = format!("Origin: {}", origin.replace(',', "\\,"));
            options.push(format!("http-header-fields={}", quoted(&field)));
        }
    }
    (!options.is_empty()).then(|| options.join(","))
}

const ENTRIES_KEPT: usize = 8;
const TITLE: &str = "--title=${?media-title:${media-title}}${!media-title:No file} - mpv";

impl Player {
    pub fn new(launch: Launch, events: mpsc::UnboundedSender<Event>) -> Self {
        let (signal_tx, signals) = mpsc::unbounded_channel();
        Self {
            launch,
            events,
            running: None,
            next_request: 1,
            current: None,
            load: 0,
            signal_tx,
            signals,
            reconnect: None,
            retries: &RETRIES,
        }
    }

    /// How the next mpv starts (settings ticket 02); one running keeps
    /// going as it is.
    pub fn set_launch(&mut self, launch: Launch) {
        self.launch = launch;
    }

    /// Waits for what [`Player::wake`] acts on: what mpv said about the
    /// latest `loadfile`, or a retry falling due. Cancel safe, so the app's
    /// player task can wait on it beside its commands.
    pub async fn wait(&mut self) -> Wake {
        let due = self.reconnect.as_ref().and_then(|r| r.due);
        let at = tokio::time::Instant::from_std(due.unwrap_or_else(Instant::now));
        tokio::select! {
            signal = self.signals.recv() => {
                let (load, signal) = signal.expect("the Player holds a sender");
                Wake::Signal(load, signal)
            }
            () = tokio::time::sleep_until(at), if due.is_some() => Wake::Retry,
        }
    }

    /// Acts on what [`Player::wait`] returned: a drop starts reconnecting or
    /// schedules the next retry, or gives up after the last; a retry playing
    /// ends it. Each retry plays the channel again at the live edge,
    /// unpaused, as going live does (stream recovery ticket 02).
    pub async fn wake(&mut self, wake: Wake) {
        match wake {
            Wake::Signal(load, _) if load != self.load => {}
            Wake::Signal(_, Signal::Dropped(reason)) => {
                let sent = self.reconnect.as_ref().map_or(0, |r| r.sent);
                match self.retries.get(sent) {
                    Some(after) => {
                        log::info!(
                            "live channel dropped ({reason}); retry {} in {} s",
                            sent + 1,
                            after.as_secs_f64()
                        );
                        if self.reconnect.is_none() {
                            let _ = self.events.send(Event::Reconnecting(true));
                        }
                        self.reconnect = Some(Reconnect {
                            sent,
                            due: Some(Instant::now() + *after),
                        });
                    }
                    None => {
                        log::info!("live channel dropped ({reason}); gave up after {sent} retries");
                        self.reconnect = None;
                        let title = self.current.as_ref().map_or("", |(_, _, t)| t).to_string();
                        let _ = self.events.send(Event::Failed { title, reason });
                    }
                }
            }
            Wake::Signal(_, Signal::Back) => {
                if let Some(r) = self.reconnect.take() {
                    log::info!("live channel back after {} retries", r.sent);
                    let _ = self.events.send(Event::Reconnecting(false));
                }
            }
            Wake::Retry => {
                let Some(r) = self.reconnect.as_mut() else {
                    return;
                };
                r.due = None;
                r.sent += 1;
                let alive = self
                    .running
                    .as_ref()
                    .is_some_and(|r| r.titles.lock().unwrap().alive);
                let current = self.current.clone().filter(|_| alive);
                let Some((url, headers, title)) = current else {
                    // mpv was closed: not a drop.
                    return self.cancel_reconnect();
                };
                if let Err(e) = self.reload(&url, headers.as_ref(), &title).await {
                    log::error!("{e}");
                    self.cancel_reconnect();
                }
            }
        }
    }

    /// The user played or controlled something: no more retries.
    fn cancel_reconnect(&mut self) {
        if self.reconnect.take().is_some() {
            let _ = self.events.send(Event::Reconnecting(false));
        }
    }

    /// [`Player::play_at`] from the start.
    #[cfg(test)]
    pub async fn play(&mut self, url: &str, title: &str, pressed: Instant) -> Result<(), String> {
        self.play_at(url, None, title, None, pressed).await
    }

    /// Plays `url`, spawning mpv first if it is not running, from `start`
    /// seconds in when given: a movie or episode resuming (VOD ticket 07).
    /// `headers` are an M3U channel's (M3U ticket 03), for this `url` only.
    /// `title` becomes mpv's media title before the URL is loaded, so the
    /// URL (which carries credentials) never becomes the window title.
    /// `pressed` is when the user asked, for the SPEC 10 switch budget.
    pub async fn play_at(
        &mut self,
        url: &str,
        headers: Option<&Headers>,
        title: &str,
        start: Option<f64>,
        pressed: Instant,
    ) -> Result<(), String> {
        self.cancel_reconnect();
        let alive = self
            .running
            .as_ref()
            .is_some_and(|r| r.titles.lock().unwrap().alive);
        if !alive {
            self.running = None;
            self.running = Some(self.spawn().await?);
        }
        // Before the load ends the dump, so the recording ends where the
        // switch was pressed.
        self.end_recording().await?;
        let mut msg = format!(
            "{}\n",
            json!({ "command": ["set_property", "force-media-title", title] })
        );
        msg.push_str(&self.loadfile(url, headers, title, false, start));
        self.write(&msg).await?;
        log::debug!(
            "loadfile sent {} ms after keypress",
            pressed.elapsed().as_millis()
        );
        Ok(())
    }

    /// The `loadfile` line for `url`, noting its title for errors and
    /// resetting [`Behind`]. Needs mpv running. A URL with `headers` is an
    /// M3U channel's, so live whatever its path.
    fn loadfile(
        &mut self,
        url: &str,
        headers: Option<&Headers>,
        title: &str,
        unpause: bool,
        start: Option<f64>,
    ) -> String {
        let request = self.next_request;
        self.next_request += 1;
        self.current = Some((url.to_string(), headers.cloned(), title.to_string()));
        let kind = if headers.is_some() {
            "live"
        } else {
            stream_kind(url)
        };
        self.load = request;
        let running = self.running.as_ref().unwrap();
        {
            let mut t = running.titles.lock().unwrap();
            t.by_request.insert(request, title.to_string());
            t.last = title.to_string();
            t.loaded = Some((Instant::now(), kind));
            t.load = request;
            t.played = false;
            t.seeking = false;
            t.retry = self.reconnect.is_some();
        }
        let mut behind = running.behind.lock().unwrap();
        behind.reload(unpause, Instant::now());
        let _ = self.events.send(Event::Behind(*behind));
        // mpv 0.38 and later: the index, then the options (SPEC 6.1).
        let command = match loadfile_options(start, headers) {
            Some(options) => json!(["loadfile", url, "replace", -1, options]),
            None => json!(["loadfile", url, "replace"]),
        };
        format!("{}\n", json!({ "command": command, "request_id": request }))
    }

    /// Starts recording what plays to `file`, from the time shown, or with
    /// `None` stops; nothing if mpv is not running.
    pub async fn record(&mut self, file: Option<&Path>) -> Result<(), String> {
        self.cancel_reconnect();
        let alive = self
            .running
            .as_ref()
            .is_some_and(|r| r.titles.lock().unwrap().alive);
        if !alive {
            return Ok(());
        }
        let Some(file) = file else {
            return self.end_recording().await;
        };
        let running = self.running.as_mut().unwrap();
        if running.recording.load(Ordering::SeqCst) {
            return Ok(());
        }
        // One that ended by itself (mpv failed it, the stream ended).
        running.dump = None;
        let _ = self.events.send(Event::Recording(file.to_path_buf()));
        let shown = self.request(json!(["get_property", "time-pos"])).await;
        let Some(shown) = shown.ok().and_then(|v| v.as_f64()) else {
            let error = "nothing is playing yet".to_string();
            let _ = self.events.send(Event::RecordingEnded(Some(error)));
            return Ok(());
        };
        log::info!("recording from {shown:.2} s");
        let (tx, done) = oneshot::channel();
        let request = self.next_request;
        self.next_request += 1;
        let running = self.running.as_mut().unwrap();
        running.replies.lock().unwrap().insert(request, tx);
        // Set before mpv can fail it, so the failure is reported.
        running.recording.store(true, Ordering::SeqCst);
        running.dump = Some(Dump {
            file: file.to_path_buf(),
            done,
        });
        // `no`: after what is cached, goes on appending what is read.
        let path = file.display().to_string();
        let cmd = json!({
            "command": ["dump-cache", shown, "no", path],
            "request_id": request,
            "async": true,
        });
        self.write(&format!("{cmd}\n")).await
    }

    /// Asks mpv for `time-pos`, the seconds shown into what plays, and
    /// reports it as [`Event::Position`]; nothing if mpv is not running or
    /// does not know yet. For catch-up, whose `duration` is meaningless for
    /// `.ts` (SPEC 6.1).
    pub async fn position(&mut self) {
        let alive = self
            .running
            .as_ref()
            .is_some_and(|r| r.titles.lock().unwrap().alive);
        if !alive {
            return;
        }
        let pos = self.request(json!(["get_property", "time-pos"])).await;
        if let Some(pos) = pos.ok().and_then(|v| v.as_f64()) {
            let _ = self.events.send(Event::Position(pos));
        }
    }

    /// Asks mpv for `duration`, the length of what plays, and reports it as
    /// [`Event::Length`]; nothing if mpv is not running or does not know
    /// yet. For a movie, whose info's length is unreliable (SPEC 6.1).
    pub async fn length(&mut self) {
        let alive = self
            .running
            .as_ref()
            .is_some_and(|r| r.titles.lock().unwrap().alive);
        if !alive {
            return;
        }
        let len = self.request(json!(["get_property", "duration"])).await;
        if let Some(len) = len.ok().and_then(|v| v.as_f64()) {
            let _ = self.events.send(Event::Length(len));
        }
    }

    /// Shows `text` on mpv's OSD for `ms` milliseconds; nothing if mpv is
    /// not running.
    pub async fn show(&mut self, text: &str, ms: u64) {
        let alive = self
            .running
            .as_ref()
            .is_some_and(|r| r.titles.lock().unwrap().alive);
        if alive {
            let _ = self.request(json!(["show-text", text, ms])).await;
        }
    }

    /// Ends the recording, if one runs: stops the dump where playback is,
    /// then cuts off what mpv had read past it. Nothing if it ended already
    /// (mpv failed it, or the stream ended).
    async fn end_recording(&mut self) -> Result<(), String> {
        let Some(running) = self.running.as_mut() else {
            return Ok(());
        };
        let Some(dump) = running.dump.take() else {
            return Ok(());
        };
        if !running.recording.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        // How far mpv has read ahead of the picture, all of it written.
        let shown = self.request(json!(["get_property", "time-pos"])).await;
        let cache = self
            .request(json!(["get_property", "demuxer-cache-state"]))
            .await;
        let ahead = match (shown, cache) {
            (Ok(shown), Ok(cache)) => shown
                .as_f64()
                .zip(cache["cache-end"].as_f64())
                .map(|(shown, end)| end - shown),
            _ => None,
        };
        if let Some(ahead) = ahead {
            log::info!("recording stopped, {ahead:.2} s read ahead to cut");
        }
        let stop = json!({ "command": ["dump-cache", 0, 0, ""] });
        let written = self.write(&format!("{stop}\n")).await;
        let closed = tokio::time::timeout(REPLY_WAIT, dump.done).await;
        let _ = self.events.send(Event::RecordingEnded(None));
        written?;
        match (closed, ahead) {
            (Ok(Ok(_)), Some(ahead)) if ahead > 0.0 => {
                let ahead = Duration::from_secs_f64(ahead);
                tokio::task::spawn_blocking(move || {
                    if let Err(e) = crate::ts::cut_tail(&dump.file, ahead) {
                        log::warn!("cutting {}: {e}", dump.file.display());
                    }
                });
            }
            (Ok(Ok(_)), _) => {}
            _ => log::warn!("mpv did not close {}", dump.file.display()),
        }
        Ok(())
    }

    /// Sends `command` and waits for mpv's answer: its `data`, or its error.
    async fn request(&mut self, command: Value) -> Result<Value, String> {
        let request = self.next_request;
        self.next_request += 1;
        let (tx, rx) = oneshot::channel();
        let running = self.running.as_ref().ok_or("mpv is not running")?;
        running.replies.lock().unwrap().insert(request, tx);
        let msg = json!({ "command": command, "request_id": request });
        self.write(&format!("{msg}\n")).await?;
        let reply = tokio::time::timeout(REPLY_WAIT, rx)
            .await
            .map_err(|_| "mpv did not answer".to_string())?
            .map_err(|_| "mpv exited".to_string())?;
        match reply["error"].as_str() {
            Some("success") => Ok(reply["data"].clone()),
            e => Err(e.unwrap_or("no answer").to_string()),
        }
    }

    /// Writes to mpv's IPC socket; a failure means mpv is gone.
    async fn write(&mut self, msg: &str) -> Result<(), String> {
        let running = self.running.as_mut().unwrap();
        if let Err(e) = running.ipc.write_all(msg.as_bytes()).await {
            self.running = None;
            return Err(format!("mpv IPC write failed: {e}"));
        }
        Ok(())
    }

    /// Sends a control to mpv; nothing if it is not running. A stop returns
    /// once mpv has gone, so a play right after spawns a new one rather than
    /// loading into the one closing.
    pub async fn control(&mut self, control: Control) -> Result<(), String> {
        let reconnecting = self.reconnect.is_some();
        self.cancel_reconnect();
        let alive = self
            .running
            .as_ref()
            .is_some_and(|r| r.titles.lock().unwrap().alive);
        if !alive {
            return Ok(());
        }
        if control == Control::Stop {
            self.end_recording().await?;
        }
        let command = match control {
            Control::Pause => json!(["cycle", "pause"]),
            Control::Mute => json!(["cycle", "mute"]),
            Control::Fullscreen => json!(["cycle", "fullscreen"]),
            Control::Stop => json!(["quit"]),
            Control::Restart => json!(["seek", 0, "absolute"]),
            Control::Live => return self.go_live(reconnecting).await,
        };
        let msg = format!("{}\n", json!({ "command": command }));
        self.write(&msg).await?;
        if control == Control::Stop {
            let running = self.running.as_mut().unwrap();
            if tokio::time::timeout(QUIT_WAIT, &mut running.reader)
                .await
                .is_err()
            {
                log::warn!(
                    "mpv did not quit within {} s; killing it",
                    QUIT_WAIT.as_secs()
                );
            }
            self.running = None;
        }
        Ok(())
    }

    /// Reloads the stream playing and unpauses: streams have no live edge to
    /// seek to, so this is the only way back. Nothing while at live, unless
    /// it was `reconnecting` after a drop: then it retries at once.
    async fn go_live(&mut self, reconnecting: bool) -> Result<(), String> {
        let behind = self
            .running
            .as_ref()
            .unwrap()
            .behind
            .lock()
            .unwrap()
            .at(Instant::now());
        let Some((url, headers, title)) = self
            .current
            .clone()
            .filter(|_| reconnecting || !behind.is_zero())
        else {
            return Ok(());
        };
        self.reload(&url, headers.as_ref(), &title).await
    }

    /// Loads `url` again at the live edge, unpaused. Needs mpv running.
    async fn reload(
        &mut self,
        url: &str,
        headers: Option<&Headers>,
        title: &str,
    ) -> Result<(), String> {
        let mut msg = self.loadfile(url, headers, title, true, None);
        msg.push_str(&format!(
            "{}\n",
            json!({ "command": ["set_property", "pause", false] })
        ));
        self.write(&msg).await
    }

    async fn spawn(&self) -> Result<Running, String> {
        let socket = &self.launch.socket;
        if let Some(dir) = socket.parent() {
            use std::os::unix::fs::DirBuilderExt;
            let _ = std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir);
        }
        let _ = std::fs::remove_file(socket);
        log::info!("starting {}", self.launch.program);
        let mut cmd = Command::new(&self.launch.program);
        cmd.arg("--idle=yes")
            .arg("--force-window=yes")
            .arg(format!("--input-ipc-server={}", socket.display()))
            .arg("--wayland-app-id=vidicon-mpv")
            // mpv's default title, pinned so an `mpv.conf` title using
            // `${path}` or `${filename}` cannot show the URL.
            .arg(TITLE)
            .args(RECONNECT)
            .args(&self.launch.extra_args)
            // mpv's terminal output prints the stream URL, so it is discarded.
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // Also covers the app dying without cleanup (Ctrl-C, a crash). The
        // signal follows the spawning thread, a tokio worker, which lives as
        // long as the runtime.
        // SAFETY: prctl is async-signal-safe and touches no shared state.
        unsafe {
            cmd.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", self.launch.program))?;
        for _ in 0..250 {
            if let Ok(stream) = UnixStream::connect(socket).await {
                let (read, mut ipc) = stream.into_split();
                ipc.write_all((keybinds() + &observes() + LOG_MESSAGES).as_bytes())
                    .await
                    .map_err(|e| format!("mpv IPC write failed: {e}"))?;
                let titles = Arc::new(Mutex::new(Titles {
                    alive: true,
                    ..Titles::default()
                }));
                let behind = Arc::default();
                let recording = Arc::new(AtomicBool::new(false));
                let replies = Replies::default();
                let _ = self.events.send(Event::Started);
                let reader = tokio::spawn(read_events(
                    read,
                    titles.clone(),
                    Arc::clone(&behind),
                    Arc::clone(&recording),
                    Arc::clone(&replies),
                    self.events.clone(),
                    self.signal_tx.clone(),
                ));
                let (kill, killed) = oneshot::channel();
                tokio::spawn(watch(child, killed));
                return Ok(Running {
                    _kill: kill,
                    ipc,
                    titles,
                    behind,
                    reader,
                    recording,
                    dump: None,
                    replies,
                });
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("mpv exited during startup ({status})"));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Err("mpv did not open its IPC socket within 5 s".into())
    }
}

/// Owns the mpv process: reaps it as soon as it exits, so no zombie is left
/// while the app runs, and kills it when `killed` fires or its sender is
/// dropped. If the runtime shuts down first, dropping the child kills it
/// (`kill_on_drop`).
async fn watch(mut child: Child, killed: oneshot::Receiver<()>) {
    tokio::select! {
        status = child.wait() => log::debug!("mpv exit status: {status:?}"),
        _ = killed => {
            let _ = child.kill().await;
        }
    }
}

/// Reads mpv's replies and events until the socket closes (mpv exited).
async fn read_events(
    read: OwnedReadHalf,
    titles: Arc<Mutex<Titles>>,
    behind: Arc<Mutex<Behind>>,
    recording: Arc<AtomicBool>,
    replies: Replies,
    events: mpsc::UnboundedSender<Event>,
    signals: mpsc::UnboundedSender<(i64, Signal)>,
) {
    let ended = |error: Option<String>| {
        if recording.swap(false, Ordering::SeqCst) {
            let _ = events.send(Event::RecordingEnded(error));
        }
    };
    // The last unpause, for the drop log.
    let mut resumed = None;
    // The recorder's last error, the reason when the dump stops.
    let mut recorder_error = None;
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let waiting = msg["request_id"]
            .as_i64()
            .and_then(|r| replies.lock().unwrap().remove(&r));
        if let Some(tx) = waiting {
            let _ = tx.send(msg);
            continue;
        }
        if msg["event"] == "log-message" {
            // Only the recorder's text: others can quote the stream URL. It
            // goes on after some (`Failed writing packet.` when mpv
            // reconnects), so the recording ends when the dump stops.
            let text = msg["text"].as_str().unwrap_or_default().trim();
            if msg["prefix"] == "recorder" {
                recorder_error = Some(text.to_string());
            } else if msg["prefix"] == "cplayer" && text == "Cache dumping stopped due to error." {
                let reason = recorder_error.take().unwrap_or("recording failed".into());
                ended(Some(reason));
            }
            continue;
        }
        if msg["event"] == "end-file" && matches!(msg["reason"].as_str(), Some("eof" | "error")) {
            let t = titles.lock().unwrap();
            let entry = msg["playlist_entry_id"].as_i64();
            // Not one replaced since: no `loadfile` waiting, and the latest.
            let latest = t.by_request.is_empty() && t.by_entry.back().map(|(e, _)| *e) == entry;
            if latest && let Some((at, kind)) = t.loaded {
                let now = Instant::now();
                let behind = behind.lock().unwrap();
                let pause = if behind.paused() {
                    ", paused now".to_string()
                } else {
                    resumed
                        .filter(|r| *r >= at)
                        .map_or(String::new(), |r: Instant| {
                            let ago = now.saturating_duration_since(r).as_secs_f64();
                            format!(", resumed {ago:.0} s before")
                        })
                };
                log::debug!(
                    "end-file {kind}: {} ({}) {:.0} s after loadfile, paused {:.0} s of it{pause}",
                    msg["reason"].as_str().unwrap_or_default(),
                    msg["file_error"].as_str().unwrap_or("no error"),
                    now.saturating_duration_since(at).as_secs_f64(),
                    behind.at(now).as_secs_f64(),
                );
            }
            let eof = msg["reason"] == "eof";
            // A live channel never ends on its own. Failing to start is
            // reported as such, unless a retry failed (stream recovery
            // ticket 02).
            let live = t.loaded.is_some_and(|(_, kind)| kind == "live");
            let dropped = latest && live && (eof || t.played || t.retry);
            if dropped {
                let reason = msg["file_error"].as_str().unwrap_or("the stream ended");
                let _ = signals.send((t.load, Signal::Dropped(reason.to_string())));
            } else if latest && eof {
                let _ = events.send(Event::Ended);
            }
            drop(t);
            ended(None);
            if dropped {
                continue;
            }
        }
        if msg["event"] == "seek" {
            titles.lock().unwrap().seeking = true;
            continue;
        }
        if msg["event"] == "playback-restart" {
            let mut t = titles.lock().unwrap();
            // An older file's: a `loadfile` waits for its reply.
            if !t.by_request.is_empty() {
                continue;
            }
            let (played, seeking) = (t.played, t.seeking);
            (t.played, t.seeking) = (true, false);
            let live = t.loaded.is_some_and(|(_, kind)| kind == "live");
            if !played && t.retry {
                let _ = signals.send((t.load, Signal::Back));
            } else if played && !seeking && live {
                // mpv reconnected by itself, at the live edge (SPEC 9.3).
                log::debug!("live channel reconnected by mpv");
                let mut behind = behind.lock().unwrap();
                behind.reload(false, Instant::now());
                let _ = events.send(Event::Behind(*behind));
            }
            continue;
        }
        if msg["event"] == "client-message" {
            if let Some(r) = remote(&msg["args"]) {
                let _ = events.send(Event::Remote(r));
            }
            continue;
        }
        if msg["event"] == "property-change" {
            if let Some(e) = property(&msg) {
                let paused = match e {
                    Event::Paused(on) => Some(on),
                    _ => None,
                };
                let _ = events.send(e);
                if let Some(on) = paused {
                    // Sent under the lock, so it cannot overtake a reload's.
                    let mut behind = behind.lock().unwrap();
                    // Only an unpause: mpv reports `false` at spawn too.
                    if !on && behind.paused() {
                        resumed = Some(Instant::now());
                    }
                    behind.pause(on, Instant::now());
                    let _ = events.send(Event::Behind(*behind));
                }
            }
            continue;
        }
        let mut t = titles.lock().unwrap();
        if let Some(request) = msg["request_id"].as_i64() {
            // Reply to a `loadfile`: remember which entry it created.
            let title = t.by_request.remove(&request);
            if let (Some(title), Some(entry)) = (title, msg["data"]["playlist_entry_id"].as_i64()) {
                if t.by_entry.len() == ENTRIES_KEPT {
                    t.by_entry.pop_front();
                }
                t.by_entry.push_back((entry, title));
            }
        } else if msg["event"] == "end-file" && msg["reason"] == "error" {
            let entry = msg["playlist_entry_id"].as_i64();
            let title = t
                .by_entry
                .iter()
                .find(|(e, _)| Some(*e) == entry)
                .map_or_else(|| t.last.clone(), |(_, title)| title.clone());
            let reason = msg["file_error"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string();
            let _ = events.send(Event::Failed { title, reason });
        }
    }
    titles.lock().unwrap().alive = false;
    // Dropped, so whatever waits on them sees mpv gone.
    replies.lock().unwrap().clear();
    ended(None);
    log::info!("mpv exited");
    let _ = events.send(Event::Stopped);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use tokio::net::UnixListener;
    use tokio::net::unix::OwnedWriteHalf as ServerWrite;

    /// A fake mpv: records its arguments and pid, then sleeps. The test plays
    /// mpv's side of the IPC socket.
    struct Fake {
        dir: tempfile::TempDir,
        player: Player,
        events: mpsc::UnboundedReceiver<Event>,
    }

    impl Fake {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let script = dir.path().join("mpv");
            std::fs::write(
                &script,
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\necho $$ >> \"$0.starts\"\nexec sleep 30\n",
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
            let (tx, events) = mpsc::unbounded_channel();
            let player = Player::new(
                Launch {
                    program: script.display().to_string(),
                    extra_args: vec!["--mute=yes".into()],
                    socket: dir.path().join("run/mpv.sock"),
                },
                tx,
            );
            Self {
                dir,
                player,
                events,
            }
        }

        fn path(&self, name: &str) -> std::path::PathBuf {
            self.dir.path().join(name)
        }

        /// The most recently started fake's pid.
        fn pid(&self) -> i32 {
            let starts = std::fs::read_to_string(self.path("mpv.starts")).unwrap();
            starts.lines().last().unwrap().parse().unwrap()
        }

        fn starts(&self) -> usize {
            std::fs::read_to_string(self.path("mpv.starts")).map_or(0, |s| s.lines().count())
        }

        /// Plays, serving the IPC socket once the fake has started. Returns
        /// the server side of the new connection.
        async fn play_spawning(&mut self, url: &str, title: &str) -> (Lines, ServerWrite) {
            let starts = self.starts();
            let socket = self.player.launch.socket.clone();
            let marker = self.path("mpv.starts");
            let serve = async {
                while std::fs::read_to_string(&marker).map_or(0, |s| s.lines().count()) == starts {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                let listener = UnixListener::bind(&socket).unwrap();
                let (stream, _) = listener.accept().await.unwrap();
                let (read, write) = stream.into_split();
                (BufReader::new(read).lines(), write)
            };
            let (played, (mut lines, write)) =
                tokio::join!(self.player.play(url, title, Instant::now()), serve);
            played.unwrap();
            // Every spawn binds the app's keys and observes the controls'
            // properties before anything else.
            for (key, arg) in KEYS {
                let cmd = next_json(&mut lines).await["command"].clone();
                assert_eq!(
                    cmd,
                    json!(["keybind", key, format!("script-message vidicon {arg}")])
                );
            }
            for (id, prop) in [(1, "pause"), (2, "mute"), (3, "fullscreen")] {
                let cmd = next_json(&mut lines).await["command"].clone();
                assert_eq!(cmd, json!(["observe_property", id, prop]));
            }
            assert_eq!(
                next_json(&mut lines).await["command"],
                json!(["request_log_messages", "error"])
            );
            assert_eq!(wait_event(&mut self.events).await, Event::Started);
            (lines, write)
        }
    }

    type Lines = tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>;

    async fn next_json(lines: &mut Lines) -> Value {
        serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap()
    }

    /// The next event other than [`Event::Behind`].
    async fn wait_event(events: &mut mpsc::UnboundedReceiver<Event>) -> Event {
        loop {
            match wait_any(events).await {
                Event::Behind(_) => {}
                e => return e,
            }
        }
    }

    async fn wait_any(events: &mut mpsc::UnboundedReceiver<Event>) -> Event {
        tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap()
    }

    /// The next [`Event::Behind`], skipping others.
    async fn wait_behind(events: &mut mpsc::UnboundedReceiver<Event>) -> Behind {
        loop {
            if let Event::Behind(b) = wait_any(events).await {
                return b;
            }
        }
    }

    /// An event already queued, other than [`Event::Behind`].
    fn queued_event(events: &mut mpsc::UnboundedReceiver<Event>) -> Option<Event> {
        std::iter::from_fn(|| events.try_recv().ok()).find(|e| !matches!(e, Event::Behind(_)))
    }

    #[tokio::test]
    async fn spawns_once_with_spec_args_and_title_before_url() {
        let mut f = Fake::new();
        let (mut lines, _w) = f
            .play_spawning("http://h/live/u/p/1.ts", "101 Harbour One")
            .await;
        let args = std::fs::read_to_string(f.path("mpv.args")).unwrap();
        let socket = f.player.launch.socket.display().to_string();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            [
                "--idle=yes",
                "--force-window=yes",
                &format!("--input-ipc-server={socket}"),
                "--wayland-app-id=vidicon-mpv",
                TITLE,
                "--stream-lavf-o-append=reconnect=1",
                "--stream-lavf-o-append=reconnect_streamed=1",
                "--stream-lavf-o-append=reconnect_delay_max=10",
                "--mute=yes",
            ]
        );
        let title = next_json(&mut lines).await;
        assert_eq!(
            title["command"],
            json!(["set_property", "force-media-title", "101 Harbour One"])
        );
        let load = next_json(&mut lines).await;
        assert_eq!(
            load["command"],
            json!(["loadfile", "http://h/live/u/p/1.ts", "replace"])
        );
        assert_socket_private(Path::new(&socket));
    }

    fn assert_socket_private(socket: &Path) {
        let mode = std::fs::metadata(socket.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[tokio::test]
    async fn switching_reuses_the_running_mpv() {
        let mut f = Fake::new();
        let (mut lines, _w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        next_json(&mut lines).await;
        f.player
            .play("http://h/2.ts", "2 Two", Instant::now())
            .await
            .unwrap();
        assert_eq!(next_json(&mut lines).await["command"][2], "2 Two");
        assert_eq!(
            next_json(&mut lines).await["command"],
            json!(["loadfile", "http://h/2.ts", "replace"])
        );
        assert_eq!(f.starts(), 1);
    }

    /// A movie resuming loads from its position (VOD ticket 07); `S`
    /// seeks back to the start.
    #[tokio::test]
    async fn resuming_loads_from_the_position() {
        let mut f = Fake::new();
        let (mut lines, _w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        next_json(&mut lines).await;
        f.player
            .play_at(
                "http://h/m.mkv",
                None,
                "Quiet Shore",
                Some(600.0),
                Instant::now(),
            )
            .await
            .unwrap();
        next_json(&mut lines).await;
        assert_eq!(
            next_json(&mut lines).await["command"],
            json!(["loadfile", "http://h/m.mkv", "replace", -1, "start=600"])
        );
        f.player.control(Control::Restart).await.unwrap();
        assert_eq!(
            next_json(&mut lines).await["command"],
            json!(["seek", 0, "absolute"])
        );
    }

    #[test]
    fn stream_kind_reads_the_path_only() {
        assert_eq!(stream_kind("http://h/live/u/p/7.ts"), "live");
        assert_eq!(
            stream_kind("http://h/timeshift/u/p/60/2026-10-01:20-00/7.ts"),
            "catch-up"
        );
        assert_eq!(
            stream_kind("http://h/streaming/timeshift.php?stream=7"),
            "catch-up"
        );
        assert_eq!(stream_kind("http://h/movie/u/p/7.mkv"), "movie");
        assert_eq!(stream_kind("http://h/series/u/p/7.mp4"), "episode");
        assert_eq!(stream_kind("http://h/x.ts?next=/live/"), "other");
    }

    #[tokio::test]
    async fn end_file_error_names_the_failed_channel() {
        let mut f = Fake::new();
        let (mut lines, mut w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        let first = next_json(&mut lines).await["request_id"].as_i64().unwrap();
        f.player
            .play("http://h/2.ts", "2 Two", Instant::now())
            .await
            .unwrap();
        next_json(&mut lines).await;
        let second = next_json(&mut lines).await["request_id"].as_i64().unwrap();
        // mpv replies with the playlist entries, then the first one fails late.
        let reply = |r: i64, e: i64| {
            format!(
                "{{\"request_id\":{r},\"error\":\"success\",\"data\":{{\"playlist_entry_id\":{e}}}}}\n"
            )
        };
        w.write_all(reply(first, 7).as_bytes()).await.unwrap();
        w.write_all(reply(second, 8).as_bytes()).await.unwrap();
        w.write_all(b"{\"event\":\"end-file\",\"reason\":\"stop\",\"playlist_entry_id\":7}\n")
            .await
            .unwrap();
        w.write_all(b"{\"event\":\"end-file\",\"reason\":\"error\",\"playlist_entry_id\":7,\"file_error\":\"loading failed\"}\n")
            .await
            .unwrap();
        assert_eq!(
            wait_event(&mut f.events).await,
            Event::Failed {
                title: "1 One".into(),
                reason: "loading failed".into()
            }
        );
    }

    #[tokio::test]
    async fn only_the_latest_file_running_out_is_reported() {
        let mut f = Fake::new();
        let (mut lines, mut w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        let first = next_json(&mut lines).await["request_id"].as_i64().unwrap();
        f.player
            .play("http://h/2.ts", "2 Two", Instant::now())
            .await
            .unwrap();
        next_json(&mut lines).await;
        let second = next_json(&mut lines).await["request_id"].as_i64().unwrap();
        let reply = |r: i64, e: i64| {
            format!(
                "{{\"request_id\":{r},\"error\":\"success\",\"data\":{{\"playlist_entry_id\":{e}}}}}\n"
            )
        };
        w.write_all(reply(first, 7).as_bytes()).await.unwrap();
        // The first ran out while the second was loading: not reported.
        w.write_all(b"{\"event\":\"end-file\",\"reason\":\"eof\",\"playlist_entry_id\":7}\n")
            .await
            .unwrap();
        w.write_all(reply(second, 8).as_bytes()).await.unwrap();
        // Replaced: not reported either.
        w.write_all(b"{\"event\":\"end-file\",\"reason\":\"stop\",\"playlist_entry_id\":7}\n")
            .await
            .unwrap();
        w.write_all(b"{\"event\":\"end-file\",\"reason\":\"eof\",\"playlist_entry_id\":8}\n")
            .await
            .unwrap();
        assert_eq!(wait_event(&mut f.events).await, Event::Ended);
        assert_eq!(queued_event(&mut f.events), None, "reported once");
    }

    #[tokio::test]
    async fn bound_keys_come_back_as_remote_events() {
        let mut f = Fake::new();
        let (_lines, mut w) = f.play_spawning("http://h/1.ts", "1 One").await;
        for (args, expected) in [
            (r#"["vidicon","next"]"#, Some(Remote::Next)),
            (r#"["vidicon","prev"]"#, Some(Remote::Prev)),
            (r#"["vidicon","digit","7"]"#, Some(Remote::Digit(7))),
            (r#"["vidicon","back"]"#, Some(Remote::Back)),
            (r#"["vidicon","live"]"#, Some(Remote::Live)),
            (r#"["vidicon","record"]"#, Some(Remote::Record)),
            (r#"["vidicon","start-over"]"#, Some(Remote::StartOver)),
            // Another script's message, and malformed ones: ignored.
            (r#"["other","next"]"#, None),
            (r#"["vidicon","digit","12"]"#, None),
            (r#"["vidicon"]"#, None),
        ] {
            let line = format!("{{\"event\":\"client-message\",\"args\":{args}}}\n");
            w.write_all(line.as_bytes()).await.unwrap();
            if let Some(r) = expected {
                assert_eq!(wait_event(&mut f.events).await, Event::Remote(r), "{args}");
            }
        }
        // Nothing queued from the ignored ones.
        w.write_all(b"{\"event\":\"client-message\",\"args\":[\"vidicon\",\"next\"]}\n")
            .await
            .unwrap();
        assert_eq!(wait_event(&mut f.events).await, Event::Remote(Remote::Next));
    }

    #[tokio::test]
    async fn observed_property_changes_become_events() {
        let mut f = Fake::new();
        let (_lines, mut w) = f.play_spawning("http://h/1.ts", "1 One").await;
        for (line, expected) in [
            (
                r#"{"event":"property-change","id":1,"name":"pause","data":true}"#,
                Some(Event::Paused(true)),
            ),
            (
                r#"{"event":"property-change","id":2,"name":"mute","data":false}"#,
                Some(Event::Muted(false)),
            ),
            (
                r#"{"event":"property-change","id":3,"name":"fullscreen","data":true}"#,
                Some(Event::Fullscreen(true)),
            ),
            (
                r#"{"event":"property-change","id":1,"name":"pause","data":false}"#,
                Some(Event::Paused(false)),
            ),
            // No value yet, and properties not ours: ignored.
            (
                r#"{"event":"property-change","id":3,"name":"fullscreen"}"#,
                None,
            ),
            (
                r#"{"event":"property-change","id":9,"name":"volume","data":50}"#,
                None,
            ),
        ] {
            w.write_all(format!("{line}\n").as_bytes()).await.unwrap();
            if let Some(e) = expected {
                assert_eq!(wait_event(&mut f.events).await, e, "{line}");
            }
        }
        w.write_all(b"{\"event\":\"property-change\",\"id\":2,\"name\":\"mute\",\"data\":true}\n")
            .await
            .unwrap();
        assert_eq!(wait_event(&mut f.events).await, Event::Muted(true));
    }

    #[tokio::test]
    async fn controls_send_cycle_commands() {
        let mut f = Fake::new();
        // Nothing running: nothing is sent and nothing spawns.
        f.player.control(Control::Pause).await.unwrap();
        assert_eq!(f.starts(), 0);

        let (mut lines, _w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        next_json(&mut lines).await;
        for (control, prop) in [
            (Control::Pause, "pause"),
            (Control::Mute, "mute"),
            (Control::Fullscreen, "fullscreen"),
        ] {
            f.player.control(control).await.unwrap();
            assert_eq!(
                next_json(&mut lines).await["command"],
                json!(["cycle", prop])
            );
        }
    }

    #[tokio::test]
    async fn stop_quits_mpv_and_reports_stopped() {
        let mut f = Fake::new();
        let (mut lines, w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        next_json(&mut lines).await;
        let pid = f.pid();
        // mpv's side: quit arrives, then the socket closes as mpv exits.
        let mpv = async {
            assert_eq!(next_json(&mut lines).await["command"], json!(["quit"]));
            drop((lines, w));
        };
        let (stopped, ()) = tokio::join!(f.player.control(Control::Stop), mpv);
        stopped.unwrap();
        // Reported before `control` returns, so ahead of any later play.
        assert_eq!(queued_event(&mut f.events), Some(Event::Stopped));
        assert_reaped(pid).await;
        let (mut lines, _w) = f.play_spawning("http://h/2.ts", "2 Two").await;
        assert_eq!(f.starts(), 2);
        assert_eq!(next_json(&mut lines).await["command"][2], "2 Two");
    }

    #[tokio::test]
    async fn exit_reports_stopped_and_next_play_respawns() {
        let mut f = Fake::new();
        let (lines, w) = f.play_spawning("http://h/1.ts", "1 One").await;
        // mpv exiting: the process ends and its socket closes.
        let pid = f.pid();
        unsafe { libc::kill(pid, libc::SIGTERM) };
        drop((lines, w));
        assert_eq!(wait_event(&mut f.events).await, Event::Stopped);
        assert_reaped(pid).await;
        let (mut lines, _w) = f.play_spawning("http://h/2.ts", "2 Two").await;
        assert_eq!(f.starts(), 2);
        assert_eq!(next_json(&mut lines).await["command"][2], "2 Two");
    }

    /// A saved `mpv_path` or `extra_mpv_args` is used from the next spawn;
    /// the mpv playing keeps going.
    #[tokio::test]
    async fn a_new_launch_applies_from_the_next_spawn() {
        let mut f = Fake::new();
        let (lines, w) = f.play_spawning("http://h/1.ts", "1 One").await;
        let launch = Launch {
            program: f.player.launch.program.clone(),
            extra_args: vec!["--volume=50".into()],
            socket: f.player.launch.socket.clone(),
        };
        f.player.set_launch(launch);
        let pid = f.pid();
        assert!(Path::new("/proc").join(pid.to_string()).exists());
        assert_eq!(f.starts(), 1);
        unsafe { libc::kill(pid, libc::SIGTERM) };
        drop((lines, w));
        assert_eq!(wait_event(&mut f.events).await, Event::Stopped);
        let _server = f.play_spawning("http://h/2.ts", "2 Two").await;
        let args = std::fs::read_to_string(f.path("mpv.args")).unwrap();
        assert_eq!(args.lines().last(), Some("--volume=50"));
        assert!(!args.contains("--mute=yes"), "{args}");
    }

    #[tokio::test]
    async fn dropping_the_player_kills_mpv() {
        let mut f = Fake::new();
        let _server = f.play_spawning("http://h/1.ts", "1 One").await;
        let pid = f.pid();
        assert!(Path::new("/proc").join(pid.to_string()).exists());
        drop(f.player);
        assert_reaped(pid).await;
    }

    #[test]
    fn behind_live_counts_time_paused_since_the_last_load() {
        let s = Duration::from_secs;
        let t0 = Instant::now();
        let mut b = Behind::default();
        assert_eq!(b.at(t0 + s(100)), s(0), "playing does not count");

        b.pause(true, t0 + s(10));
        assert!(b.paused());
        assert_eq!(b.at(t0 + s(40)), s(30), "grows while paused");
        b.pause(false, t0 + s(70));
        assert_eq!(b.at(t0 + s(500)), s(60), "pause and resume sum");

        // mpv repeats a value sometimes; that is not a new pause.
        b.pause(false, t0 + s(80));
        b.pause(true, t0 + s(100));
        b.pause(true, t0 + s(110));
        b.pause(false, t0 + s(145));
        assert_eq!(b.at(t0 + s(900)), s(105), "pausing twice adds up");

        // A load while playing: at live.
        b.reload(false, t0 + s(200));
        assert_eq!(b.at(t0 + s(300)), s(0));

        // A load while paused stays paused, counting from the load.
        b.pause(true, t0 + s(300));
        b.reload(false, t0 + s(320));
        assert!(b.paused());
        assert_eq!(b.at(t0 + s(350)), s(30));

        // Going live unpauses: its pause change adds nothing.
        b.reload(true, t0 + s(400));
        b.pause(false, t0 + s(401));
        assert_eq!(b.at(t0 + s(500)), s(0));
    }

    #[tokio::test]
    async fn live_reloads_the_channel_unpaused_when_behind() {
        let mut f = Fake::new();
        // No Player: nothing is sent and nothing spawns.
        f.player.control(Control::Live).await.unwrap();
        assert_eq!(f.starts(), 0);

        let (mut lines, mut w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        next_json(&mut lines).await;
        assert_eq!(
            wait_behind(&mut f.events).await.at(Instant::now()),
            Duration::ZERO
        );

        // At live: nothing sent. The next line mpv sees is the pause below.
        f.player.control(Control::Live).await.unwrap();
        f.player.control(Control::Pause).await.unwrap();
        assert_eq!(
            next_json(&mut lines).await["command"],
            json!(["cycle", "pause"])
        );

        let pause = |on: bool| {
            format!("{{\"event\":\"property-change\",\"id\":1,\"name\":\"pause\",\"data\":{on}}}\n")
        };
        w.write_all(pause(true).as_bytes()).await.unwrap();
        let behind = wait_behind(&mut f.events).await;
        assert!(behind.paused());
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!behind.at(Instant::now()).is_zero());

        f.player.control(Control::Live).await.unwrap();
        let load = next_json(&mut lines).await;
        assert_eq!(
            load["command"],
            json!(["loadfile", "http://h/1.ts", "replace"])
        );
        assert!(load["request_id"].is_i64(), "errors still name the channel");
        assert_eq!(
            next_json(&mut lines).await["command"],
            json!(["set_property", "pause", false])
        );
        assert_eq!(wait_behind(&mut f.events).await, Behind::default());
        // mpv reports the unpause; still at live.
        w.write_all(pause(false).as_bytes()).await.unwrap();
        assert_eq!(wait_behind(&mut f.events).await, Behind::default());

        // At live again: `L` does nothing.
        f.player.control(Control::Live).await.unwrap();
        f.player.control(Control::Mute).await.unwrap();
        assert_eq!(
            next_json(&mut lines).await["command"],
            json!(["cycle", "mute"])
        );
    }

    #[tokio::test]
    async fn a_channel_switch_resets_behind_live() {
        let mut f = Fake::new();
        let (mut lines, mut w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        next_json(&mut lines).await;
        wait_behind(&mut f.events).await;
        w.write_all(b"{\"event\":\"property-change\",\"id\":1,\"name\":\"pause\",\"data\":true}\n")
            .await
            .unwrap();
        wait_behind(&mut f.events).await;
        w.write_all(
            b"{\"event\":\"property-change\",\"id\":1,\"name\":\"pause\",\"data\":false}\n",
        )
        .await
        .unwrap();
        assert!(
            !wait_behind(&mut f.events)
                .await
                .at(Instant::now())
                .is_zero()
        );
        f.player
            .play("http://h/2.ts", "2 Two", Instant::now())
            .await
            .unwrap();
        assert_eq!(wait_behind(&mut f.events).await, Behind::default());
        // `L` now does nothing: the next line after the switch is the mute.
        next_json(&mut lines).await;
        next_json(&mut lines).await;
        f.player.control(Control::Live).await.unwrap();
        f.player.control(Control::Mute).await.unwrap();
        assert_eq!(
            next_json(&mut lines).await["command"],
            json!(["cycle", "mute"])
        );
    }

    /// The next recording event, skipping others.
    async fn wait_recording(events: &mut mpsc::UnboundedReceiver<Event>) -> Event {
        loop {
            if let e @ (Event::Recording(_) | Event::RecordingEnded(_)) = wait_any(events).await {
                return e;
            }
        }
    }

    /// mpv's side of a request: reads it and answers with `data`, returning
    /// its command.
    async fn answer(lines: &mut Lines, w: &mut ServerWrite, data: Value) -> Value {
        let msg = next_json(lines).await;
        let reply = json!({ "request_id": msg["request_id"], "error": "success", "data": data });
        w.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
        msg["command"].clone()
    }

    /// Starts recording to `file` with the picture at 20 s; returns the
    /// dump's request id.
    async fn start_recording(
        f: &mut Fake,
        lines: &mut Lines,
        w: &mut ServerWrite,
        file: &Path,
    ) -> i64 {
        let mpv = async {
            let asked = answer(lines, w, json!(20.0)).await;
            assert_eq!(asked, json!(["get_property", "time-pos"]));
            let dump = next_json(lines).await;
            assert_eq!(
                dump["command"],
                json!(["dump-cache", 20.0, "no", file.display().to_string()])
            );
            assert_eq!(dump["async"], true, "it runs until stopped");
            dump["request_id"].as_i64().unwrap()
        };
        let (started, dump) = tokio::join!(f.player.record(Some(file)), mpv);
        started.unwrap();
        assert_eq!(
            wait_recording(&mut f.events).await,
            Event::Recording(file.to_path_buf())
        );
        dump
    }

    /// mpv's side of a stop with the picture at 20 s and 10 s read ahead:
    /// answers where it is, then closes the dump.
    async fn stop_recording(lines: &mut Lines, w: &mut ServerWrite, dump: i64) {
        assert_eq!(
            answer(lines, w, json!(20.0)).await,
            json!(["get_property", "time-pos"])
        );
        let cache = json!({ "cache-end": 30.0, "reader-pts": 20.3 });
        assert_eq!(
            answer(lines, w, cache).await,
            json!(["get_property", "demuxer-cache-state"])
        );
        assert_eq!(
            answer(lines, w, Value::Null).await,
            json!(["dump-cache", 0, 0, ""])
        );
        let closed = json!({ "request_id": dump, "error": "success", "data": null });
        w.write_all(format!("{closed}\n").as_bytes()).await.unwrap();
    }

    async fn wait_len(file: &Path, len: u64) {
        for _ in 0..200 {
            if std::fs::metadata(file).unwrap().len() == len {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{} never became {len} bytes", file.display());
    }

    #[tokio::test]
    async fn a_recording_runs_from_the_time_shown_and_stops_where_shown() {
        let mut f = Fake::new();
        // Nothing running: nothing is sent and nothing spawns.
        f.player.record(Some(Path::new("/r/a.ts"))).await.unwrap();
        assert_eq!(f.starts(), 0);

        let (mut lines, mut w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        next_json(&mut lines).await;
        // 30 s of frames from 1 s; with 10 s read ahead of the picture at the
        // stop, the file is cut 10 s before its last frame.
        let (file, video_at) = crate::ts::tests::recording();
        let dump = start_recording(&mut f, &mut lines, &mut w, file.path()).await;
        let (stopped, ()) = tokio::join!(
            f.player.record(None),
            stop_recording(&mut lines, &mut w, dump)
        );
        stopped.unwrap();
        assert_eq!(
            wait_recording(&mut f.events).await,
            Event::RecordingEnded(None)
        );
        wait_len(file.path(), video_at[499]).await;

        // Not recording: stopping sends nothing, and a switch only loads.
        f.player.record(None).await.unwrap();
        f.player
            .play("http://h/2.ts", "2 Two", Instant::now())
            .await
            .unwrap();
        assert_eq!(
            next_json(&mut lines).await["command"][1],
            "force-media-title"
        );
    }

    #[tokio::test]
    async fn a_channel_switch_stops_the_recording_before_loading() {
        let mut f = Fake::new();
        let (mut lines, mut w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        next_json(&mut lines).await;
        let (file, video_at) = crate::ts::tests::recording();
        let dump = start_recording(&mut f, &mut lines, &mut w, file.path()).await;
        let mpv = async {
            stop_recording(&mut lines, &mut w, dump).await;
            assert_eq!(
                next_json(&mut lines).await["command"][1],
                "force-media-title"
            );
            assert_eq!(
                next_json(&mut lines).await["command"],
                json!(["loadfile", "http://h/2.ts", "replace"])
            );
        };
        let (played, ()) =
            tokio::join!(f.player.play("http://h/2.ts", "2 Two", Instant::now()), mpv);
        played.unwrap();
        assert_eq!(
            wait_recording(&mut f.events).await,
            Event::RecordingEnded(None)
        );
        wait_len(file.path(), video_at[499]).await;
    }

    #[tokio::test]
    async fn stop_ends_the_recording_before_quitting() {
        let mut f = Fake::new();
        let (mut lines, mut w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        next_json(&mut lines).await;
        let (file, _) = crate::ts::tests::recording();
        let dump = start_recording(&mut f, &mut lines, &mut w, file.path()).await;
        let mpv = async {
            stop_recording(&mut lines, &mut w, dump).await;
            assert_eq!(next_json(&mut lines).await["command"], json!(["quit"]));
            drop((lines, w));
        };
        let (stopped, ()) = tokio::join!(f.player.control(Control::Stop), mpv);
        stopped.unwrap();
        assert_eq!(
            wait_recording(&mut f.events).await,
            Event::RecordingEnded(None)
        );
    }

    #[tokio::test]
    async fn position_and_length_report_time_pos_and_duration() {
        let mut f = Fake::new();
        // Nothing running: nothing asked.
        f.player.position().await;
        assert_eq!(f.starts(), 0);
        let (mut lines, mut w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        next_json(&mut lines).await;
        let mpv = answer(&mut lines, &mut w, json!(754.5));
        let ((), asked) = tokio::join!(f.player.position(), mpv);
        assert_eq!(asked, json!(["get_property", "time-pos"]));
        assert_eq!(wait_event(&mut f.events).await, Event::Position(754.5));
        let mpv = answer(&mut lines, &mut w, json!(5594.2));
        let ((), asked) = tokio::join!(f.player.length(), mpv);
        assert_eq!(asked, json!(["get_property", "duration"]));
        assert_eq!(wait_event(&mut f.events).await, Event::Length(5594.2));
    }

    #[tokio::test]
    async fn nothing_shown_yet_is_not_recorded() {
        let mut f = Fake::new();
        let (mut lines, mut w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        next_json(&mut lines).await;
        let mpv = async {
            let msg = next_json(&mut lines).await;
            let reply = json!({ "request_id": msg["request_id"], "error": "property unavailable" });
            w.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
        };
        let (started, ()) = tokio::join!(f.player.record(Some(Path::new("/r/a.ts"))), mpv);
        started.unwrap();
        assert_eq!(
            wait_recording(&mut f.events).await,
            Event::Recording("/r/a.ts".into())
        );
        assert_eq!(
            wait_recording(&mut f.events).await,
            Event::RecordingEnded(Some("nothing is playing yet".into()))
        );
    }

    #[tokio::test]
    async fn mpvs_recorder_error_ends_the_recording_with_its_reason() {
        let mut f = Fake::new();
        let (mut lines, mut w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        next_json(&mut lines).await;
        start_recording(&mut f, &mut lines, &mut w, Path::new("/r/a.ts")).await;
        // As mpv 0.41 reports it; another module's error is not the recorder's,
        // and one it writes on after (mpv reconnecting) does not end it.
        for line in [
            r#"{"event":"log-message","prefix":"recorder","level":"error","text":"Failed writing packet.\n"}"#,
            r#"{"event":"log-message","prefix":"ffmpeg","level":"error","text":"http://h/1.ts: 403\n"}"#,
            r#"{"event":"log-message","prefix":"recorder","level":"error","text":"Failed opening output file.\n"}"#,
            r#"{"event":"log-message","prefix":"cplayer","level":"error","text":"Cache dumping stopped due to error.\n"}"#,
        ] {
            w.write_all(format!("{line}\n").as_bytes()).await.unwrap();
        }
        assert_eq!(
            wait_recording(&mut f.events).await,
            Event::RecordingEnded(Some("Failed opening output file.".into()))
        );
        // Ended already: a switch just loads.
        f.player
            .play("http://h/2.ts", "2 Two", Instant::now())
            .await
            .unwrap();
        assert_eq!(
            next_json(&mut lines).await["command"][1],
            "force-media-title"
        );
        assert_eq!(queued_event(&mut f.events), None, "reported once");
    }

    #[tokio::test]
    async fn the_stream_or_mpv_ending_ends_the_recording() {
        let mut f = Fake::new();
        let (mut lines, mut w) = f.play_spawning("http://h/1.ts", "1 One").await;
        next_json(&mut lines).await;
        next_json(&mut lines).await;
        start_recording(&mut f, &mut lines, &mut w, Path::new("/r/a.ts")).await;
        w.write_all(b"{\"event\":\"end-file\",\"reason\":\"eof\",\"playlist_entry_id\":1}\n")
            .await
            .unwrap();
        assert_eq!(
            wait_recording(&mut f.events).await,
            Event::RecordingEnded(None)
        );

        start_recording(&mut f, &mut lines, &mut w, Path::new("/r/b.ts")).await;
        unsafe { libc::kill(f.pid(), libc::SIGTERM) };
        drop((lines, w));
        assert_eq!(wait_event(&mut f.events).await, Event::RecordingEnded(None));
        assert_eq!(wait_event(&mut f.events).await, Event::Stopped);
    }

    const LIVE: &str = "http://h/live/u/p/1.ts";

    impl Fake {
        /// [`Fake::new`] retrying a drop after 20 ms, twice.
        fn reconnecting() -> Self {
            let mut f = Self::new();
            const RETRIES: [Duration; 2] = [Duration::from_millis(20), Duration::from_millis(20)];
            f.player.retries = &RETRIES;
            f
        }

        /// What the player task does beside its commands, once.
        async fn tick(&mut self) {
            let wake = tokio::time::timeout(Duration::from_secs(2), self.player.wait())
                .await
                .expect("something to wake for");
            self.player.wake(wake).await;
        }

        /// Nothing for the player task to wake for: no retry is due.
        async fn quiet(&mut self) {
            let wake = tokio::time::timeout(Duration::from_millis(100), self.player.wait()).await;
            assert!(wake.is_err(), "woke for {wake:?}");
        }
    }

    /// The next [`Event::Reconnecting`] or [`Event::Failed`], skipping others.
    async fn wait_reconnect(events: &mut mpsc::UnboundedReceiver<Event>) -> Event {
        loop {
            if let e @ (Event::Reconnecting(_) | Event::Failed { .. }) = wait_any(events).await {
                return e;
            }
        }
    }

    /// mpv's side of a `loadfile`: answers it with playlist entry `entry`.
    async fn load(lines: &mut Lines, w: &mut ServerWrite, entry: i64) -> Value {
        answer(lines, w, json!({ "playlist_entry_id": entry })).await
    }

    async fn send(w: &mut ServerWrite, line: &str) {
        w.write_all(format!("{line}\n").as_bytes()).await.unwrap();
    }

    fn end_file(reason: &str, entry: i64) -> String {
        json!({ "event": "end-file", "reason": reason, "playlist_entry_id": entry }).to_string()
    }

    const RESTART: &str = r#"{"event":"playback-restart"}"#;

    /// Plays [`LIVE`] as entry 1 and lets it start.
    async fn playing_live(f: &mut Fake) -> (Lines, ServerWrite) {
        let (mut lines, mut w) = f.play_spawning(LIVE, "1 One").await;
        next_json(&mut lines).await;
        load(&mut lines, &mut w, 1).await;
        send(&mut w, RESTART).await;
        (lines, w)
    }

    /// mpv's side of a retry: the channel again, unpaused, as entry `entry`.
    async fn retried(lines: &mut Lines, w: &mut ServerWrite, entry: i64) {
        assert_eq!(
            load(lines, w, entry).await,
            json!(["loadfile", LIVE, "replace"])
        );
        assert_eq!(
            next_json(lines).await["command"],
            json!(["set_property", "pause", false])
        );
    }

    #[tokio::test]
    async fn a_dropped_channel_is_played_again_until_it_plays() {
        let mut f = Fake::reconnecting();
        let (mut lines, mut w) = playing_live(&mut f).await;
        send(&mut w, &end_file("eof", 1)).await;
        f.tick().await;
        assert_eq!(
            wait_reconnect(&mut f.events).await,
            Event::Reconnecting(true)
        );
        f.tick().await;
        retried(&mut lines, &mut w, 2).await;
        // The retry fails to start: still a drop, so the second retry.
        let failed = json!({ "event": "end-file", "reason": "error", "playlist_entry_id": 2, "file_error": "loading failed" });
        send(&mut w, &failed.to_string()).await;
        f.tick().await;
        f.tick().await;
        retried(&mut lines, &mut w, 3).await;
        send(&mut w, RESTART).await;
        f.tick().await;
        assert_eq!(
            wait_reconnect(&mut f.events).await,
            Event::Reconnecting(false)
        );
        assert_eq!(queued_event(&mut f.events), None, "nothing failed");
        f.quiet().await;

        // Back, so a later drop starts over.
        send(&mut w, &end_file("eof", 3)).await;
        f.tick().await;
        assert_eq!(
            wait_reconnect(&mut f.events).await,
            Event::Reconnecting(true)
        );
    }

    /// M3U ticket 03: an M3U channel's headers go with its `loadfile` and
    /// its retries, never with the next channel's. It is live whatever its
    /// URL's path.
    #[tokio::test]
    async fn m3u_headers_go_with_their_channel_only() {
        let mut f = Fake::reconnecting();
        let (mut lines, mut w) = playing_live(&mut f).await;
        let headers = Headers {
            user_agent: Some("Mozilla/5.0 (X11, Linux)".into()),
            referrer: Some("https://r.example/".into()),
            origin: Some("https://o.example".into()),
        };
        const M3U: &str = "http://s.example/1.m3u8?token=t";
        f.player
            .play_at(M3U, Some(&headers), "2 Two", None, Instant::now())
            .await
            .unwrap();
        next_json(&mut lines).await;
        let options = "user-agent=%24%Mozilla/5.0 (X11, Linux),\
            referrer=%18%https://r.example/,\
            http-header-fields=%25%Origin: https://o.example";
        let m3u = json!(["loadfile", M3U, "replace", -1, options]);
        assert_eq!(load(&mut lines, &mut w, 2).await, m3u);
        send(&mut w, RESTART).await;
        send(&mut w, &end_file("eof", 2)).await;
        f.tick().await;
        assert_eq!(
            wait_reconnect(&mut f.events).await,
            Event::Reconnecting(true)
        );
        f.tick().await;
        assert_eq!(load(&mut lines, &mut w, 3).await, m3u);
        next_json(&mut lines).await;
        f.player.play(LIVE, "1 One", Instant::now()).await.unwrap();
        next_json(&mut lines).await;
        assert_eq!(
            load(&mut lines, &mut w, 4).await,
            json!(["loadfile", LIVE, "replace"])
        );
    }

    #[test]
    fn loadfile_options_quote_header_values() {
        let only = |h: Headers| loadfile_options(None, Some(&h)).unwrap();
        let agent = Headers {
            user_agent: Some("A, b=c".into()),
            ..Headers::default()
        };
        assert_eq!(only(agent), "user-agent=%6%A, b=c");
        let origin = Headers {
            origin: Some("https://o.example,x".into()),
            ..Headers::default()
        };
        assert_eq!(
            only(origin),
            r"http-header-fields=%28%Origin: https://o.example\,x"
        );
        assert_eq!(
            loadfile_options(Some(600.0), Some(&Headers::default())),
            Some("start=600".into())
        );
        assert_eq!(loadfile_options(None, Some(&Headers::default())), None);
        assert_eq!(loadfile_options(None, None), None);
    }

    #[tokio::test]
    async fn reconnecting_gives_up_after_the_last_retry() {
        let mut f = Fake::reconnecting();
        let (mut lines, mut w) = playing_live(&mut f).await;
        send(&mut w, &end_file("eof", 1)).await;
        f.tick().await;
        assert_eq!(
            wait_reconnect(&mut f.events).await,
            Event::Reconnecting(true)
        );
        for entry in [2, 3] {
            f.tick().await;
            retried(&mut lines, &mut w, entry).await;
            send(&mut w, &end_file("eof", entry)).await;
            f.tick().await;
        }
        assert_eq!(
            wait_reconnect(&mut f.events).await,
            Event::Failed {
                title: "1 One".into(),
                reason: "the stream ended".into()
            }
        );
        f.quiet().await;
    }

    #[tokio::test]
    async fn the_user_playing_or_pressing_a_key_cancels_reconnecting() {
        let mut f = Fake::reconnecting();
        let (mut lines, mut w) = playing_live(&mut f).await;
        send(&mut w, &end_file("eof", 1)).await;
        f.tick().await;
        f.player.control(Control::Mute).await.unwrap();
        assert_eq!(
            wait_reconnect(&mut f.events).await,
            Event::Reconnecting(true)
        );
        assert_eq!(
            wait_reconnect(&mut f.events).await,
            Event::Reconnecting(false)
        );
        assert_eq!(
            next_json(&mut lines).await["command"],
            json!(["cycle", "mute"])
        );
        f.quiet().await;

        // A play: the channel played instead, and no retry after.
        f.player.play(LIVE, "1 One", Instant::now()).await.unwrap();
        next_json(&mut lines).await;
        load(&mut lines, &mut w, 2).await;
        send(&mut w, RESTART).await;
        send(&mut w, &end_file("eof", 2)).await;
        f.tick().await;
        assert_eq!(
            wait_reconnect(&mut f.events).await,
            Event::Reconnecting(true)
        );
        f.player
            .play("http://h/live/u/p/2.ts", "2 Two", Instant::now())
            .await
            .unwrap();
        assert_eq!(
            wait_reconnect(&mut f.events).await,
            Event::Reconnecting(false)
        );
        assert_eq!(next_json(&mut lines).await["command"][2], "2 Two");
        load(&mut lines, &mut w, 3).await;
        f.quiet().await;

        // `L` retries at once, even at live.
        send(&mut w, RESTART).await;
        send(&mut w, &end_file("eof", 3)).await;
        f.tick().await;
        f.player.control(Control::Live).await.unwrap();
        assert_eq!(
            next_json(&mut lines).await["command"],
            json!(["loadfile", "http://h/live/u/p/2.ts", "replace"])
        );
    }

    #[tokio::test]
    async fn stop_catch_up_and_older_loads_ending_are_not_drops() {
        let mut f = Fake::reconnecting();
        let (mut lines, mut w) = playing_live(&mut f).await;
        // Replaced by another channel: its end is not a drop.
        f.player
            .play("http://h/live/u/p/2.ts", "2 Two", Instant::now())
            .await
            .unwrap();
        next_json(&mut lines).await;
        send(&mut w, &end_file("eof", 1)).await;
        load(&mut lines, &mut w, 2).await;
        send(&mut w, &end_file("stop", 1)).await;
        f.quiet().await;

        // A channel that never started is an error, not a drop.
        let failed = json!({ "event": "end-file", "reason": "error", "playlist_entry_id": 2, "file_error": "loading failed" });
        send(&mut w, &failed.to_string()).await;
        assert_eq!(
            wait_reconnect(&mut f.events).await,
            Event::Failed {
                title: "2 Two".into(),
                reason: "loading failed".into()
            }
        );
        f.quiet().await;

        // Catch-up running out is its end (catch-up ticket 04).
        let archive = "http://h/timeshift/u/p/60/2026-10-01:20-00/1.ts";
        f.player
            .play(archive, "1 One", Instant::now())
            .await
            .unwrap();
        next_json(&mut lines).await;
        load(&mut lines, &mut w, 3).await;
        send(&mut w, RESTART).await;
        send(&mut w, &end_file("eof", 3)).await;
        assert_eq!(wait_event(&mut f.events).await, Event::Ended);
        f.quiet().await;

        // Stop quits mpv: no drop, and nothing to retry.
        f.player.play(LIVE, "1 One", Instant::now()).await.unwrap();
        next_json(&mut lines).await;
        load(&mut lines, &mut w, 4).await;
        send(&mut w, RESTART).await;
        let mpv = async {
            assert_eq!(next_json(&mut lines).await["command"], json!(["quit"]));
            send(&mut w, &end_file("quit", 4)).await;
            drop((lines, w));
        };
        let (stopped, ()) = tokio::join!(f.player.control(Control::Stop), mpv);
        stopped.unwrap();
        assert_eq!(queued_event(&mut f.events), Some(Event::Stopped));
        f.quiet().await;
    }

    #[tokio::test]
    async fn mpv_reconnecting_by_itself_is_back_at_live() {
        let mut f = Fake::reconnecting();
        let (_lines, mut w) = playing_live(&mut f).await;
        let pause = |on: bool| {
            format!("{{\"event\":\"property-change\",\"id\":1,\"name\":\"pause\",\"data\":{on}}}")
        };
        send(&mut w, &pause(true)).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        send(&mut w, &pause(false)).await;
        // Behind live: paused, then resumed.
        loop {
            let b = wait_behind(&mut f.events).await;
            if !b.paused() && !b.at(Instant::now()).is_zero() {
                break;
            }
        }
        // A seek's restart leaves the time behind as it is; one with no
        // seek is mpv reconnecting, at the live edge.
        send(&mut w, r#"{"event":"seek"}"#).await;
        send(&mut w, RESTART).await;
        send(&mut w, RESTART).await;
        assert_eq!(wait_behind(&mut f.events).await, Behind::default());
        assert_eq!(queued_event(&mut f.events), None, "not a drop");
        f.quiet().await;
    }

    /// Gone entirely: not running, and not left as a zombie.
    async fn assert_reaped(pid: i32) {
        let proc = Path::new("/proc").join(pid.to_string());
        for _ in 0..200 {
            if !proc.exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let state = std::fs::read_to_string(proc.join("stat")).unwrap_or_default();
        panic!("fake mpv {pid} not reaped: {state}");
    }

    /// Manual smoke test against the real mpv (opens a window):
    /// `cargo test real_mpv -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn real_mpv_smoke() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let (tx, mut events) = mpsc::unbounded_channel();
        let mut player = Player::new(
            Launch {
                program: "mpv".into(),
                extra_args: vec!["--mute=yes".into()],
                socket: socket.clone(),
            },
            tx,
        );
        // A second IPC client, to look at mpv from outside.
        async fn get(socket: &Path, prop: &str) -> Value {
            let stream = UnixStream::connect(socket).await.unwrap();
            let (read, mut write) = stream.into_split();
            let q = json!({ "command": ["get_property", prop], "request_id": 1 });
            write.write_all(format!("{q}\n").as_bytes()).await.unwrap();
            let mut lines = BufReader::new(read).lines();
            loop {
                let v: Value =
                    serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                if v["request_id"] == 1 {
                    return v["data"].clone();
                }
            }
        }

        let t = Instant::now();
        player
            .play("av://lavfi:testsrc", "1 Test source", t)
            .await
            .unwrap();
        eprintln!(
            "first play (spawn + loadfile): {} ms",
            t.elapsed().as_millis()
        );
        // SPEC 9.1: the observed properties' first values.
        for expected in [
            Event::Started,
            Event::Paused(false),
            Event::Muted(true),
            Event::Fullscreen(false),
        ] {
            assert_eq!(wait_event(&mut events).await, expected);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(get(&socket, "media-title").await, "1 Test source");

        let t = Instant::now();
        player
            .play("av://lavfi:mandelbrot", "2 Mandelbrot", t)
            .await
            .unwrap();
        eprintln!("switch (loadfile sent): {} µs", t.elapsed().as_micros());
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(get(&socket, "media-title").await, "2 Mandelbrot");

        player
            .play(
                "http://127.0.0.1:9/live/u/p/3.ts",
                "3 Unreachable",
                Instant::now(),
            )
            .await
            .unwrap();
        let event = wait_event(&mut events).await;
        eprintln!("error event: {event:?}");
        assert!(matches!(&event, Event::Failed { title, .. } if title == "3 Unreachable"));
        assert_eq!(get(&socket, "media-title").await, "3 Unreachable");

        // SPEC 9.2 keys: a simulated key press in mpv comes back as an event.
        let stream = UnixStream::connect(&socket).await.unwrap();
        let (_read, mut write) = stream.into_split();
        for (key, expected) in [
            ("PGDWN", Remote::Next),
            ("PGUP", Remote::Prev),
            ("5", Remote::Digit(5)),
            ("BS", Remote::Back),
            ("L", Remote::Live),
            ("R", Remote::Record),
            ("S", Remote::StartOver),
        ] {
            let cmd = json!({ "command": ["keypress", key] });
            write
                .write_all(format!("{cmd}\n").as_bytes())
                .await
                .unwrap();
            assert_eq!(
                wait_event(&mut events).await,
                Event::Remote(expected),
                "{key}"
            );
        }
        // Only those keys changed: SPACE keeps its binding (the user's
        // input.conf or mpv's default).
        let bindings = get(&socket, "input-bindings").await;
        let active = |key: &str| -> Vec<String> {
            bindings
                .as_array()
                .unwrap()
                .iter()
                .filter(|b| b["key"] == key)
                .map(|b| b["cmd"].as_str().unwrap_or("").to_string())
                .collect()
        };
        eprintln!("PGDWN: {:?}\nSPACE: {:?}", active("PGDWN"), active("SPACE"));
        assert!(
            active("PGDWN")
                .iter()
                .any(|c| c == "script-message vidicon next")
        );
        assert!(!active("SPACE").is_empty());
        assert!(active("SPACE").iter().all(|c| !c.contains("vidicon")));

        // Pausing and muting inside mpv reach the app (assumes mpv's default
        // SPACE and m bindings), and so do the app's own controls.
        for (key, expected) in [("SPACE", Event::Paused(true)), ("m", Event::Muted(false))] {
            let cmd = json!({ "command": ["keypress", key] });
            write
                .write_all(format!("{cmd}\n").as_bytes())
                .await
                .unwrap();
            assert_eq!(wait_event(&mut events).await, expected, "{key}");
        }
        for (control, expected) in [
            (Control::Pause, Event::Paused(false)),
            (Control::Mute, Event::Muted(true)),
            (Control::Fullscreen, Event::Fullscreen(true)),
            (Control::Fullscreen, Event::Fullscreen(false)),
        ] {
            player.control(control).await.unwrap();
            assert_eq!(wait_event(&mut events).await, expected, "{control:?}");
        }
        // Going live after a pause reloads and unpauses.
        player
            .play("av://lavfi:testsrc", "4 Test source", Instant::now())
            .await
            .unwrap();
        player.control(Control::Pause).await.unwrap();
        assert_eq!(wait_event(&mut events).await, Event::Paused(true));
        tokio::time::sleep(Duration::from_millis(500)).await;
        player.control(Control::Live).await.unwrap();
        assert_eq!(wait_event(&mut events).await, Event::Paused(false));
        assert_eq!(wait_behind(&mut events).await, Behind::default());
        assert_eq!(get(&socket, "media-title").await, "4 Test source");
        player.control(Control::Stop).await.unwrap();
        assert_eq!(queued_event(&mut events), Some(Event::Stopped));
        player
            .play("av://lavfi:testsrc", "1 Test source", Instant::now())
            .await
            .unwrap();
        assert_eq!(wait_event(&mut events).await, Event::Started);
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(get(&socket, "media-title").await, "1 Test source");
    }
}
