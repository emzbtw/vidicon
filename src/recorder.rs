//! Scheduled recordings (recording ticket 02; SPEC 9.5).
//!
//! Schedules live in the store. At a schedule's padded start an `ffmpeg`
//! child copies the channel's stream to a `.ts` file on a connection of its
//! own, and is stopped at the padded stop. A start that would take more
//! connections than the account allows (SPEC 6.3) fails rather than waits.
//! The task sleeps until the next start or stop, and while anything is
//! scheduled or recording also wakes on each minute, which catches up after
//! a suspend. The app must be running: what came while it was closed is
//! missed.
//!
//! Recordings of what plays (SPEC 9.4) are mpv's, not the recorder's, but
//! it stores them too, so the Recordings view lists them with the rest.
//!
//! Downloads (VOD ticket 10; SPEC 9.6) are queued here too, as they take
//! connections from the same count: one at a time per provider, and only
//! on a connection nothing else needs. Playback or a recording that needs
//! it pauses the download, which resumes once one is free.

use crate::download;
use crate::import::{Writer, unix_now};
use crate::store::{self, DlState, Download, NewDownload, NewRecording, RecState, Recording};
use crate::trash;
use crate::xtream::{self, Clients};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command as Process};
use tokio::sync::{mpsc, oneshot};

/// How long a stopped ffmpeg gets to finish its file before it is killed.
const STOP_WAIT: Duration = Duration::from_secs(2);

/// A done recording's note when the app quit while it ran.
const QUIT_NOTE: &str = "cut short: the app quit";
/// The same when the app ended without quitting (a crash), seen at launch.
const CRASH_NOTE: &str = "cut short: the app closed";

/// How recordings are made.
#[derive(Debug)]
pub struct Setup {
    pub ffmpeg: String,
    pub dir: PathBuf,
    /// The home trash, where a deleted recording's file goes.
    pub trash: PathBuf,
    /// Padding, in seconds, from `record_before` and `record_after`.
    pub before: i64,
    pub after: i64,
    /// Where downloads go, `downloads_dir`.
    pub downloads: PathBuf,
}

/// What the app tells the recorder.
#[derive(Debug)]
pub enum Command {
    /// Record a programme. One airing now starts at once. One scheduled
    /// or recording already is left as it is.
    Schedule(NewRecording),
    /// Cancel a schedule, or stop a recording, keeping what it wrote.
    Cancel(i64),
    /// Schedules changed in the store: a guide import moved some.
    Changed,
    /// The provider mpv is playing from, which takes one of its
    /// connections, or `None`.
    Watching(Option<String>),
    /// A provider's `max_connections` from its last account check.
    Limit(String, Option<i64>),
    /// Stop every recording, keeping each as done, and answer when they are.
    Quit(oneshot::Sender<()>),
    /// mpv started recording what plays to this file (SPEC 9.4); the
    /// programme's `start` is when, its `stop` unused.
    Playing(NewRecording, PathBuf),
    /// mpv's recording of what plays ended, with its reason if it failed.
    PlayingEnded(Option<String>),
    /// Forget the finished recordings, keeping their files.
    ClearFinished,
    /// Move a finished recording's file to the trash, hiding it.
    Delete(i64),
    /// Queue movies or episodes to download.
    Download(Vec<NewDownload>),
    /// Stop a queued or running download, removing its part, and forget it.
    CancelDownload(i64),
    /// Move a done download's file to the trash, hiding it, or drop a
    /// failed one's part and forget it.
    DeleteDownload(i64),
    /// Forget the done and failed downloads, keeping done ones' files.
    ClearDownloads,
    /// The settings changed (settings ticket 02): the next schedule,
    /// recording and download use these; what runs keeps going.
    Setup(Setup),
    /// A provider was removed (onboarding ticket 02): its recordings stop
    /// as cancelled and its download stops, keeping what each wrote; then
    /// its data goes ([`store::remove_provider`]), reported as
    /// [`Event::ProviderRemoved`].
    RemoveProvider { provider_id: String, personal: bool },
}

/// What the recorder tells the app; each carries the row as it now is.
#[derive(Debug, PartialEq)]
pub enum Event {
    /// Stored as scheduled; one airing now starts next.
    Scheduled(Recording),
    Started(Recording),
    /// Done, failed (with why in `note`) or cancelled.
    Ended(Recording),
    /// This many schedules passed unstarted.
    Missed(usize),
    /// A recording of what plays was stored or ended: the list changed, and
    /// the app has said so already.
    Listed,
    /// This many finished recordings were forgotten, the second number of
    /// them having a file, which is kept.
    Cleared(usize, usize),
    /// What became of a recording `Delete` was for, the row as it was.
    Deleted(Recording, Deleted),
    /// What `Download` did.
    DlQueued(store::Queued),
    /// A download started, `true` when it resumes a part.
    DlStarted(Download, bool),
    /// How far a running download is, in bytes, and its size if known.
    DlProgress {
        id: i64,
        have: u64,
        total: Option<u64>,
    },
    /// A download gave up its connection, which playback or a recording
    /// needs, and is queued again.
    DlPaused(Download),
    /// Done (with where its file is) or failed (with why in `note`).
    DlEnded(Download),
    /// Cancelled and forgotten, as it was.
    DlCancelled(Download),
    /// What became of a download `DeleteDownload` was for, as it was.
    DlDeleted(Download, Deleted),
    /// This many finished downloads were forgotten, the second number of
    /// them having a file, which is kept.
    DlCleared(usize, usize),
    /// What `RemoveProvider` did: the provider's id, and whether its data
    /// went.
    ProviderRemoved(String, bool),
}

/// What `Delete` did. `Trashed` marks the row trashed, which the
/// Recordings view hides while its file is gone; `NoFile` forgets it.
#[derive(Debug, PartialEq)]
pub enum Deleted {
    /// Its file went to the trash.
    Trashed,
    /// It had no file, or its file was gone already.
    NoFile,
    /// Its file is outside `recordings_dir`, so it was left alone.
    Elsewhere,
    /// It is scheduled or recording, which `Cancel` is for.
    NotFinished,
    /// The move failed, saying why; nothing changed.
    Failed(String),
}

/// How an ffmpeg ended.
#[derive(Debug)]
enum Exit {
    /// Stopped by the recorder.
    Stopped,
    /// By itself, with its last error line.
    Early(String),
}

/// Why a running download was stopped.
#[derive(Clone, Copy, PartialEq)]
enum Halt {
    None,
    /// Its connection is needed: queued again.
    Pause,
    Cancel,
    Quit,
}

/// A running download, one per provider.
struct Fetching {
    id: i64,
    total: Option<u64>,
    /// Taken when the stop is sent.
    stop: Option<oneshot::Sender<()>>,
    halt: Halt,
}

/// What a download's task tells the recorder.
#[derive(Debug)]
enum DlMsg {
    Report(download::Report),
    Exit(download::Exit),
}

struct Running {
    provider_id: String,
    stop_at: i64,
    /// Taken when the stop is sent.
    stop: Option<oneshot::Sender<()>>,
    cancelled: bool,
}

pub struct Recorder {
    writer: Writer,
    clients: Clients,
    setup: Setup,
    events: mpsc::UnboundedSender<Event>,
    watching: Option<String>,
    limits: HashMap<String, Option<i64>>,
    running: HashMap<i64, Running>,
    exits: mpsc::UnboundedSender<(i64, Exit)>,
    /// The recording of what plays, and whether it was cancelled.
    playing: Option<(i64, bool)>,
    /// The download running for each provider.
    fetching: HashMap<String, Fetching>,
    dl_msgs: mpsc::UnboundedSender<(i64, DlMsg)>,
    http: reqwest::Client,
}

impl Recorder {
    /// Runs until `commands` closes or asks to quit. At launch, recordings
    /// left running by a crash are done (cut short), and schedules that
    /// passed while the app was closed are missed; one still inside its
    /// padded times starts late.
    pub async fn run(
        writer: Writer,
        clients: Clients,
        setup: Setup,
        events: mpsc::UnboundedSender<Event>,
        mut commands: mpsc::UnboundedReceiver<Command>,
    ) {
        let (exits, mut exited) = mpsc::unbounded_channel();
        let (dl_msgs, mut dl_rx) = mpsc::unbounded_channel();
        let mut this = Self {
            writer,
            clients,
            setup,
            events,
            watching: None,
            limits: HashMap::new(),
            running: HashMap::new(),
            exits,
            playing: None,
            fetching: HashMap::new(),
            dl_msgs,
            http: download::http(),
        };
        this.db(store::settle_downloads).await;
        let now = unix_now();
        let missed = this
            .db(move |c| store::settle_recordings(c, now, CRASH_NOTE))
            .await;
        let mut wait = this.check().await;
        // After any late start's message, so the status bar keeps this one.
        if let Some(n) = missed.filter(|&n| n > 0) {
            let _ = this.events.send(Event::Missed(n));
        }
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command::Quit(done)) => {
                        this.quit(&mut exited, &mut dl_rx).await;
                        let _ = done.send(());
                        return;
                    }
                    Some(command) => this.command(command).await,
                    None => return this.quit(&mut exited, &mut dl_rx).await,
                },
                Some((id, exit)) = exited.recv() => this.exited(id, exit).await,
                Some((id, msg)) = dl_rx.recv() => this.dl_msg(id, msg).await,
                () = sleep(wait) => {}
            }
            wait = this.check().await;
        }
    }

    async fn command(&mut self, command: Command) {
        match command {
            Command::Schedule(r) => {
                let (before, after) = (self.setup.before, self.setup.after);
                let scheduled = self
                    .db(move |c| {
                        if store::active_recording(c, &r.provider_id, r.stream_id, r.start)?
                            .is_some()
                        {
                            return Ok(None);
                        }
                        let id = store::schedule_recording(c, &r, before, after)?;
                        store::recording(c, id)
                    })
                    .await
                    .flatten();
                if let Some(r) = scheduled {
                    let _ = self.events.send(Event::Scheduled(r));
                }
            }
            Command::Cancel(id) => {
                if let Some((playing, cancelled)) = &mut self.playing
                    && *playing == id
                {
                    // Kept as cancelled once mpv, which the app stops, ends it.
                    *cancelled = true;
                } else if let Some(r) = self.running.get_mut(&id) {
                    r.cancelled = true;
                    if let Some(stop) = r.stop.take() {
                        let _ = stop.send(());
                    }
                } else if self.db(move |c| store::cancel_schedule(c, id)).await == Some(true) {
                    self.report(id, Event::Ended).await;
                }
            }
            Command::Changed | Command::Quit(_) => {}
            Command::Watching(provider_id) => {
                self.watching = provider_id.clone();
                if let Some(p) = provider_id {
                    self.make_room(&p, 0);
                }
            }
            Command::Limit(provider_id, max) => {
                self.limits.insert(provider_id, max);
            }
            Command::Playing(r, file) => {
                let file = file.display().to_string();
                let id = self.db(move |c| store::record_playing(c, &r, &file)).await;
                self.playing = id.map(|id| (id, false));
                let _ = self.events.send(Event::Listed);
            }
            Command::PlayingEnded(error) => {
                let Some((id, cancelled)) = self.playing.take() else {
                    return;
                };
                let (state, note) = match error {
                    Some(e) => (RecState::Failed, Some(xtream::redact(&e))),
                    None if cancelled => (RecState::Cancelled, None),
                    None => (RecState::Done, None),
                };
                self.end_playing(id, state, note).await;
                let _ = self.events.send(Event::Listed);
            }
            Command::ClearFinished => {
                if let Some((n, with_file)) = self.db(store::clear_finished_recordings).await {
                    let _ = self.events.send(Event::Cleared(n, with_file));
                }
            }
            Command::Delete(id) => {
                let (dir, bin) = (self.setup.dir.clone(), self.setup.trash.clone());
                let deleted = self
                    .db(move |c| {
                        let Some(r) = store::recording(c, id)? else {
                            return Ok(None);
                        };
                        let deleted = trash_file(&r, &dir, &bin);
                        match deleted {
                            // Kept, so a restore from the trash shows it again.
                            Deleted::Trashed => store::trash_recording(c, id)?,
                            Deleted::NoFile => store::forget_recording(c, id)?,
                            _ => {}
                        }
                        Ok(Some((r, deleted)))
                    })
                    .await
                    .flatten();
                if let Some((r, deleted)) = deleted {
                    let _ = self.events.send(Event::Deleted(r, deleted));
                }
            }
            Command::Download(titles) => {
                let now = unix_now();
                let queued = self
                    .db(move |c| store::queue_downloads(c, &titles, now))
                    .await;
                if let Some(queued) = queued {
                    let _ = self.events.send(Event::DlQueued(queued));
                }
            }
            Command::CancelDownload(id) => {
                if let Some(f) = self.fetching.values_mut().find(|f| f.id == id) {
                    // Forgotten once its task has stopped.
                    f.halt = Halt::Cancel;
                    if let Some(stop) = f.stop.take() {
                        let _ = stop.send(());
                    }
                    return;
                }
                let d = self.db(move |c| store::download(c, id)).await.flatten();
                if let Some(d) = d.filter(|d| d.state != DlState::Done) {
                    self.cancelled(d).await;
                }
            }
            Command::DeleteDownload(id) => {
                let (dir, bin) = (self.setup.downloads.clone(), self.setup.trash.clone());
                let deleted = self
                    .db(move |c| {
                        let Some(d) = store::download(c, id)? else {
                            return Ok(None);
                        };
                        let deleted = trash_download(&d, &dir, &bin);
                        match deleted {
                            // Kept, so a restore from the trash shows it again.
                            Deleted::Trashed => store::trash_download(c, id)?,
                            Deleted::NoFile => store::forget_download(c, id)?,
                            _ => {}
                        }
                        Ok(Some((d, deleted)))
                    })
                    .await
                    .flatten();
                if let Some((d, deleted)) = deleted {
                    let _ = self.events.send(Event::DlDeleted(d, deleted));
                }
            }
            Command::Setup(setup) => self.setup = setup,
            Command::RemoveProvider {
                provider_id,
                personal,
            } => {
                for r in self.running.values_mut() {
                    if r.provider_id == provider_id {
                        r.cancelled = true;
                        if let Some(stop) = r.stop.take() {
                            let _ = stop.send(());
                        }
                    }
                }
                // Paused, its part kept; the removal forgets its row.
                if let Some(f) = self.fetching.get_mut(&provider_id) {
                    f.halt = Halt::Pause;
                    if let Some(stop) = f.stop.take() {
                        let _ = stop.send(());
                    }
                }
                self.limits.remove(&provider_id);
                let id = provider_id.clone();
                let removed = self
                    .db(move |c| store::remove_provider(c, &id, personal))
                    .await;
                let _ = self
                    .events
                    .send(Event::ProviderRemoved(provider_id, removed.is_some()));
            }
            Command::ClearDownloads => {
                if let Some(cleared) = self.db(store::clear_finished_downloads).await {
                    // Counting what the tab showed: trashed ones but restored.
                    let (mut n, mut with_file) = (0, 0);
                    for d in &cleared {
                        n += usize::from(store::download_shown(d));
                        match d.state {
                            DlState::Done => {
                                with_file += usize::from(
                                    d.file.as_deref().is_some_and(|f| Path::new(f).exists()),
                                );
                            }
                            _ => remove_part(d),
                        }
                    }
                    let _ = self.events.send(Event::DlCleared(n, with_file));
                }
            }
        }
    }

    /// Starts the next queued download of each provider that has none
    /// running and a connection free.
    async fn start_downloads(&mut self) {
        let providers = self
            .db(store::queued_download_providers)
            .await
            .unwrap_or_default();
        for provider_id in providers {
            if self.fetching.contains_key(&provider_id) || self.free(&provider_id, true) == 0 {
                continue;
            }
            let p = provider_id.clone();
            if let Some(Some(d)) = self.db(move |c| store::next_download(c, &p)).await {
                self.start_download(d).await;
            }
        }
    }

    /// Starts download `d`, or fails it saying why.
    async fn start_download(&mut self, mut d: Download) {
        let Some(client) = self.clients.get(&d.provider_id) else {
            let note = "the provider is no longer in the credentials file";
            return self.dl_failed(d.id, note).await;
        };
        let kind = if d.series_id.is_some() {
            "series"
        } else {
            "movie"
        };
        let url = match client.vod_url(kind, d.stream_id, &d.extension) {
            Ok(url) => url,
            Err(e) => return self.dl_failed(d.id, &e).await,
        };
        let file = match &d.file {
            Some(file) => PathBuf::from(file),
            None => {
                let stem = download::file_stem(&d);
                match download::reserve(&self.setup.downloads, &stem, &d.extension) {
                    Ok(file) => file,
                    Err(e) => {
                        let dir = self.setup.downloads.display();
                        return self
                            .dl_failed(d.id, &format!("can't download to {dir}: {e}"))
                            .await;
                    }
                }
            }
        };
        let part = download::part_of(&file);
        let resumed = std::fs::metadata(&part).is_ok_and(|m| m.len() > 0);
        log::info!("downloading {}", file.display());
        let path = file.display().to_string();
        let id = d.id;
        self.db(move |c| store::download_started(c, id, &path))
            .await;
        let (stop, stopped) = oneshot::channel();
        self.fetching.insert(
            d.provider_id.clone(),
            Fetching {
                id,
                total: d.total.map(|t| t as u64),
                stop: Some(stop),
                halt: Halt::None,
            },
        );
        let (http, msgs) = (self.http.clone(), self.dl_msgs.clone());
        tokio::spawn(async move {
            let report = |r| {
                let _ = msgs.send((id, DlMsg::Report(r)));
            };
            let exit = download::fetch(&http, url, &part, stopped, report).await;
            let _ = msgs.send((id, DlMsg::Exit(exit)));
        });
        d.state = DlState::Downloading;
        d.file = Some(file.display().to_string());
        let _ = self.events.send(Event::DlStarted(d, resumed));
    }

    /// What a download's task said.
    async fn dl_msg(&mut self, id: i64, msg: DlMsg) {
        let Some(provider_id) = self
            .fetching
            .iter()
            .find(|(_, f)| f.id == id)
            .map(|(p, _)| p.clone())
        else {
            return;
        };
        match msg {
            DlMsg::Report(download::Report::Total(total)) => {
                if let Some(f) = self.fetching.get_mut(&provider_id) {
                    f.total = Some(total);
                }
                self.db(move |c| store::download_total(c, id, total as i64))
                    .await;
            }
            DlMsg::Report(download::Report::Have(have)) => {
                let total = self.fetching.get(&provider_id).and_then(|f| f.total);
                let _ = self.events.send(Event::DlProgress { id, have, total });
            }
            DlMsg::Exit(exit) => {
                let Some(f) = self.fetching.remove(&provider_id) else {
                    return;
                };
                let Some(d) = self.db(move |c| store::download(c, id)).await.flatten() else {
                    return;
                };
                match (exit, f.halt) {
                    (download::Exit::Done, _) => self.dl_done(d).await,
                    (_, Halt::Cancel) => self.cancelled(d).await,
                    (download::Exit::Stopped, _) => {
                        self.db(move |c| {
                            store::set_download_state(c, id, DlState::Queued, None, None, 0)
                        })
                        .await;
                        if let Some(Some(d)) = self.db(move |c| store::download(c, id)).await {
                            let _ = self.events.send(Event::DlPaused(d));
                        }
                    }
                    (download::Exit::Failed(note), _) => {
                        log::warn!("download {id} failed: {note}");
                        self.dl_failed(id, &note).await;
                    }
                }
            }
        }
    }

    /// Renames done download `d`'s part to its file.
    async fn dl_done(&mut self, d: Download) {
        let id = d.id;
        let Some(file) = d.file.as_deref().map(PathBuf::from) else {
            return self.dl_failed(id, "it has no file").await;
        };
        let file = match download::finish(&file) {
            Ok(file) => file.display().to_string(),
            Err(e) => {
                return self
                    .dl_failed(id, &format!("can't finish {}: {e}", file.display()))
                    .await;
            }
        };
        log::info!("downloaded {file}");
        let now = unix_now();
        self.db(move |c| store::set_download_state(c, id, DlState::Done, None, Some(&file), now))
            .await;
        self.report_download(id, Event::DlEnded).await;
    }

    /// Fails download `id` with `note`, keeping its part to resume.
    async fn dl_failed(&mut self, id: i64, note: &str) {
        let note = xtream::redact(note);
        let now = unix_now();
        self.db(move |c| store::set_download_state(c, id, DlState::Failed, Some(&note), None, now))
            .await;
        self.report_download(id, Event::DlEnded).await;
    }

    /// Forgets download `d` and removes its part.
    async fn cancelled(&mut self, d: Download) {
        remove_part(&d);
        let id = d.id;
        self.db(move |c| store::forget_download(c, id)).await;
        let _ = self.events.send(Event::DlCancelled(d));
    }

    /// Sends `event` with download `id` as stored.
    async fn report_download(&self, id: i64, event: fn(Download) -> Event) {
        if let Some(Some(d)) = self.db(move |c| store::download(c, id)).await {
            let _ = self.events.send(event(d));
        }
    }

    /// Pauses `provider_id`'s download if it holds a connection that mpv's
    /// stream and the recordings, with `extra` more, need.
    fn make_room(&mut self, provider_id: &str, extra: usize) {
        // The download holds one of them, so this many must be free.
        if self.free(provider_id, false) > extra {
            return;
        }
        if let Some(f) = self.fetching.get_mut(provider_id)
            && let Some(stop) = f.stop.take()
        {
            f.halt = Halt::Pause;
            let _ = stop.send(());
        }
    }

    /// How many of `provider_id`'s connections are free, past mpv's, if it
    /// plays from there, the recordings running and, with `downloads`, the
    /// download. An unknown limit counts as 1.
    fn free(&self, provider_id: &str, downloads: bool) -> usize {
        let max = self.limits.get(provider_id).copied().flatten();
        let allowed = max.filter(|&m| m > 0).map_or(1, |m| m as usize);
        let watching = usize::from(self.watching.as_deref() == Some(provider_id));
        let recording = self
            .running
            .values()
            .filter(|r| r.provider_id == provider_id)
            .count();
        let downloading = usize::from(downloads && self.fetching.contains_key(provider_id));
        allowed.saturating_sub(watching + recording + downloading)
    }

    /// Ends the recording of what plays at `id` in `state`. A file left
    /// empty, as by a failure, is removed and forgotten.
    async fn end_playing(&self, id: i64, state: RecState, note: Option<String>) {
        let now = unix_now();
        self.db(move |c| {
            let file = store::recording(c, id)?.and_then(|r| r.file);
            let empty = file.is_some_and(|f| {
                let empty = std::fs::metadata(&f).is_ok_and(|m| m.len() == 0);
                if empty {
                    let _ = std::fs::remove_file(&f);
                }
                empty
            });
            store::recording_stopped_at(c, id, now)?;
            store::set_recording_state(c, id, state, note.as_deref(), empty)
        })
        .await;
    }

    /// Stops what is due to stop, misses what passed, starts what is due.
    /// Returns how long to sleep: until the next start or stop, at most to
    /// the next minute, or `None` with nothing scheduled or running.
    async fn check(&mut self) -> Option<Duration> {
        let now = unix_now();
        for r in self.running.values_mut().filter(|r| r.stop_at <= now) {
            if let Some(stop) = r.stop.take() {
                let _ = stop.send(());
            }
        }
        if let Some(n) = self
            .db(move |c| store::miss_passed(c, now))
            .await
            .filter(|&n| n > 0)
        {
            let _ = self.events.send(Event::Missed(n));
        }
        let scheduled = self.db(store::scheduled).await.unwrap_or_default();
        let mut next: Option<i64> = None;
        for r in scheduled {
            if r.padded_start <= now {
                self.start(r).await;
            } else {
                next = Some(next.map_or(r.padded_start, |t| t.min(r.padded_start)));
            }
        }
        self.start_downloads().await;
        let stops = self.running.values().filter(|r| r.stop.is_some());
        if let Some(stop) = stops.map(|r| r.stop_at).min() {
            next = Some(next.map_or(stop, |t| t.min(stop)));
        }
        if next.is_none() && self.running.is_empty() {
            return None;
        }
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let minute = Duration::from_secs(60 - since.as_secs() % 60)
            - Duration::from_nanos(since.subsec_nanos().into());
        let due = next.map_or(minute, |t| {
            Duration::from_secs(t.max(0) as u64).saturating_sub(since)
        });
        Some(due.min(minute))
    }

    /// Starts `r` now, or fails it saying why.
    async fn start(&mut self, mut r: Recording) {
        if let Err(note) = self.free_connection(&r.provider_id) {
            return self.fail(r.id, &note, false).await;
        }
        // A download gives up its connection to a recording.
        self.make_room(&r.provider_id, 1);
        let (provider_id, stream_id) = (r.provider_id.clone(), r.stream_id);
        let extension = self
            .db(move |c| store::channel_extension(c, &provider_id, stream_id))
            .await;
        let Some(extension) = extension else {
            return; // Logged; tried again on the next check.
        };
        let Some(extension) = extension else {
            let note = format!("{} is no longer in the provider's list", r.channel);
            return self.fail(r.id, &note, false).await;
        };
        let Some(client) = self.clients.get(&r.provider_id) else {
            let note = "the provider is no longer in the credentials file";
            return self.fail(r.id, note, false).await;
        };
        let url = match client.stream_url(r.stream_id, &extension) {
            Ok(url) => url,
            Err(e) => return self.fail(r.id, &e, false).await,
        };
        let name = recording_name(&r.channel, r.title.as_deref(), r.start);
        let file = match reserve_recording(&self.setup.dir, &name) {
            Ok(file) => file,
            Err(e) => {
                let note = format!("can't record to {}: {e}", self.setup.dir.display());
                return self.fail(r.id, &note, false).await;
            }
        };
        let child = match spawn(&self.setup.ffmpeg, url.as_str(), &file) {
            Ok(child) => child,
            Err(e) => {
                let _ = std::fs::remove_file(&file);
                let note = format!("cannot start {}: {e}", self.setup.ffmpeg);
                return self.fail(r.id, &note, false).await;
            }
        };
        log::info!("recording {} to {}", r.channel, file.display());
        let path = file.display().to_string();
        let id = r.id;
        self.db(move |c| store::recording_started(c, id, &path))
            .await;
        let (stop, stopped) = oneshot::channel();
        self.running.insert(
            id,
            Running {
                provider_id: r.provider_id.clone(),
                stop_at: r.padded_stop,
                stop: Some(stop),
                cancelled: false,
            },
        );
        tokio::spawn(supervise(child, stopped, id, self.exits.clone()));
        r.state = RecState::Recording;
        r.file = Some(file.display().to_string());
        let _ = self.events.send(Event::Started(r));
    }

    /// Whether `provider_id` has a connection free for one more recording:
    /// mpv's, if it plays from there, and the recordings running count
    /// against `max_connections`; a download does not, as it gives its
    /// connection up. An unknown limit counts as 1.
    fn free_connection(&self, provider_id: &str) -> Result<(), String> {
        let max = self.limits.get(provider_id).copied().flatten();
        let watching = self.watching.as_deref() == Some(provider_id);
        let recording = self
            .running
            .values()
            .filter(|r| r.provider_id == provider_id)
            .count();
        let used = usize::from(watching) + recording;
        let allowed = max.filter(|&m| m > 0).map_or(1, |m| m as usize);
        if used < allowed {
            return Ok(());
        }
        let mut using = Vec::new();
        if watching {
            using.push("watching".to_string());
        }
        if recording > 0 {
            let s = if recording == 1 { "" } else { "s" };
            using.push(format!("{recording} recording{s}"));
        }
        let limit = match max.filter(|&m| m > 0) {
            Some(m) => m.to_string(),
            None => "not known, so 1 is assumed".into(),
        };
        Err(format!(
            "no free connection (the account's limit is {limit}; in use: {})",
            using.join(", ")
        ))
    }

    async fn exited(&mut self, id: i64, exit: Exit) {
        let Some(r) = self.running.remove(&id) else {
            return;
        };
        match exit {
            Exit::Stopped if r.cancelled => {
                self.db(move |c| {
                    store::set_recording_state(c, id, RecState::Cancelled, None, false)
                })
                .await;
                self.report(id, Event::Ended).await;
            }
            Exit::Stopped => {
                self.db(move |c| store::set_recording_state(c, id, RecState::Done, None, false))
                    .await;
                self.report(id, Event::Ended).await;
            }
            Exit::Early(line) => {
                log::warn!("recording {id} failed: {line}");
                self.fail(id, &line, true).await;
            }
        }
    }

    /// Fails recording `id` with `note`. With `ran`, removes its file if
    /// nothing was written.
    async fn fail(&mut self, id: i64, note: &str, ran: bool) {
        let note = xtream::redact(note);
        let mut empty = false;
        if ran {
            let file = self.db(move |c| store::recording(c, id)).await.flatten();
            if let Some(file) = file.and_then(|r| r.file) {
                empty = std::fs::metadata(&file).is_ok_and(|m| m.len() == 0);
                if empty {
                    let _ = std::fs::remove_file(&file);
                }
            }
        }
        self.db(move |c| store::set_recording_state(c, id, RecState::Failed, Some(&note), empty))
            .await;
        self.report(id, Event::Ended).await;
    }

    /// Sends `event` with recording `id` as stored.
    async fn report(&self, id: i64, event: fn(Recording) -> Event) {
        if let Some(Some(r)) = self.db(move |c| store::recording(c, id)).await {
            let _ = self.events.send(event(r));
        }
    }

    /// Stops every recording and keeps each as done, cut short. mpv, which
    /// the app closes, ends its recording of what plays.
    async fn quit(
        &mut self,
        exited: &mut mpsc::UnboundedReceiver<(i64, Exit)>,
        dl_rx: &mut mpsc::UnboundedReceiver<(i64, DlMsg)>,
    ) {
        for f in self.fetching.values_mut() {
            f.halt = Halt::Quit;
            if let Some(stop) = f.stop.take() {
                let _ = stop.send(());
            }
        }
        if let Some((id, cancelled)) = self.playing.take() {
            let (state, note) = match cancelled {
                true => (RecState::Cancelled, None),
                false => (RecState::Done, Some(QUIT_NOTE.to_string())),
            };
            self.end_playing(id, state, note).await;
        }
        for r in self.running.values_mut() {
            if let Some(stop) = r.stop.take() {
                let _ = stop.send(());
            }
        }
        while !self.running.is_empty() {
            let Ok(Some((id, _))) = tokio::time::timeout(STOP_WAIT * 2, exited.recv()).await else {
                break;
            };
            let state = match self.running.remove(&id) {
                Some(r) if r.cancelled => RecState::Cancelled,
                _ => RecState::Done,
            };
            let note = (state == RecState::Done).then_some(QUIT_NOTE);
            self.db(move |c| store::set_recording_state(c, id, state, note, false))
                .await;
        }
        // Each download stops as soon as asked; its part resumes next time.
        while !self.fetching.is_empty() {
            let Ok(Some((id, msg))) = tokio::time::timeout(STOP_WAIT, dl_rx.recv()).await else {
                break;
            };
            if let DlMsg::Exit(download::Exit::Done) = msg {
                // Finished as it was stopped: kept as done.
                if let Some(p) = self
                    .fetching
                    .iter()
                    .find(|(_, f)| f.id == id)
                    .map(|(p, _)| p.clone())
                {
                    self.fetching.remove(&p);
                }
                if let Some(Some(d)) = self.db(move |c| store::download(c, id)).await {
                    self.dl_done(d).await;
                }
            } else if let DlMsg::Exit(_) = msg {
                self.fetching.retain(|_, f| f.id != id);
            }
        }
        self.db(store::settle_downloads).await;
    }

    /// Runs `f` on the writer connection (SPEC 3), off the async threads, as
    /// an import can hold it for seconds. `None` on an error, logged.
    async fn db<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Option<T> {
        let writer = self.writer.clone();
        let done = tokio::task::spawn_blocking(move || {
            let conn = writer.lock().unwrap_or_else(|e| e.into_inner());
            f(&conn)
        })
        .await;
        match done {
            Ok(Ok(t)) => Some(t),
            Ok(Err(e)) => {
                log::error!("saving recordings: {e}");
                None
            }
            Err(e) => {
                log::error!("recordings task failed: {e}");
                None
            }
        }
    }
}

/// What `Delete` would do with recording `r`: move its file, here (`Some`)
/// or forget a row with no file (`None`), or refuse it, `Elsewhere` when
/// its file is outside `dir`.
pub fn deletable(r: &Recording, dir: &Path) -> Result<Option<PathBuf>, Deleted> {
    if matches!(r.state, RecState::Scheduled | RecState::Recording) {
        return Err(Deleted::NotFinished);
    }
    let Some(file) = r.file.as_deref().map(Path::new).filter(|f| f.exists()) else {
        return Ok(None);
    };
    within(file, dir).map(Some).ok_or(Deleted::Elsewhere)
}

/// `file` with its folders resolved, if it is under `dir`. The folders,
/// not the file: a link is trashed as a link.
fn within(file: &Path, dir: &Path) -> Option<PathBuf> {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let file = canon(file.parent()?).join(file.file_name()?);
    file.starts_with(canon(dir)).then_some(file)
}

/// What `DeleteDownload` would do with download `d`: move its file, here
/// (`Some`), or forget a failed one or one with no file (`None`), or
/// refuse it, `Elsewhere` when its file is outside `dir`.
pub fn download_deletable(d: &Download, dir: &Path) -> Result<Option<PathBuf>, Deleted> {
    match d.state {
        DlState::Queued | DlState::Downloading => return Err(Deleted::NotFinished),
        DlState::Failed => return Ok(None),
        DlState::Done => {}
    }
    let Some(file) = d.file.as_deref().map(Path::new).filter(|f| f.exists()) else {
        return Ok(None);
    };
    within(file, dir).map(Some).ok_or(Deleted::Elsewhere)
}

/// Moves done download `d`'s file to the trash at `bin`, if it is under
/// `dir`; removes a failed one's part.
fn trash_download(d: &Download, dir: &Path, bin: &Path) -> Deleted {
    let file = match download_deletable(d, dir) {
        Ok(Some(file)) => file,
        Ok(None) => {
            remove_part(d);
            return Deleted::NoFile;
        }
        Err(refused) => return refused,
    };
    trash_to(&file, bin)
}

/// Removes download `d`'s part, if it has one.
fn remove_part(d: &Download) {
    if let Some(part) = d.part() {
        let _ = std::fs::remove_file(part);
    }
}

/// Moves finished recording `r`'s file to the trash at `bin`, if it has
/// one and it is under `dir`.
fn trash_file(r: &Recording, dir: &Path, bin: &Path) -> Deleted {
    let file = match deletable(r, dir) {
        Ok(Some(file)) => file,
        Ok(None) => return Deleted::NoFile,
        Err(refused) => return refused,
    };
    trash_to(&file, bin)
}

/// Moves `file` to the trash at `bin`.
fn trash_to(file: &Path, bin: &Path) -> Deleted {
    match trash::trash(file, bin, unix_now()) {
        Ok(_) => Deleted::Trashed,
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            Deleted::Failed("it is on another filesystem than the trash".into())
        }
        Err(e) => Deleted::Failed(e.to_string()),
    }
}

async fn sleep(wait: Option<Duration>) {
    match wait {
        Some(wait) => tokio::time::sleep(wait).await,
        None => std::future::pending().await,
    }
}

/// `ffmpeg -i <url> -c copy` to `file`, which exists already (reserved).
/// Its output is errors only: they can quote the URL, so are redacted
/// before they are kept.
fn spawn(ffmpeg: &str, url: &str, file: &Path) -> std::io::Result<Child> {
    let mut cmd = Process::new(ffmpeg);
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i", url])
        .args(["-c", "copy", "-f", "mpegts", "-y"])
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Stopped with the app even if it dies without cleanup, as mpv is.
    // SAFETY: prctl is async-signal-safe and touches no shared state.
    unsafe {
        cmd.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()
}

/// Waits for ffmpeg to exit by itself, or for `stop` (or the recorder
/// gone): then SIGTERM, which ffmpeg answers by finishing the file, and a
/// kill if it takes too long. Reports how it ended.
async fn supervise(
    mut child: Child,
    stop: oneshot::Receiver<()>,
    id: i64,
    exits: mpsc::UnboundedSender<(i64, Exit)>,
) {
    let stderr = child.stderr.take();
    let last = tokio::spawn(async move {
        let mut last = String::new();
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    last = line.trim().to_string();
                }
            }
        }
        last
    });
    let exit = tokio::select! {
        status = child.wait() => {
            let last = last.await.unwrap_or_default();
            Exit::Early(match (last.is_empty(), status) {
                (false, _) => last,
                (true, Ok(status)) => format!("ffmpeg ended early ({status})"),
                (true, Err(e)) => format!("ffmpeg ended early: {e}"),
            })
        }
        _ = stop => {
            if let Some(pid) = child.id() {
                // SAFETY: a plain syscall on our own child, not yet reaped.
                unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            }
            if tokio::time::timeout(STOP_WAIT, child.wait()).await.is_err() {
                log::warn!("ffmpeg did not stop within {} s; killing it", STOP_WAIT.as_secs());
                let _ = child.kill().await;
            }
            Exit::Stopped
        }
    };
    let _ = exits.send((id, exit));
}

/// `s`'s first `max` characters for a file name, trimmed. Characters a
/// file name cannot hold, here or on a FAT drive it may be copied to,
/// become `-`.
pub fn name_part(s: &str, max: usize) -> String {
    s.chars()
        .take(max)
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// A recording's file name: `Channel - Programme - 2026-10-01 20.00.ts`,
/// without the programme when there is no guide.
pub fn recording_name(channel: &str, programme: Option<&str>, start: i64) -> String {
    let tm = crate::ui::local_tm(start);
    let mut name = name_part(channel, 80);
    if let Some(p) = programme {
        name = format!("{name} - {}", name_part(p, 100));
    }
    format!(
        "{name} - {}-{:02}-{:02} {:02}.{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min
    )
}

/// Creates `<dir>/<name>.ts`, or `<name> (2).ts` and on if taken, making
/// `dir` first. Creating it reserves the name and shows `dir` can be written.
pub fn reserve_recording(dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    for n in 1.. {
        let file = match n {
            1 => dir.join(format!("{name}.ts")),
            n => dir.join(format!("{name} ({n}).ts")),
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)
        {
            Ok(_) => return Ok(file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Provider;
    use crate::fake_xtream::{FakeFile, FileOpts};
    use crate::xtream::Client;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::sync::Mutex;

    /// Records until stopped: notes its arguments, writes to the output
    /// file (its last argument), and notes the SIGTERM that stops it.
    const RECORDS: &str = "#!/bin/sh
printf '%s\\n' \"$@\" > \"$0.args\"
for a; do out=$a; done
trap 'echo stopped >> \"$0.args\"; exit 255' TERM
echo data > \"$out\"
while :; do sleep 0.05; done
";

    /// Fails as ffmpeg does on a missing stream, quoting the URL.
    const FAILS: &str = "#!/bin/sh
echo 'http://h:8080/live/user/secret/1.ts: Server returned 404 Not Found' >&2
exit 1
";

    /// A recorder with a fake ffmpeg, provider `p` and its channel 1.
    struct Fake {
        dir: tempfile::TempDir,
        writer: Writer,
        commands: mpsc::UnboundedSender<Command>,
        events: mpsc::UnboundedReceiver<Event>,
    }

    impl Fake {
        /// Starts the recorder with no padding, after `before` has set up
        /// the store as if from an earlier run.
        fn new(ffmpeg: &str, before: impl FnOnce(&Connection)) -> Self {
            Self::padded(ffmpeg, 0, 0, before)
        }

        fn padded(
            ffmpeg: &str,
            pad_before: i64,
            after: i64,
            before: impl FnOnce(&Connection),
        ) -> Self {
            Self::at(ffmpeg, pad_before, after, "http://127.0.0.1:9", before)
        }

        /// A recorder whose provider is at `url`, for downloads.
        fn serving(url: &str) -> Self {
            Self::at(RECORDS, 0, 0, url, |_| {})
        }

        /// The same in `dir`, as a second run of the app.
        fn again(dir: tempfile::TempDir, url: &str) -> Self {
            Self::start(dir, RECORDS, 0, 0, url, |_| {})
        }

        fn at(
            ffmpeg: &str,
            pad_before: i64,
            after: i64,
            url: &str,
            before: impl FnOnce(&Connection),
        ) -> Self {
            Self::start(
                tempfile::tempdir().unwrap(),
                ffmpeg,
                pad_before,
                after,
                url,
                before,
            )
        }

        fn start(
            dir: tempfile::TempDir,
            ffmpeg: &str,
            pad_before: i64,
            after: i64,
            url: &str,
            before: impl FnOnce(&Connection),
        ) -> Self {
            let script = dir.path().join("ffmpeg");
            std::fs::write(&script, ffmpeg).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
            let conn = with_channel(dir.path());
            before(&conn);
            let writer: Writer = Arc::new(Mutex::new(conn));
            let provider = Provider {
                id: "p".into(),
                name: "P".into(),
                url: url.into(),
                username: "user".into(),
                password: "secret".into(),
                epg: Vec::new(),
                source: crate::config::Source::Xtream,
                user_agent: None,
            };
            let clients = [("p".to_string(), Arc::new(Client::new(provider)))]
                .into_iter()
                .collect();
            let setup = Setup {
                ffmpeg: script.display().to_string(),
                dir: dir.path().join("rec"),
                trash: dir.path().join("Trash"),
                before: pad_before,
                after,
                downloads: dir.path().join("Downloads"),
            };
            let (tx, rx) = mpsc::unbounded_channel();
            let (events_tx, events) = mpsc::unbounded_channel();
            tokio::spawn(Recorder::run(writer.clone(), clients, setup, events_tx, rx));
            Self {
                dir,
                writer,
                commands: tx,
                events,
            }
        }

        fn send(&self, command: Command) {
            self.commands.send(command).unwrap();
        }

        /// The next event after any `Scheduled`, which every schedule sends.
        async fn event(&mut self) -> Event {
            loop {
                match self.any_event().await {
                    Event::Scheduled(_) => {}
                    e => return e,
                }
            }
        }

        async fn any_event(&mut self) -> Event {
            tokio::time::timeout(Duration::from_secs(5), self.events.recv())
                .await
                .expect("no event within 5 s")
                .unwrap()
        }

        fn row(&self, id: i64) -> Recording {
            store::recording(&self.writer.lock().unwrap(), id)
                .unwrap()
                .unwrap()
        }

        fn args(&self) -> String {
            std::fs::read_to_string(self.dir.path().join("ffmpeg.args")).unwrap_or_default()
        }
    }

    /// A database in `dir` with provider `p`'s channel 1.
    fn with_channel(dir: &Path) -> Connection {
        let mut conn = store::open(&dir.join("v.db")).unwrap();
        let tx = conn.transaction().unwrap();
        store::begin_channel_import(&tx, "p", "P", []).unwrap();
        let c = store::NewChannel {
            stream_id: 1,
            num: Some(1),
            name: "Northwind Sport 1",
            category_id: None,
            epg_channel_id: Some("northwind1"),
            icon_url: None,
            extension: "ts",
            archive_days: 0,
            url: None,
            tvg_name: None,
        };
        store::insert_channel(&tx, "p", &c).unwrap();
        store::finish_channel_import(&tx, "p", 0).unwrap();
        tx.commit().unwrap();
        conn
    }

    /// Football Tonight on channel 1, from `start` to `stop`.
    fn programme(start: i64, stop: i64) -> NewRecording {
        NewRecording {
            provider_id: "p".into(),
            stream_id: 1,
            channel: "Northwind Sport 1".into(),
            guide_id: Some("northwind1".into()),
            title: Some("Football Tonight".into()),
            start,
            stop,
        }
    }

    /// Waits for the fake to write `file`, by when its SIGTERM trap is set.
    async fn written(file: &str) {
        while std::fs::metadata(file).unwrap().len() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn ended(event: Event) -> Recording {
        match event {
            Event::Ended(r) => r,
            e => panic!("expected Ended, got {e:?}"),
        }
    }

    fn started(event: Event) -> Recording {
        match event {
            Event::Started(r) => r,
            e => panic!("expected Started, got {e:?}"),
        }
    }

    #[tokio::test]
    async fn records_from_the_padded_start_to_the_padded_stop() {
        let mut f = Fake::new(RECORDS, |_| {});
        let now = unix_now();
        // Starts on its own timer, with no command after scheduling.
        f.send(Command::Schedule(programme(now + 1, now + 2)));
        let r = started(f.event().await);
        assert!(unix_now() > now);
        assert_eq!(r.state, RecState::Recording);
        let file = PathBuf::from(r.file.unwrap());
        assert_eq!(file.parent(), Some(f.dir.path().join("rec").as_path()));
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("Northwind Sport 1 - Football Tonight - "),
            "{name}"
        );
        let r = ended(f.event().await);
        assert!(unix_now() >= now + 2);
        assert_eq!((r.state, r.note), (RecState::Done, None));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "data\n");
        let args = f.args();
        let args: Vec<&str> = args.lines().collect();
        assert_eq!(
            args[..6],
            [
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                "http://127.0.0.1:9/live/user/secret/1.ts"
            ]
        );
        assert_eq!(args[6..9], ["-c", "copy", "-f"]);
        assert_eq!(args.last(), Some(&"stopped"), "stopped with SIGTERM");
    }

    #[tokio::test]
    async fn padding_comes_from_the_settings() {
        let mut f = Fake::padded(RECORDS, 120, 300, |_| {});
        let start = unix_now() + 3600;
        f.send(Command::Schedule(programme(start, start + 1800)));
        f.send(Command::Cancel(1));
        let r = ended(f.event().await);
        assert_eq!((r.start, r.stop), (start, start + 1800));
        assert_eq!((r.padded_start, r.padded_stop), (start - 120, start + 2100));
        assert_eq!(r.state, RecState::Cancelled);
        assert!(f.args().is_empty(), "never started");
    }

    #[tokio::test]
    async fn ffmpeg_ending_early_fails_with_its_last_line_redacted() {
        let mut f = Fake::new(FAILS, |_| {});
        let now = unix_now();
        f.send(Command::Schedule(programme(now - 60, now + 60)));
        let file = started(f.event().await).file.unwrap();
        let r = ended(f.event().await);
        assert_eq!(r.state, RecState::Failed);
        let note = r.note.unwrap();
        assert!(note.contains("404 Not Found"), "{note}");
        assert!(!note.contains("secret"), "{note}");
        // Nothing was written: the reserved file is gone, and forgotten.
        assert!(!Path::new(&file).exists());
        assert_eq!(r.file, None);
    }

    #[tokio::test]
    async fn ffmpeg_that_cannot_start_fails() {
        let mut f = Fake::new(RECORDS, |_| {});
        std::fs::remove_file(f.dir.path().join("ffmpeg")).unwrap();
        let now = unix_now();
        f.send(Command::Schedule(programme(now - 60, now + 60)));
        let r = ended(f.event().await);
        assert_eq!(r.state, RecState::Failed);
        assert!(r.note.unwrap().starts_with("cannot start"));
        assert!(
            std::fs::read_dir(f.dir.path().join("rec"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_start_over_the_connection_limit_is_refused() {
        let mut f = Fake::new(RECORDS, |_| {});
        let now = unix_now();
        // An unknown limit counts as 1, which mpv takes.
        f.send(Command::Watching(Some("p".into())));
        f.send(Command::Schedule(programme(now - 60, now + 60)));
        let r = ended(f.event().await);
        assert_eq!(r.state, RecState::Failed);
        let note = r.note.unwrap();
        assert!(
            note.contains("not known") && note.contains("watching"),
            "{note}"
        );

        // Two allowed: mpv and one recording, then refused again.
        f.send(Command::Limit("p".into(), Some(2)));
        f.send(Command::Schedule(programme(now - 60, now + 60)));
        assert_eq!(started(f.event().await).id, 2);
        // Other programmes, as one recording already is not scheduled again.
        f.send(Command::Schedule(programme(now - 30, now + 60)));
        let r = ended(f.event().await);
        assert_eq!((r.id, r.state), (3, RecState::Failed));
        let note = r.note.unwrap();
        assert!(
            note.contains("limit is 2; in use: watching, 1 recording"),
            "{note}"
        );
        // Not retried silently.
        assert_eq!(f.row(3).state, RecState::Failed);

        // Another provider's playback takes none of these.
        f.send(Command::Watching(Some("q".into())));
        f.send(Command::Schedule(programme(now - 20, now + 60)));
        assert_eq!(started(f.event().await).id, 4);
    }

    #[tokio::test]
    async fn launch_misses_what_passed_and_starts_what_is_on_late() {
        let now = unix_now();
        let mut f = Fake::new(RECORDS, |c| {
            // Passed while closed, on now, and left running by a crash.
            store::schedule_recording(c, &programme(now - 7200, now - 3600), 0, 0).unwrap();
            store::schedule_recording(c, &programme(now - 600, now + 600), 0, 0).unwrap();
            store::schedule_recording(c, &programme(now - 900, now + 900), 0, 0).unwrap();
            store::recording_started(c, 3, "/x.ts").unwrap();
        });
        let r = started(f.event().await);
        assert_eq!(r.id, 2);
        assert_eq!(f.event().await, Event::Missed(1));
        assert_eq!(r.padded_stop, now + 600, "late, to its own stop");
        assert_eq!(f.row(1).state, RecState::Missed);
        let crashed = f.row(3);
        assert_eq!(crashed.state, RecState::Done);
        assert_eq!(crashed.note.as_deref(), Some(CRASH_NOTE));
    }

    #[tokio::test]
    async fn cancel_drops_a_schedule_or_stops_a_recording_keeping_it() {
        let mut f = Fake::new(RECORDS, |_| {});
        let now = unix_now();
        f.send(Command::Schedule(programme(now - 60, now + 60)));
        let file = started(f.event().await).file.unwrap();
        written(&file).await;
        f.send(Command::Cancel(1));
        let r = ended(f.event().await);
        assert_eq!(r.state, RecState::Cancelled);
        assert_eq!(std::fs::read_to_string(file).unwrap(), "data\n");
        assert!(f.args().ends_with("stopped\n"));

        f.send(Command::Schedule(programme(now + 600, now + 1200)));
        f.send(Command::Cancel(2));
        assert_eq!(ended(f.event().await).state, RecState::Cancelled);
        // Cancelling again, or something gone, does nothing.
        f.send(Command::Cancel(2));
        f.send(Command::Cancel(9));
        f.send(Command::Schedule(programme(now - 60, now + 60)));
        assert_eq!(started(f.event().await).id, 3);
    }

    /// Onboarding ticket 02: removing the provider stops its recording,
    /// kept as cancelled with its file, and cancels its schedules, before
    /// its data goes.
    #[tokio::test]
    async fn removing_the_provider_cancels_its_recordings() {
        let mut f = Fake::new(RECORDS, |_| {});
        let now = unix_now();
        f.send(Command::Schedule(programme(now - 60, now + 60)));
        let file = started(f.event().await).file.unwrap();
        written(&file).await;
        f.send(Command::Schedule(programme(now + 600, now + 1200)));
        f.send(Command::RemoveProvider {
            provider_id: "p".into(),
            personal: false,
        });
        // In either order: the recording ends on its own task.
        let (mut ended, mut removed) = (None, false);
        while ended.is_none() || !removed {
            match f.event().await {
                Event::Ended(r) => ended = Some(r),
                Event::ProviderRemoved(id, ok) => removed = id == "p" && ok,
                e => panic!("unexpected {e:?}"),
            }
        }
        let r = ended.unwrap();
        assert_eq!((r.id, r.state), (1, RecState::Cancelled));
        assert_eq!(std::fs::read_to_string(file).unwrap(), "data\n");
        assert_eq!(f.row(2).state, RecState::Cancelled);
        let channels: i64 = f
            .writer
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM channel", [], |r| r.get(0))
            .unwrap();
        assert_eq!(channels, 0);
    }

    #[tokio::test]
    async fn a_schedule_is_reported_and_never_doubled() {
        let mut f = Fake::padded(RECORDS, 120, 300, |_| {});
        let start = unix_now() + 3600;
        f.send(Command::Schedule(programme(start, start + 1800)));
        let Event::Scheduled(r) = f.any_event().await else {
            panic!("expected Scheduled");
        };
        assert_eq!((r.id, r.state), (1, RecState::Scheduled));
        assert_eq!((r.padded_start, r.padded_stop), (start - 120, start + 2100));
        // A second `R` before the first was stored adds nothing.
        f.send(Command::Schedule(programme(start, start + 1800)));
        f.send(Command::Cancel(1));
        assert_eq!(ended(f.any_event().await).id, 1);
        f.send(Command::Schedule(programme(start, start + 1800)));
        let Event::Scheduled(r) = f.any_event().await else {
            panic!("expected Scheduled");
        };
        assert_eq!(r.id, 2, "a cancelled one can be scheduled again");
    }

    #[tokio::test]
    async fn an_airing_programme_starts_at_once_to_its_padded_stop() {
        let mut f = Fake::padded(RECORDS, 120, 300, |_| {});
        let now = unix_now();
        f.send(Command::Schedule(programme(now - 600, now + 600)));
        let Event::Scheduled(r) = f.any_event().await else {
            panic!("expected Scheduled");
        };
        assert_eq!(r.padded_stop, now + 900);
        let r = started(f.any_event().await);
        assert_eq!((r.id, r.padded_stop), (1, now + 900));
    }

    #[tokio::test]
    async fn quitting_stops_recordings_and_keeps_them_as_done() {
        let mut f = Fake::new(RECORDS, |_| {});
        let now = unix_now();
        f.send(Command::Schedule(programme(now - 60, now + 60)));
        written(&started(f.event().await).file.unwrap()).await;
        let (done, quit) = oneshot::channel();
        f.send(Command::Quit(done));
        tokio::time::timeout(Duration::from_secs(5), quit)
            .await
            .unwrap()
            .unwrap();
        let r = f.row(1);
        assert_eq!(r.state, RecState::Done);
        assert_eq!(r.note.as_deref(), Some(QUIT_NOTE));
        assert!(f.args().ends_with("stopped\n"));
    }

    /// What plays on channel 1 from `start`, as the app sends it.
    fn playing(start: i64) -> NewRecording {
        NewRecording {
            guide_id: None,
            title: None,
            ..programme(start, start)
        }
    }

    #[tokio::test]
    async fn recordings_of_what_plays_are_stored_and_ended() {
        let mut f = Fake::new(RECORDS, |_| {});
        let file = f.dir.path().join("a.ts");
        let now = unix_now();

        // Done: stored as recording, open-ended, then stopped where it ended.
        std::fs::write(&file, "data").unwrap();
        f.send(Command::Playing(playing(now - 60), file.clone()));
        assert_eq!(f.any_event().await, Event::Listed);
        let r = f.row(1);
        assert_eq!(
            (r.state, r.start, r.stop),
            (RecState::Recording, now - 60, now - 60)
        );
        assert_eq!(r.file.as_deref(), Some(file.to_str().unwrap()));
        f.send(Command::PlayingEnded(None));
        assert_eq!(f.any_event().await, Event::Listed);
        let r = f.row(1);
        assert_eq!((r.state, r.note), (RecState::Done, None));
        assert!(r.stop >= now && r.padded_stop == r.stop);

        // Failed with nothing written: the file is removed and forgotten.
        let empty = f.dir.path().join("b.ts");
        std::fs::write(&empty, "").unwrap();
        f.send(Command::Playing(playing(now), empty.clone()));
        f.send(Command::PlayingEnded(Some(
            "Failed opening output file.".into(),
        )));
        assert_eq!(f.any_event().await, Event::Listed);
        assert_eq!(f.any_event().await, Event::Listed);
        let r = f.row(2);
        assert_eq!(r.state, RecState::Failed);
        assert_eq!(r.note.as_deref(), Some("Failed opening output file."));
        assert_eq!(r.file, None);
        assert!(!empty.exists());

        // An end with none running does nothing.
        f.send(Command::PlayingEnded(None));
        f.send(Command::Playing(playing(now), file.clone()));
        assert_eq!(f.any_event().await, Event::Listed);
        assert_eq!(f.row(3).state, RecState::Recording);
    }

    #[tokio::test]
    async fn cancel_on_each_state() {
        let now = unix_now();
        let mut f = Fake::new(RECORDS, |c| {
            // Finished already: done, failed, missed, cancelled.
            for (i, state) in [
                RecState::Done,
                RecState::Failed,
                RecState::Missed,
                RecState::Cancelled,
            ]
            .into_iter()
            .enumerate()
            {
                let start = now - 7200 - i as i64 * 3600;
                let id =
                    store::schedule_recording(c, &programme(start, start + 600), 0, 0).unwrap();
                store::set_recording_state(c, id, state, None, false).unwrap();
            }
        });
        // Finished ones stay as they are.
        for id in 1..=4 {
            f.send(Command::Cancel(id));
        }
        // Scheduled: cancelled, never started.
        f.send(Command::Schedule(programme(now + 600, now + 1200)));
        f.send(Command::Cancel(5));
        assert_eq!(ended(f.event().await).state, RecState::Cancelled);
        let states: Vec<RecState> = (1..=4).map(|id| f.row(id).state).collect();
        assert_eq!(
            states,
            [
                RecState::Done,
                RecState::Failed,
                RecState::Missed,
                RecState::Cancelled
            ]
        );

        // Recording on its own connection: stopped, cancelled, file kept.
        f.send(Command::Schedule(programme(now - 60, now + 60)));
        let file = started(f.event().await).file.unwrap();
        written(&file).await;
        f.send(Command::Cancel(6));
        let r = ended(f.event().await);
        assert_eq!((r.id, r.state), (6, RecState::Cancelled));
        assert!(Path::new(&file).exists());

        // Recording what plays: mpv, which the app stops, ends it; then
        // cancelled, file kept.
        let file = f.dir.path().join("a.ts");
        std::fs::write(&file, "data").unwrap();
        f.send(Command::Playing(playing(now), file.clone()));
        assert_eq!(f.any_event().await, Event::Listed);
        f.send(Command::Cancel(7));
        f.send(Command::PlayingEnded(None));
        assert_eq!(f.any_event().await, Event::Listed);
        let r = f.row(7);
        assert_eq!((r.state, r.note), (RecState::Cancelled, None));
        assert!(file.exists());
    }

    /// A finished recording in `state` from an hour ago, writing to `file`
    /// if any. Returns its id.
    fn finished(f: &Fake, state: RecState, file: Option<&Path>) -> i64 {
        let c = f.writer.lock().unwrap();
        let now = unix_now();
        let id = store::schedule_recording(&c, &programme(now - 3600, now - 600), 0, 0).unwrap();
        if let Some(file) = file {
            store::recording_started(&c, id, file.to_str().unwrap()).unwrap();
        }
        store::set_recording_state(&c, id, state, None, false).unwrap();
        id
    }

    fn deleted(event: Event) -> (i64, Deleted) {
        match event {
            Event::Deleted(r, d) => (r.id, d),
            e => panic!("expected Deleted, got {e:?}"),
        }
    }

    fn exists(f: &Fake, id: i64) -> bool {
        store::recording(&f.writer.lock().unwrap(), id)
            .unwrap()
            .is_some()
    }

    #[tokio::test]
    async fn deleting_moves_the_file_to_the_trash_and_hides_it() {
        let mut f = Fake::new(RECORDS, |_| {});
        let rec = f.dir.path().join("rec");
        std::fs::create_dir(&rec).unwrap();
        let file = rec.join("Northwind Sport 1 - Football Tonight.ts");
        std::fs::write(&file, "data").unwrap();
        let id = finished(&f, RecState::Done, Some(&file));
        f.send(Command::Delete(id));
        assert_eq!(deleted(f.any_event().await), (id, Deleted::Trashed));
        assert!(!file.exists() && f.row(id).trashed);
        assert!(!store::recording_shown(&f.row(id)));
        let first = id;
        let bin = f.dir.path().join("Trash");
        let name = "Northwind Sport 1 - Football Tonight.ts";
        assert_eq!(
            std::fs::read_to_string(bin.join("files").join(name)).unwrap(),
            "data"
        );
        let info = std::fs::read_to_string(bin.join(format!("info/{name}.trashinfo"))).unwrap();
        let path = file.to_str().unwrap().replace(' ', "%20");
        assert!(info.contains(&format!("\nPath={path}\n")), "{info}");

        // A cancelled one that kept its file goes the same way. Taking the
        // trashed one's name forgets that row, which would otherwise show
        // the new file as its own.
        std::fs::write(&file, "more").unwrap();
        let id = finished(&f, RecState::Cancelled, Some(&file));
        assert!(!exists(&f, first));
        f.send(Command::Delete(id));
        assert_eq!(deleted(f.any_event().await), (id, Deleted::Trashed));
        let trashed = bin.join("files/Northwind Sport 1 - Football Tonight (2).ts");
        assert!(trashed.exists());

        // Restored from the trash: shown again.
        std::fs::rename(&trashed, &file).unwrap();
        assert!(store::recording_shown(&f.row(id)));
    }

    #[tokio::test]
    async fn deleting_refuses_what_is_not_finished_or_not_in_the_folder() {
        let mut f = Fake::new(RECORDS, |_| {});
        let now = unix_now();

        // Outside `recordings_dir`, as after the setting changed.
        let elsewhere = f.dir.path().join("a.ts");
        std::fs::write(&elsewhere, "data").unwrap();
        let id = finished(&f, RecState::Done, Some(&elsewhere));
        f.send(Command::Delete(id));
        assert_eq!(deleted(f.any_event().await), (id, Deleted::Elsewhere));
        assert!(elsewhere.exists() && exists(&f, id));

        // Scheduled, and recording.
        f.send(Command::Schedule(programme(now + 600, now + 1200)));
        let scheduled = match f.any_event().await {
            Event::Scheduled(r) => r.id,
            e => panic!("expected Scheduled, got {e:?}"),
        };
        f.send(Command::Delete(scheduled));
        assert_eq!(
            deleted(f.any_event().await),
            (scheduled, Deleted::NotFinished)
        );
        f.send(Command::Schedule(programme(now - 60, now + 60)));
        let r = started(f.event().await);
        written(r.file.as_deref().unwrap()).await;
        f.send(Command::Delete(r.id));
        assert_eq!(deleted(f.any_event().await), (r.id, Deleted::NotFinished));
        assert!(Path::new(r.file.as_deref().unwrap()).exists());
        assert!(exists(&f, scheduled) && exists(&f, r.id));
        assert!(!f.dir.path().join("Trash").exists());
    }

    #[tokio::test]
    async fn deleting_one_without_a_file_only_forgets_it() {
        let mut f = Fake::new(RECORDS, |_| {});
        let gone = f.dir.path().join("rec/gone.ts");
        for file in [None, Some(gone.as_path())] {
            let id = finished(&f, RecState::Missed, file);
            f.send(Command::Delete(id));
            assert_eq!(deleted(f.any_event().await), (id, Deleted::NoFile));
            assert!(!exists(&f, id));
        }
        // Even outside the folder: there is nothing to trash.
        let id = finished(&f, RecState::Done, Some(Path::new("/nonexistent/a.ts")));
        f.send(Command::Delete(id));
        assert_eq!(deleted(f.any_event().await), (id, Deleted::NoFile));
    }

    #[tokio::test]
    async fn a_failed_move_keeps_the_file_and_the_row() {
        let mut f = Fake::new(RECORDS, |_| {});
        let rec = f.dir.path().join("rec");
        std::fs::create_dir(&rec).unwrap();
        let file = rec.join("a.ts");
        std::fs::write(&file, "data").unwrap();
        // The trash cannot be made: a file is in its place.
        std::fs::write(f.dir.path().join("Trash"), "").unwrap();
        let id = finished(&f, RecState::Done, Some(&file));
        f.send(Command::Delete(id));
        let (_, d) = deleted(f.any_event().await);
        assert!(matches!(d, Deleted::Failed(_)), "{d:?}");
        assert!(file.exists() && exists(&f, id));
    }

    #[tokio::test]
    async fn clearing_counts_only_what_the_view_shows() {
        let mut f = Fake::new(RECORDS, |_| {});
        let rec = f.dir.path().join("rec");
        std::fs::create_dir(&rec).unwrap();
        let (a, b) = (rec.join("a.ts"), rec.join("b.ts"));
        for file in [&a, &b] {
            std::fs::write(file, "data").unwrap();
            let id = finished(&f, RecState::Done, Some(file));
            f.send(Command::Delete(id));
            assert_eq!(deleted(f.any_event().await), (id, Deleted::Trashed));
        }
        // `b` restored: shown, its file in the folder but not "kept" by
        // Clear's count, as it was trashed; `a` is hidden and not counted.
        std::fs::write(&b, "data").unwrap();
        finished(&f, RecState::Done, None);
        f.send(Command::ClearFinished);
        assert_eq!(f.any_event().await, Event::Cleared(2, 0));
    }

    #[tokio::test]
    async fn clearing_forgets_finished_recordings_only() {
        let now = unix_now();
        let mut f = Fake::new(RECORDS, |c| {
            for state in [
                RecState::Done,
                RecState::Failed,
                RecState::Missed,
                RecState::Cancelled,
            ] {
                let id =
                    store::schedule_recording(c, &programme(now - 7200, now - 3600), 0, 0).unwrap();
                // Only the done one has a file.
                if state == RecState::Done {
                    store::recording_started(c, id, "/x.ts").unwrap();
                }
                store::set_recording_state(c, id, state, None, false).unwrap();
            }
        });
        let file = f.dir.path().join("a.ts");
        std::fs::write(&file, "data").unwrap();
        f.send(Command::Schedule(programme(now + 600, now + 1200)));
        f.send(Command::Playing(playing(now), file.clone()));
        assert!(matches!(f.any_event().await, Event::Scheduled(_)));
        assert_eq!(f.any_event().await, Event::Listed);
        f.send(Command::ClearFinished);
        assert_eq!(f.any_event().await, Event::Cleared(4, 1));
        let left: Vec<RecState> = store::recordings(&f.writer.lock().unwrap())
            .unwrap()
            .iter()
            .map(|r| r.state)
            .collect();
        assert_eq!(left, [RecState::Scheduled, RecState::Recording]);
        assert!(file.exists());
    }

    #[tokio::test]
    async fn quitting_keeps_what_plays_as_done() {
        let mut f = Fake::new(RECORDS, |_| {});
        let file = f.dir.path().join("a.ts");
        std::fs::write(&file, "data").unwrap();
        f.send(Command::Playing(playing(unix_now()), file));
        assert_eq!(f.any_event().await, Event::Listed);
        let (done, quit) = oneshot::channel();
        f.send(Command::Quit(done));
        quit.await.unwrap();
        let r = f.row(1);
        assert_eq!(r.state, RecState::Done);
        assert_eq!(r.note.as_deref(), Some(QUIT_NOTE));
    }

    #[tokio::test]
    async fn a_channel_gone_from_the_list_fails() {
        let mut f = Fake::new(RECORDS, |_| {});
        let now = unix_now();
        let mut gone = programme(now - 60, now + 60);
        gone.stream_id = 2;
        f.send(Command::Schedule(gone));
        let r = ended(f.event().await);
        assert_eq!(r.state, RecState::Failed);
        assert!(r.note.unwrap().contains("no longer in the provider's list"));
    }

    #[test]
    fn recording_names_are_safe_and_never_taken() {
        // 2026-10-01 20:00 local time.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        (tm.tm_year, tm.tm_mon, tm.tm_mday, tm.tm_hour, tm.tm_isdst) = (126, 9, 1, 20, -1);
        let start = unsafe { libc::mktime(&mut tm) } as i64;
        assert_eq!(
            recording_name("Northwind Sport 1", Some("Football Tonight"), start),
            "Northwind Sport 1 - Football Tonight - 2026-10-01 20.00"
        );
        assert_eq!(
            recording_name("Harbour One", None, start + 65),
            "Harbour One - 2026-10-01 20.01"
        );
        assert_eq!(
            recording_name("4K: ALT|Coast/News", Some("Test Cricket: Day 2?\n"), start),
            "4K- ALT-Coast-News - Test Cricket- Day 2-- - 2026-10-01 20.00"
        );
        let long = "x".repeat(400);
        assert!(recording_name(&long, Some(&long), start).len() < 255 - " (99).ts".len());

        let dir = tempfile::tempdir().unwrap();
        let rec = dir.path().join("rec");
        let first = reserve_recording(&rec, "A - 2026-10-01 20.00").unwrap();
        assert_eq!(first, rec.join("A - 2026-10-01 20.00.ts"));
        let second = reserve_recording(&rec, "A - 2026-10-01 20.00").unwrap();
        assert_eq!(second, rec.join("A - 2026-10-01 20.00 (2).ts"));
        assert_eq!(
            reserve_recording(&rec, "A - 2026-10-01 20.00").unwrap(),
            rec.join("A - 2026-10-01 20.00 (3).ts")
        );
        // A directory that cannot be made is an error, not a panic.
        let blocked = dir.path().join("file");
        std::fs::write(&blocked, "").unwrap();
        assert!(reserve_recording(&blocked.join("sub"), "A").is_err());
    }

    /// Manual check against the demo provider and the real ffmpeg
    /// (`tools/demo/README.md`): with `demo_xtream.py 47811` running,
    /// `cargo test real_ffmpeg -- --ignored --nocapture`, then play the
    /// file it prints.
    #[tokio::test]
    #[ignore]
    async fn real_ffmpeg_records_the_demo_provider() {
        let dir = tempfile::tempdir().unwrap();
        let writer: Writer = Arc::new(Mutex::new(with_channel(dir.path())));
        let provider = Provider {
            id: "p".into(),
            name: "Demo".into(),
            url: "http://127.0.0.1:47811".into(),
            username: "demo".into(),
            password: "demo".into(),
            epg: Vec::new(),
            source: crate::config::Source::Xtream,
            user_agent: None,
        };
        let clients = [("p".to_string(), Arc::new(Client::new(provider)))]
            .into_iter()
            .collect();
        let setup = Setup {
            ffmpeg: "ffmpeg".into(),
            dir: std::env::temp_dir().join("vidicon-real-ffmpeg"),
            trash: std::env::temp_dir().join("vidicon-real-ffmpeg/Trash"),
            before: 0,
            after: 0,
            downloads: std::env::temp_dir().join("vidicon-real-ffmpeg/Downloads"),
        };
        let (commands, rx) = mpsc::unbounded_channel();
        let (events_tx, mut events) = mpsc::unbounded_channel();
        tokio::spawn(Recorder::run(writer, clients, setup, events_tx, rx));
        let now = unix_now();
        commands
            .send(Command::Schedule(programme(now - 1, now + 8)))
            .unwrap();
        let file = started(events.recv().await.unwrap()).file.unwrap();
        let r = tokio::time::timeout(Duration::from_secs(20), events.recv())
            .await
            .unwrap()
            .map(ended)
            .unwrap();
        assert_eq!((r.state, r.note), (RecState::Done, None));
        let size = std::fs::metadata(&file).unwrap().len();
        eprintln!("recorded {size} bytes to {file}");
        assert!(size > 0);
    }

    /// Movie `id`, `Film <id>`, to download.
    fn film(id: i64) -> NewDownload {
        NewDownload {
            provider_id: "p".into(),
            stream_id: id,
            series_id: None,
            season: None,
            episode_num: None,
            title: format!("Film {id}"),
            episode: None,
            extension: "mkv".into(),
        }
    }

    fn film_body() -> Vec<u8> {
        (0..40_000u32).map(|i| (i % 253) as u8).collect()
    }

    /// Sent in a second or so, slow enough to pause or quit part way.
    fn slow() -> FileOpts {
        FileOpts {
            chunk: 1_000,
            every: Duration::from_millis(25),
            ..Default::default()
        }
    }

    fn quick() -> FileOpts {
        FileOpts {
            chunk: 10_000,
            ..Default::default()
        }
    }

    impl Fake {
        /// The next event but progress.
        async fn dl_event(&mut self) -> Event {
            loop {
                match self.any_event().await {
                    Event::DlProgress { .. } => {}
                    e => return e,
                }
            }
        }

        /// No event but progress for `ms`.
        async fn quiet(&mut self, ms: u64) {
            let wait = Duration::from_millis(ms);
            loop {
                match tokio::time::timeout(wait, self.events.recv()).await {
                    Err(_) => return,
                    Ok(Some(Event::DlProgress { .. })) => {}
                    Ok(e) => panic!("expected nothing, got {e:?}"),
                }
            }
        }

        fn download(&self, id: i64) -> Option<Download> {
            store::download(&self.writer.lock().unwrap(), id).unwrap()
        }
    }

    fn dl_started(event: Event) -> (Download, bool) {
        match event {
            Event::DlStarted(d, resumed) => (d, resumed),
            e => panic!("expected DlStarted, got {e:?}"),
        }
    }

    fn dl_ended(event: Event) -> Download {
        match event {
            Event::DlEnded(d) => d,
            e => panic!("expected DlEnded, got {e:?}"),
        }
    }

    fn queued(added: usize, already: usize) -> Event {
        Event::DlQueued(store::Queued { added, already })
    }

    /// Saved settings apply to the next download and recording: its
    /// folder, ffmpeg and padding.
    #[tokio::test]
    async fn a_new_setup_applies_from_the_next_one() {
        let server = FakeFile::start(film_body(), quick()).await;
        let mut f = Fake::serving(server.url());
        let ffmpeg = f.dir.path().join("ffmpeg2");
        std::fs::write(&ffmpeg, RECORDS).unwrap();
        std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o700)).unwrap();
        f.send(Command::Setup(Setup {
            ffmpeg: ffmpeg.display().to_string(),
            dir: f.dir.path().join("rec2"),
            trash: f.dir.path().join("Trash"),
            before: 60,
            after: 120,
            downloads: f.dir.path().join("Downloads2"),
        }));
        f.send(Command::Download(vec![film(1)]));
        assert_eq!(f.dl_event().await, queued(1, 0));
        dl_started(f.dl_event().await);
        let d = dl_ended(f.dl_event().await);
        let file = PathBuf::from(d.file.unwrap());
        assert_eq!(file, f.dir.path().join("Downloads2/Film 1.mkv"));

        let now = unix_now();
        f.send(Command::Schedule(programme(now - 30, now + 30)));
        let r = started(f.event().await);
        assert_eq!((r.padded_start, r.padded_stop), (now - 90, now + 150));
        let file = r.file.unwrap();
        written(&file).await;
        assert_eq!(
            Path::new(&file).parent(),
            Some(f.dir.path().join("rec2").as_path())
        );
        assert!(f.dir.path().join("ffmpeg2.args").exists());
        assert!(f.args().is_empty(), "the first ffmpeg never ran");
        f.send(Command::Cancel(r.id));
        assert_eq!(ended(f.event().await).state, RecState::Cancelled);
    }

    #[tokio::test]
    async fn downloads_one_at_a_time_in_queue_order() {
        let server = FakeFile::start(film_body(), quick()).await;
        let mut f = Fake::serving(server.url());
        // Two connections: still one download at a time.
        f.send(Command::Limit("p".into(), Some(2)));
        f.send(Command::Download(vec![film(1), film(2)]));
        assert_eq!(f.dl_event().await, queued(2, 0));
        for id in [1, 2] {
            let (d, resumed) = dl_started(f.dl_event().await);
            assert_eq!((d.stream_id, resumed), (id, false));
            let d = dl_ended(f.dl_event().await);
            assert_eq!(d.state, DlState::Done);
            let file = PathBuf::from(d.file.unwrap());
            assert_eq!(file, f.dir.path().join(format!("Downloads/Film {id}.mkv")));
            assert_eq!(std::fs::read(&file).unwrap(), film_body());
            assert!(!download::part_of(&file).exists());
        }
        assert_eq!(server.max_in_flight(), 1);

        // Asked again: there already, so nothing is queued.
        f.send(Command::Download(vec![film(1)]));
        assert_eq!(f.dl_event().await, queued(0, 1));
    }

    #[tokio::test]
    async fn a_download_waits_for_and_gives_up_its_connection() {
        let server = FakeFile::start(film_body(), slow()).await;
        let mut f = Fake::serving(server.url());
        // The limit is not known yet, so 1, and mpv has it.
        f.send(Command::Watching(Some("p".into())));
        f.send(Command::Download(vec![film(1)]));
        assert_eq!(f.dl_event().await, queued(1, 0));
        f.quiet(300).await;
        assert_eq!(server.ranges(), [] as [u64; 0]);

        f.send(Command::Watching(None));
        let (d, resumed) = dl_started(f.dl_event().await);
        assert!(!resumed);
        tokio::time::sleep(Duration::from_millis(200)).await;
        // Playback takes it back: paused, queued with its part.
        f.send(Command::Watching(Some("p".into())));
        let Event::DlPaused(paused) = f.dl_event().await else {
            panic!("expected DlPaused");
        };
        assert_eq!(paused.state, DlState::Queued);
        let have = paused.have();
        assert!(have > 0 && have < 40_000, "{have}");
        f.quiet(200).await;

        f.send(Command::Watching(None));
        assert_eq!(
            dl_started(f.dl_event().await),
            (f.download(d.id).unwrap(), true)
        );
        let d = dl_ended(f.dl_event().await);
        assert_eq!(std::fs::read(d.file.unwrap()).unwrap(), film_body());
        let ranges = server.ranges();
        assert_eq!(ranges.len(), 2);
        assert!(ranges[1] >= have, "{ranges:?}");
    }

    #[tokio::test]
    async fn a_recording_takes_the_connection_from_a_download() {
        let server = FakeFile::start(film_body(), slow()).await;
        let mut f = Fake::serving(server.url());
        f.send(Command::Download(vec![film(1)]));
        assert_eq!(f.dl_event().await, queued(1, 0));
        dl_started(f.dl_event().await);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let now = unix_now();
        f.send(Command::Schedule(programme(now - 1, now + 60)));
        let mut got = Vec::new();
        while got.len() < 2 {
            match f.dl_event().await {
                Event::Scheduled(_) => {}
                e => got.push(e),
            }
        }
        let r = got.iter().find_map(|e| match e {
            Event::Started(r) => Some(r.clone()),
            _ => None,
        });
        let r = r.expect("the recording started");
        assert!(
            got.iter().any(|e| matches!(e, Event::DlPaused(_))),
            "{got:?}"
        );

        f.send(Command::Cancel(r.id));
        assert_eq!(ended(f.dl_event().await).state, RecState::Cancelled);
        let (_, resumed) = dl_started(f.dl_event().await);
        assert!(resumed);
    }

    #[tokio::test]
    async fn a_failed_download_says_why_and_is_queued_again_on_asking() {
        let missing = FileOpts {
            status: Some(404),
            ..quick()
        };
        let server = FakeFile::start(film_body(), missing).await;
        let mut f = Fake::serving(server.url());
        f.send(Command::Download(vec![film(1)]));
        assert_eq!(f.dl_event().await, queued(1, 0));
        dl_started(f.dl_event().await);
        let d = dl_ended(f.dl_event().await);
        assert_eq!(d.state, DlState::Failed);
        assert_eq!(d.note.as_deref(), Some("the provider answered HTTP 404"));

        f.send(Command::Download(vec![film(1)]));
        assert_eq!(f.dl_event().await, queued(1, 0));
        let (again, _) = dl_started(f.dl_event().await);
        assert_eq!(again.id, d.id);
        assert_eq!(dl_ended(f.dl_event().await).state, DlState::Failed);
    }

    #[tokio::test]
    async fn quitting_mid_download_keeps_its_part_to_resume() {
        let server = FakeFile::start(film_body(), slow()).await;
        let mut f = Fake::serving(server.url());
        f.send(Command::Download(vec![film(1)]));
        assert_eq!(f.dl_event().await, queued(1, 0));
        let (d, _) = dl_started(f.dl_event().await);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let (done, quit) = oneshot::channel();
        f.send(Command::Quit(done));
        tokio::time::timeout(Duration::from_secs(5), quit)
            .await
            .unwrap()
            .unwrap();
        let left = f.download(d.id).unwrap();
        assert_eq!(left.state, DlState::Queued);
        let have = left.have();
        assert!(have > 0 && have < 40_000, "{have}");
        assert_eq!(left.total, Some(40_000));

        // The next run carries on from the part.
        let Fake { dir, .. } = f;
        let mut f = Fake::again(dir, server.url());
        let (_, resumed) = dl_started(f.dl_event().await);
        assert!(resumed);
        let d = dl_ended(f.dl_event().await);
        assert_eq!(std::fs::read(d.file.unwrap()).unwrap(), film_body());
        assert!(server.ranges()[1] >= have);
    }

    #[tokio::test]
    async fn cancel_forgets_a_download_and_its_part() {
        let server = FakeFile::start(film_body(), slow()).await;
        let mut f = Fake::serving(server.url());
        f.send(Command::Download(vec![film(1), film(2)]));
        assert_eq!(f.dl_event().await, queued(2, 0));
        let (running, _) = dl_started(f.dl_event().await);
        let waiting = 2;
        // The queued one, then the running one, which lets the next start.
        f.send(Command::CancelDownload(waiting));
        let Event::DlCancelled(d) = f.dl_event().await else {
            panic!("expected DlCancelled");
        };
        assert_eq!(d.stream_id, 2);
        f.send(Command::CancelDownload(running.id));
        let Event::DlCancelled(d) = f.dl_event().await else {
            panic!("expected DlCancelled");
        };
        assert!(!d.part().unwrap().exists());
        assert_eq!(f.download(running.id), None);
        f.quiet(300).await;
    }

    #[tokio::test]
    async fn delete_trashes_a_done_file_and_clear_keeps_files() {
        let server = FakeFile::start(film_body(), quick()).await;
        let mut f = Fake::serving(server.url());
        f.send(Command::Download(vec![film(1), film(2)]));
        assert_eq!(f.dl_event().await, queued(2, 0));
        let mut done = Vec::new();
        for _ in 0..2 {
            dl_started(f.dl_event().await);
            done.push(dl_ended(f.dl_event().await));
        }
        f.send(Command::DeleteDownload(done[0].id));
        let Event::DlDeleted(d, how) = f.dl_event().await else {
            panic!("expected DlDeleted");
        };
        assert_eq!(how, Deleted::Trashed);
        assert!(!Path::new(d.file.as_deref().unwrap()).exists());
        let trashed = f.dir.path().join("Trash/files/Film 1.mkv");
        assert!(trashed.exists());
        // Kept, hidden while its file is gone, shown again once restored.
        let row = f.download(d.id).unwrap();
        assert!(row.trashed && !store::download_shown(&row));
        std::fs::rename(&trashed, d.file.as_deref().unwrap()).unwrap();
        assert!(store::download_shown(&f.download(d.id).unwrap()));
        std::fs::rename(d.file.as_deref().unwrap(), &trashed).unwrap();

        // Clear forgets it too, counting only what the tab showed.
        f.send(Command::ClearDownloads);
        assert_eq!(f.dl_event().await, Event::DlCleared(1, 1));
        assert_eq!(f.download(d.id), None);
        assert!(Path::new(done[1].file.as_deref().unwrap()).exists());
        assert_eq!(f.download(done[1].id), None);
    }
}
