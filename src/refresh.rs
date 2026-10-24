//! Refresh scheduling (SPEC 7.2): decides when to refresh, runs the imports
//! in the background, and reports what it is doing to the UI.

use crate::config::{Provider, Source};
use crate::epg_sources::{self, Meter, Outcome, Planned};
use crate::import::{self, Writer};
use crate::store;
use crate::xtream::{self, Blocked, Client};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// How often the scheduler looks at the data's age while running.
pub const CHECK_EVERY: Duration = Duration::from_secs(15 * 60);

/// How old the movie and series lists may get, in seconds: about 70 MB
/// each, so once a day rather than with the channels (SPEC 7.2).
const VOD_MAX_AGE: i64 = 24 * 3600;

/// Account details for the status bar and the dashboard (SPEC 6.3).
#[derive(Clone, Debug, PartialEq)]
pub struct AccountInfo {
    pub user: xtream::UserInfo,
    pub server: xtream::ServerInfo,
    pub blocked: Option<Blocked>,
    /// When the check ran.
    pub checked: i64,
}

impl AccountInfo {
    pub fn expires(&self) -> Option<i64> {
        self.user.exp_date.filter(|&t| t > 0)
    }

    /// How far the server's clock runs ahead of UTC, from `time_now` at
    /// the check (SPEC 6.1), if it sent one.
    pub fn server_offset(&self) -> Option<i64> {
        let time_now = self.server.time_now.as_deref()?;
        xtream::server_offset(time_now, self.checked)
    }
}

/// What the UI asks a scheduler for.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// `r`: refresh now.
    Refresh,
    /// The dashboard opened: check the account, refreshing only stale data.
    Account,
    /// `refresh_hours` was saved (settings ticket 01): the new age, in
    /// seconds, and the check run at once with it.
    MaxAge(i64),
    /// Guide sources were edited in Settings (EPG sources ticket 04): a
    /// pass at once stores their order, and imports a new one.
    Sources,
}

/// A refresh's stages, in run order (SPEC 7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Channels,
    Epg,
    /// A guide source's turn in the EPG stage, by its label, a row after
    /// the EPG one (EPG sources ticket 04). Run by the EPG stage.
    /// `reimport` when a mapping marked it for a whole fetch (ticket 05).
    Source {
        label: String,
        reimport: bool,
    },
    Movies,
    Series,
}

impl Stage {
    pub fn name(&self) -> &str {
        match self {
            Stage::Channels => "Channels",
            Stage::Epg => "EPG",
            Stage::Source { label, .. } => label,
            Stage::Movies => "Movies",
            Stage::Series => "Series",
        }
    }

    /// The status bar's words while it runs.
    fn busy(&self) -> &'static str {
        match self {
            Stage::Channels => "Updating channels",
            Stage::Epg | Stage::Source { .. } => "Updating guide",
            Stage::Movies => "Updating movies",
            Stage::Series => "Updating series",
        }
    }
}

/// Where a stage is, for the refresh panel (SPEC 8.2).
#[derive(Clone, Debug, PartialEq)]
pub enum StageState {
    Waiting,
    /// Megabytes read, and how far through the body when the provider
    /// sent its length.
    Running {
        mb: u64,
        percent: Option<u8>,
    },
    /// Rows imported.
    Done(usize),
    /// An M3U playlist's channels imported, and its entries skipped for
    /// having no URL (M3U ticket 02).
    DoneSkipping {
        rows: usize,
        skipped: usize,
    },
    /// A guide source the server said has not changed.
    Unchanged,
    /// A guide source not asked: `count` of its `max` requests made in
    /// the last 24 h, the next allowed at `next`.
    AtCap {
        count: usize,
        max: u32,
        next: i64,
    },
    /// A 429 or 5xx: tried again by itself at `retry_at` (SPEC 6.2). A
    /// guide source's, by its `Retry-After`.
    Backoff {
        retry_at: i64,
    },
    Failed(String),
}

pub type Stages = Vec<(Stage, StageState)>;

impl From<Outcome> for StageState {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Unchanged => StageState::Unchanged,
            Outcome::Fetched(n) => StageState::Done(n),
            Outcome::AtCap { count, max, next } => StageState::AtCap { count, max, next },
            Outcome::Backoff { retry_at } => StageState::Backoff { retry_at },
            Outcome::Failed(e) => StageState::Failed(e),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Update {
    /// A refresh is running: what it is doing, e.g. `Updating guide… 34 MB`.
    Busy(String),
    /// The stages of the refresh running or last run, each time one moves.
    Stages(Stages),
    Account(Box<AccountInfo>),
    /// Channels were replaced; the UI re-reads them.
    Channels(usize),
    /// Programmes were replaced.
    Epg(usize),
    /// Movies were replaced.
    Movies(usize),
    /// Series were replaced.
    Series(usize),
    /// Nothing running. `updated` is when the data was last refreshed (the
    /// older of channels and EPG); `retry_at` is set while backing off.
    Idle {
        updated: Option<i64>,
        error: Option<String>,
        retry_at: Option<i64>,
    },
}

pub struct Refresher {
    pub client: Arc<Client>,
    pub writer: Writer,
    pub provider: Provider,
    /// `refresh_hours`, in seconds.
    pub max_age: i64,
    pub check_every: Duration,
    pub updates: mpsc::UnboundedSender<Update>,
}

/// A guide source's row, which the EPG stage runs.
fn is_source(stage: &Stage) -> bool {
    matches!(stage, Stage::Source { .. })
}

/// A stage that has finished, so a retry skips it.
fn done(state: &StageState) -> bool {
    matches!(state, StageState::Done(_) | StageState::DoneSkipping { .. })
}

/// Data never fetched, or fetched `max_age` or more seconds ago.
fn stale(last: Option<i64>, now: i64, max_age: i64) -> bool {
    last.is_none_or(|t| now - t >= max_age)
}

impl Refresher {
    /// Runs until `manual` closes. Each request on it forces a refresh or
    /// an account check, still subject to the client's rate limits.
    ///
    /// The first pass always checks the account, so expiry and max
    /// connections show even when the cached data is fresh. Later passes
    /// check it only as part of a refresh, or when asked (SPEC 6.2).
    ///
    /// A refresh that backs off keeps its stages: the retry runs those not
    /// done, forced or not.
    pub async fn run(mut self, mut manual: mpsc::UnboundedReceiver<Request>) {
        let (mut force, mut check_account) = (false, true);
        let mut stages = Stages::new();
        loop {
            let result = self.pass(force, check_account, &mut stages).await;
            check_account = false;
            // Presses during a full refresh asked for the one that just ran.
            // A dashboard opened meanwhile still gets its own check, as the
            // pass's was made before the downloads.
            let mut dashboard = false;
            // Sources edited meanwhile may not be the ones that pass used.
            let mut sources = false;
            if result == Ok(true) {
                while let Ok(r) = manual.try_recv() {
                    match r {
                        Request::Account => dashboard = true,
                        Request::MaxAge(age) => self.max_age = age,
                        Request::Sources => sources = true,
                        Request::Refresh => {}
                    }
                }
            }
            // After a 429 or 5xx, retry once the backoff ends.
            let retry = self
                .client
                .backoff_until()
                .filter(|_| result.is_err())
                .map(|t| t.saturating_duration_since(Instant::now()))
                .filter(|&d| d < self.check_every);
            let retry_at = retry.map(|d| import::unix_now() + d.as_secs() as i64);
            if let Err(e) = &result {
                // The stage running, or the first not run if the account
                // check failed.
                if let Some((_, state)) = stages
                    .iter_mut()
                    .find(|(stage, s)| !is_source(stage) && !done(s))
                {
                    *state = match retry_at {
                        Some(retry_at) => StageState::Backoff { retry_at },
                        None => StageState::Failed(e.clone()),
                    };
                    self.send(Update::Stages(stages.clone()));
                }
            }
            if retry.is_none() {
                stages.clear();
            }
            self.idle(result.err(), retry_at);
            if dashboard || sources {
                (force, check_account) = (false, dashboard);
                continue;
            }
            tokio::select! {
                () = tokio::time::sleep(retry.unwrap_or(self.check_every)) => force = false,
                m = manual.recv() => match m {
                    Some(Request::Refresh) => {
                        force = true;
                        stages.clear();
                    }
                    Some(Request::Account) => (force, check_account) = (false, true),
                    Some(Request::MaxAge(age)) => {
                        self.max_age = age;
                        force = false;
                    }
                    Some(Request::Sources) => force = false,
                    None => return,
                },
            }
        }
    }

    /// One look at the data: refreshes channels, then EPG, then movies and
    /// series, whichever is stale, or all if `force`; or, with `stages`
    /// kept from a refresh that backed off, those not done. True if
    /// channels and EPG both were.
    async fn pass(
        &self,
        force: bool,
        check_account: bool,
        stages: &mut Stages,
    ) -> Result<bool, String> {
        self.try_pass(force, check_account, stages)
            .await
            .map_err(|e| {
                let e = xtream::redact(&e);
                log::error!("refresh: {e}");
                e
            })
    }

    async fn try_pass(
        &self,
        force: bool,
        check_account: bool,
        stages: &mut Stages,
    ) -> Result<bool, String> {
        let now = import::unix_now();
        let mut sources = self.sync_sources(now)?;
        let m3u = self.provider.source == Source::M3u;
        if stages.is_empty() {
            let (channels_at, epg_at) = self.last_refreshes()?;
            let (movies_at, series_at) = {
                let conn = self.writer.lock().unwrap_or_else(|e| e.into_inner());
                store::last_vod_refreshes(&conn, &self.provider.id)
                    .map_err(|e| format!("reading provider: {e}"))?
            };
            let ages = [
                (Stage::Channels, channels_at, self.max_age),
                (Stage::Epg, epg_at, self.max_age),
                (Stage::Movies, movies_at, VOD_MAX_AGE),
                (Stage::Series, series_at, VOD_MAX_AGE),
            ];
            // A source due makes the guide due, so one added to the file, or
            // with a shorter `every_hours`, does not wait for the provider's.
            let source_due = sources.iter().any(|s| s.due);
            // A playlist path is read again when it changes, not by age.
            let changed = if m3u {
                import::playlist_changed(&self.writer, &self.client.provider())
            } else {
                None
            };
            stages.extend(
                ages.into_iter()
                    // An M3U provider has no movies or series.
                    .filter(|(stage, ..)| !m3u || matches!(stage, Stage::Channels | Stage::Epg))
                    .filter(|(stage, at, max_age)| {
                        force
                            || match changed {
                                Some(changed) if *stage == Stage::Channels => changed,
                                _ => stale(*at, now, *max_age),
                            }
                            || *stage == Stage::Epg && source_due
                    })
                    .map(|(stage, ..)| (stage, StageState::Waiting)),
            );
        } else {
            for (stage, state) in stages.iter_mut() {
                if !is_source(stage) && !done(state) {
                    *state = StageState::Waiting;
                }
            }
        }
        // An M3U provider has no account to check.
        let check_account = check_account && !m3u;
        if stages.is_empty() && !check_account {
            return Ok(false);
        }
        if !stages.is_empty() {
            self.send(Update::Stages(stages.clone()));
        }
        let ext = if m3u { "" } else { self.account(now).await? };

        let mut ran = Vec::new();
        // The EPG stage adds its sources' rows after its own.
        let mut next = 0;
        while next < stages.len() {
            let i = next;
            next += 1;
            if is_source(&stages[i].0) || done(&stages[i].1) {
                continue;
            }
            let stage = stages[i].0.clone();
            // The client's, so a name edited in Settings is the one stored.
            let provider = self.client.provider();
            let (client, writer, provider) = (&self.client, &self.writer, &provider);
            let mut state = None;
            let n = match stage {
                Stage::Channels if provider.source == Source::M3u => {
                    let import = import::playlist(client, writer, provider, force);
                    let (n, s) = match self.progress(stages, i, import).await? {
                        Some((rows, 0)) => (rows, StageState::Done(rows)),
                        Some((rows, skipped)) => (rows, StageState::DoneSkipping { rows, skipped }),
                        None => (0, StageState::Unchanged),
                    };
                    // The header may name other guide feeds now (M3U ticket 05).
                    sources = self.sync_sources(import::unix_now())?;
                    state = Some(s);
                    n
                }
                Stage::Channels => {
                    let import = import::channels(client, writer, provider, ext);
                    self.progress(stages, i, import).await?
                }
                Stage::Epg => self.epg(stages, i, provider, &sources, force).await?,
                Stage::Movies => {
                    self.progress(stages, i, import::movies(client, writer, provider))
                        .await?
                }
                Stage::Series => {
                    self.progress(stages, i, import::series(client, writer, provider))
                        .await?
                }
                Stage::Source { .. } => continue,
            };
            stages[i].1 = state.unwrap_or(StageState::Done(n));
            self.send(Update::Stages(stages.clone()));
            self.send(match stage {
                Stage::Channels => Update::Channels(n),
                Stage::Epg | Stage::Source { .. } => Update::Epg(n),
                Stage::Movies => Update::Movies(n),
                Stage::Series => Update::Series(n),
            });
            ran.push(stage);
        }
        Ok(ran.contains(&Stage::Channels) && ran.contains(&Stage::Epg))
    }

    /// Stores the provider's guide sources in their order and returns the
    /// enabled external ones ([`epg_sources::sync`]). The client's
    /// provider, so sources edited in the file since are the ones used.
    fn sync_sources(&self, now: i64) -> Result<Vec<Planned>, String> {
        let mut conn = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        epg_sources::sync(&mut conn, &self.client.provider(), self.max_age, now)
    }

    /// The account check (SPEC 6.3), once per refresh: sends what it found
    /// and returns the stream extension, or why refresh is paused.
    async fn account(&self, now: i64) -> Result<&'static str, String> {
        self.send(Update::Busy("Checking account…".into()));
        let xtream::Account {
            user_info: user,
            server_info: server,
        } = self.client.account().await?;
        let ext = user.stream_extension();
        log::info!(
            "account: status {:?}, expires {:?}, max connections {:?}, formats {:?} -> .{ext}",
            user.status,
            user.exp_date,
            user.max_connections,
            user.allowed_output_formats
        );
        let blocked = user.blocked(now);
        self.send(Update::Account(Box::new(AccountInfo {
            user,
            server,
            blocked: blocked.clone(),
            checked: now,
        })));
        match blocked {
            Some(Blocked::Expired) => {
                return Err(
                    "Account expired. Saved channels still work; refresh is paused.".into(),
                );
            }
            Some(Blocked::Disabled(status)) => {
                return Err(format!(
                    "Account {}. Saved channels still work; refresh is paused.",
                    status.to_lowercase()
                ));
            }
            None => {}
        }

        Ok(ext)
    }

    /// The EPG stage, stage `i`: the provider's guide unless it is off, or
    /// fresh and the stage ran only for a due source, then each enabled
    /// source in order that is due, or all with `force`, within its limits
    /// (SPEC 7.2). A source that fails is logged and stored with it, and
    /// the stage goes on. Each source taking a turn has its own row after
    /// the stage's, with its own progress. Returns the programmes imported.
    async fn epg(
        &self,
        stages: &mut Stages,
        i: usize,
        provider: &Provider,
        sources: &[Planned],
        force: bool,
    ) -> Result<usize, String> {
        let (on, shift) = {
            let conn = self.writer.lock().unwrap_or_else(|e| e.into_inner());
            store::provider_guide(&conn, &provider.id)
                .map_err(|e| format!("reading guide sources: {e}"))?
        };
        let due = force || stale(self.last_refreshes()?.1, import::unix_now(), self.max_age);
        // An M3U provider has no `xmltv.php`: its playlist's feeds are
        // sources in its place (M3U ticket 05).
        let on = on && provider.source == Source::Xtream;
        let mut n = if !due {
            0
        } else if on {
            let import = import::epg(&self.client, &self.writer, provider, shift * 60);
            self.progress(stages, i, import).await?
        } else {
            let conn = self.writer.lock().unwrap_or_else(|e| e.into_inner());
            store::stamp_epg(&conn, &provider.id, import::unix_now())
                .map_err(|e| format!("saving provider: {e}"))?;
            0
        };
        // Rows from an earlier try of this stage go.
        let end = (i + 1..stages.len())
            .find(|&j| !is_source(&stages[j].0))
            .unwrap_or(stages.len());
        let turns: Vec<&Planned> = sources.iter().filter(|s| force || s.turn).collect();
        // Named as Settings › EPG names them.
        let feeds = {
            let conn = self.writer.lock().unwrap_or_else(|e| e.into_inner());
            epg_sources::feeds(&conn, provider)
        };
        let order = crate::config::feed_order(provider, &feeds);
        let labels = crate::config::feed_labels(provider, &feeds);
        let label = |s: &Planned| {
            order
                .iter()
                .zip(&labels)
                .find(|(o, _)| !o.provider_guide && o.ident() == s.source.ident())
                .map_or_else(|| s.source.label(), |(_, l)| l.clone())
        };
        stages.splice(
            i + 1..end,
            turns.iter().map(|s| {
                let (label, reimport) = (label(s), s.reimport);
                (Stage::Source { label, reimport }, StageState::Waiting)
            }),
        );
        if !turns.is_empty() {
            self.send(Update::Stages(stages.clone()));
        }
        // Rows of sources that turned out not due, dropped at the end.
        let mut idle = Vec::new();
        for (k, source) in turns.into_iter().enumerate() {
            let j = i + 1 + k;
            let meter = Meter::default();
            let import = epg_sources::import(&self.writer, &provider.id, source, force, &meter);
            match self
                .progress_on(stages, j, import, || meter.progress())
                .await
            {
                Some(outcome) => {
                    if let Outcome::Fetched(k) = outcome {
                        n += k;
                    }
                    stages[j].1 = outcome.into();
                    self.send(Update::Stages(stages.clone()));
                }
                None => idle.push(j),
            }
        }
        for j in idle.into_iter().rev() {
            stages.remove(j);
        }
        Ok(n)
    }

    /// Awaits `work`, stage `i` of `stages`, reporting how far it has got
    /// on the provider's client every half second.
    async fn progress<T>(&self, stages: &mut Stages, i: usize, work: impl Future<Output = T>) -> T {
        let client = &self.client;
        self.progress_on(stages, i, work, || {
            (client.received(), client.stream_progress())
        })
        .await
    }

    /// [`Refresher::progress`] by `meter`: bytes received so far, and the
    /// body's read and length while it is known.
    async fn progress_on<T>(
        &self,
        stages: &mut Stages,
        i: usize,
        work: impl Future<Output = T>,
        meter: impl Fn() -> (u64, Option<(u64, u64)>),
    ) -> T {
        let label = stages[i].0.busy();
        let start = meter().0;
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        tokio::pin!(work);
        loop {
            tokio::select! {
                out = &mut work => return out,
                _ = tick.tick() => {
                    let (received, stream) = meter();
                    let mb = (received - start) / 1_000_000;
                    // 100 % waits for the import to finish.
                    let percent = stream.map(|(read, len)| (read * 100 / len).min(99) as u8);
                    let state = StageState::Running { mb, percent };
                    if stages[i].1 != state {
                        stages[i].1 = state;
                        self.send(Update::Stages(stages.clone()));
                    }
                    self.send(Update::Busy(if mb == 0 {
                        format!("{label}…")
                    } else {
                        format!("{label}… {mb} MB")
                    }));
                }
            }
        }
    }

    fn idle(&self, error: Option<String>, retry_at: Option<i64>) {
        let updated = self
            .last_refreshes()
            .inspect_err(|e| log::error!("{e}"))
            .ok()
            .and_then(|(c, e)| c.into_iter().chain(e).min());
        self.send(Update::Idle {
            updated,
            error,
            retry_at,
        });
    }

    fn last_refreshes(&self) -> Result<(Option<i64>, Option<i64>), String> {
        // Uncontended: only this task writes, and imports are awaited first.
        let conn = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        store::last_refreshes(&conn, &self.provider.id)
            .map_err(|e| format!("reading provider: {e}"))
    }

    fn send(&self, update: Update) {
        let _ = self.updates.send(update);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_xtream::{self, FakeXtream};
    use crate::xtream::Limits;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const HOUR: i64 = 3600;

    const FAST: Limits = Limits {
        min_gap: Duration::from_millis(10),
        backoff_start: Duration::from_millis(200),
        backoff_max: Duration::from_millis(800),
    };

    struct Setup {
        _dir: tempfile::TempDir,
        writer: Writer,
    }

    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let conn = store::open(&dir.path().join("v.db")).unwrap();
        Setup {
            _dir: dir,
            writer: Arc::new(Mutex::new(conn)),
        }
    }

    fn refresher(
        fake: &FakeXtream,
        setup: &Setup,
        hours: i64,
    ) -> (Refresher, mpsc::UnboundedReceiver<Update>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let r = Refresher {
            client: Arc::new(Client::with_limits(fake.provider(), FAST)),
            writer: setup.writer.clone(),
            provider: fake.provider(),
            max_age: hours * HOUR,
            check_every: Duration::from_secs(3600),
            updates: tx,
        };
        (r, rx)
    }

    impl Refresher {
        /// One pass, as the scheduler's first.
        async fn once(&self, force: bool, check_account: bool) -> Result<bool, String> {
            self.pass(force, check_account, &mut Stages::new()).await
        }
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<Update>) -> Vec<Update> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn channel_count(setup: &Setup) -> i64 {
        let conn = setup.writer.lock().unwrap();
        conn.query_row("SELECT count(*) FROM channel", [], |r| r.get(0))
            .unwrap()
    }

    /// Makes the stored data `hours` old.
    fn age(setup: &Setup, hours: i64) {
        let t = import::unix_now() - hours * HOUR;
        setup
            .writer
            .lock()
            .unwrap()
            .execute(
                "UPDATE provider SET last_channels_refresh = ?1, last_epg_refresh = ?1",
                [t],
            )
            .unwrap();
    }

    /// A refresh of channels and EPG.
    const LIVE: [&str; 4] = [
        "account",
        "get_live_categories",
        "get_live_streams",
        "xmltv",
    ];

    /// A refresh of everything.
    const FULL: [&str; 8] = [
        "account",
        "get_live_categories",
        "get_live_streams",
        "xmltv",
        "get_vod_categories",
        "get_vod_streams",
        "get_series_categories",
        "get_series",
    ];

    #[test]
    fn staleness() {
        assert!(stale(None, 100, 50));
        assert!(stale(Some(50), 100, 50));
        assert!(!stale(Some(51), 100, 50));
        // A clock that went backwards is not a reason to refresh.
        assert!(!stale(Some(200), 100, 50));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stale_data_triggers_refresh() {
        let fake = FakeXtream::start(fake_xtream::active).await;
        let setup = setup();
        let (r, mut rx) = refresher(&fake, &setup, 12);
        r.once(false, false).await.unwrap();
        assert_eq!(fake.actions(), FULL);
        assert_eq!(channel_count(&setup), 2);
        let updates = drain(&mut rx);
        assert!(updates.contains(&Update::Channels(2)), "{updates:?}");
        // The past programme on the channel with an archive, and the one
        // airing; not the past one on a guide id with no channel.
        assert!(updates.contains(&Update::Epg(2)), "{updates:?}");
        let guides: Vec<(String, i64)> = setup
            .writer
            .lock()
            .unwrap()
            .prepare("SELECT epg_channel_id, archive_days FROM channel JOIN programme USING (provider_id, epg_channel_id)")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(guides, [("news1".into(), 3), ("news1".into(), 3)]);
        let account = updates.iter().find_map(|u| match u {
            Update::Account(a) => Some(a),
            _ => None,
        });
        let account = account.expect("an account update");
        assert_eq!(account.expires(), Some(fake_xtream::EXPIRES));
        assert_eq!(account.user.max_connections, Some(2));
        assert_eq!(account.user.active_cons, Some(1));
        assert_eq!(account.server.timezone.as_deref(), Some("Etc/GMT-2"));
        assert_eq!(account.blocked, None);
        assert!(updates.contains(&Update::Movies(1)), "{updates:?}");
        assert!(updates.contains(&Update::Series(1)), "{updates:?}");

        // Stale again: refreshes again; movies and series wait a day.
        age(&setup, 13);
        r.once(false, false).await.unwrap();
        assert_eq!(fake.actions()[8..], LIVE);
    }

    /// Movies and series refresh once a day, each on its own age.
    #[tokio::test(flavor = "multi_thread")]
    async fn movies_and_series_refresh_daily() {
        let fake = FakeXtream::start(fake_xtream::active).await;
        let setup = setup();
        let (r, _rx) = refresher(&fake, &setup, 12);
        r.once(false, false).await.unwrap();
        let set = |column: &str, hours: i64| {
            let t = import::unix_now() - hours * HOUR;
            let sql = format!("UPDATE provider SET {column} = ?1");
            setup.writer.lock().unwrap().execute(&sql, [t]).unwrap();
        };
        set("last_movies_refresh", 23);
        r.once(false, false).await.unwrap();
        assert_eq!(fake.actions().len(), 8);
        set("last_movies_refresh", 25);
        r.once(false, false).await.unwrap();
        assert_eq!(
            fake.actions()[8..],
            ["account", "get_vod_categories", "get_vod_streams"]
        );
        set("last_series_refresh", 25);
        r.once(false, false).await.unwrap();
        assert_eq!(
            fake.actions()[11..],
            ["account", "get_series_categories", "get_series"]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fresh_data_does_not_refresh() {
        let fake = FakeXtream::start(fake_xtream::active).await;
        let setup = setup();
        let (r, _rx) = refresher(&fake, &setup, 12);
        r.once(false, false).await.unwrap();
        age(&setup, 11);
        r.once(false, false).await.unwrap();
        assert_eq!(fake.actions(), FULL);
        // The launch pass checks the account and nothing else.
        r.once(false, true).await.unwrap();
        assert_eq!(fake.actions()[8..], ["account"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refresh_hours_six_is_honoured() {
        let fake = FakeXtream::start(fake_xtream::active).await;
        let setup = setup();
        let (twelve, _rx) = refresher(&fake, &setup, 12);
        twelve.once(false, false).await.unwrap();
        age(&setup, 7);
        twelve.once(false, false).await.unwrap();
        assert_eq!(fake.actions().len(), 8);
        let (six, _rx) = refresher(&fake, &setup, 6);
        six.once(false, false).await.unwrap();
        assert_eq!(fake.actions()[8..], LIVE);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn only_the_stale_part_refreshes() {
        let fake = FakeXtream::start(fake_xtream::active).await;
        let setup = setup();
        let (r, _rx) = refresher(&fake, &setup, 12);
        r.once(false, false).await.unwrap();
        let t = import::unix_now() - 13 * HOUR;
        setup
            .writer
            .lock()
            .unwrap()
            .execute("UPDATE provider SET last_epg_refresh = ?1", [t])
            .unwrap();
        r.once(false, false).await.unwrap();
        assert_eq!(fake.actions()[8..], ["account", "xmltv"]);
    }

    /// A source added to the file imports on the next pass, fresh as the
    /// provider's guide is, without asking the provider for it again.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_new_source_imports_without_waiting() {
        let fake = FakeXtream::start(fake_xtream::active).await;
        let setup = setup();
        let (r, _rx) = refresher(&fake, &setup, 12);
        r.once(false, false).await.unwrap();
        let feed = setup._dir.path().join("guide.xml");
        std::fs::write(&feed, "<tv></tv>").unwrap();
        let mut provider = fake.provider();
        provider.epg = vec![crate::config::EpgSource {
            url: None,
            path: Some(feed),
            provider_guide: false,
            name: None,
            shift_minutes: 0,
            enabled: true,
            every_hours: None,
            max_per_day: None,
        }];
        let r = Refresher {
            client: Arc::new(Client::with_limits(provider, FAST)),
            ..r
        };
        r.once(false, false).await.unwrap();
        assert_eq!(fake.actions()[8..], ["account"]);
        let imported: Option<i64> = setup
            .writer
            .lock()
            .unwrap()
            .query_row(
                "SELECT imported FROM epg_source WHERE ident != ''",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(imported.is_some());
        // Imported once, it waits as the provider's guide does.
        r.once(false, false).await.unwrap();
        assert_eq!(fake.actions().len(), 9);
    }

    /// Each source taking a turn has its own row under EPG, and an `r`
    /// gives every enabled one a turn (EPG sources ticket 04).
    #[tokio::test(flavor = "multi_thread")]
    async fn each_source_has_its_own_row() {
        let fake = FakeXtream::start(fake_xtream::active).await;
        let setup = setup();
        let feed = setup._dir.path().join("guide.xml");
        std::fs::write(&feed, "<tv></tv>").unwrap();
        let mut provider = fake.provider();
        provider.epg = vec![crate::config::EpgSource {
            path: Some(feed),
            provider_guide: false,
            ..crate::config::EpgSource::provider_guide()
        }];
        let (r, mut rx) = refresher(&fake, &setup, 12);
        let r = Refresher {
            client: Arc::new(Client::with_limits(provider, FAST)),
            ..r
        };
        let last_rows = |rx: &mut mpsc::UnboundedReceiver<Update>| {
            drain(rx)
                .into_iter()
                .filter_map(|u| match u {
                    Update::Stages(s) => Some(s),
                    _ => None,
                })
                .next_back()
                .unwrap()
        };
        r.once(false, false).await.unwrap();
        let rows = last_rows(&mut rx);
        assert_eq!(rows[1], (Stage::Epg, StageState::Done(2)));
        assert_eq!(
            rows[2],
            (
                Stage::Source {
                    label: "guide.xml".into(),
                    reimport: false
                },
                StageState::Done(0)
            )
        );
        assert_eq!(rows[3].0, Stage::Movies);
        // Its file unchanged, the next pass gives it no turn; `r` does.
        r.once(false, false).await.unwrap();
        let updates = drain(&mut rx);
        assert!(!updates.iter().any(|u| matches!(u, Update::Stages(_))));
        r.once(true, false).await.unwrap();
        let rows = last_rows(&mut rx);
        assert_eq!(
            rows[2],
            (
                Stage::Source {
                    label: "guide.xml".into(),
                    reimport: false
                },
                StageState::Done(0)
            )
        );
        assert_eq!(rows.len(), 5);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn forced_refresh_ignores_age() {
        let fake = FakeXtream::start(fake_xtream::active).await;
        let setup = setup();
        let (r, _rx) = refresher(&fake, &setup, 12);
        r.once(false, false).await.unwrap();
        r.once(true, false).await.unwrap();
        assert_eq!(fake.actions()[8..], FULL);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn expired_account_blocks_refresh() {
        let setup = setup();
        let fake = FakeXtream::start(fake_xtream::active).await;
        refresher(&fake, &setup, 12)
            .0
            .once(false, false)
            .await
            .unwrap();
        age(&setup, 13);

        let cases: [(fake_xtream::Handler, Blocked, &str); 3] = [
            (fake_xtream::expired, Blocked::Expired, "Account expired"),
            (
                fake_xtream::past_expiry,
                Blocked::Expired,
                "Account expired",
            ),
            (
                fake_xtream::disabled,
                Blocked::Disabled("Disabled".into()),
                "Account disabled",
            ),
        ];
        for (handler, blocked, why) in cases {
            let fake = FakeXtream::start(handler).await;
            let (r, mut rx) = refresher(&fake, &setup, 12);
            let err = r.once(true, false).await.unwrap_err();
            assert!(err.starts_with(why), "{err}");
            assert_eq!(fake.actions(), ["account"]);
            assert_eq!(channel_count(&setup), 2, "cached data kept");
            let account = drain(&mut rx).into_iter().find_map(|u| match u {
                Update::Account(a) => Some(a),
                _ => None,
            });
            assert_eq!(account.unwrap().blocked, Some(blocked));
        }
    }

    /// A saved `refresh_hours` re-arms the check: data 7 h old refreshes
    /// at once on 6, not at the next 15-minute check.
    #[tokio::test(flavor = "multi_thread")]
    async fn new_max_age_rearms_the_check() {
        let fake = FakeXtream::start(fake_xtream::active).await;
        let setup = setup();
        refresher(&fake, &setup, 12)
            .0
            .once(false, false)
            .await
            .unwrap();
        age(&setup, 7);
        let (r, rx) = refresher(&fake, &setup, 12);
        let (_, errors) = run_until_idle(r, rx, &[Request::MaxAge(6 * HOUR)], 2).await;
        assert_eq!(errors, [None, None]);
        // The launch pass checks the account; the new age refreshes.
        let mut want = vec!["account"];
        want.extend(LIVE);
        assert_eq!(fake.actions()[8..], want);
    }

    /// A 503 on the first request: the scheduler reports it, then retries
    /// by itself once the backoff ends rather than at the next check.
    #[tokio::test(flavor = "multi_thread")]
    async fn scheduler_retries_after_backoff() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let fake = FakeXtream::start(|target: &str| {
            if CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
                (503, String::new())
            } else {
                fake_xtream::active(target)
            }
        })
        .await;
        let setup = setup();
        let (r, mut rx) = refresher(&fake, &setup, 12);
        let (manual_tx, manual_rx) = mpsc::unbounded_channel();
        let started = Instant::now();
        let task = tokio::spawn(r.run(manual_rx));

        let mut idles = Vec::new();
        while idles.len() < 2 {
            let update = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("scheduler stalled")
                .unwrap();
            if let Update::Idle {
                error, retry_at, ..
            } = update
            {
                idles.push((error, retry_at));
            }
        }
        let (error, retry_at) = &idles[0];
        assert!(error.as_deref().unwrap().contains("503"), "{error:?}");
        assert!(retry_at.is_some());
        assert_eq!(idles[1], (None, None));
        assert!(started.elapsed() >= FAST.backoff_start);
        assert_eq!(fake.actions()[1..], FULL);

        // `r`: a forced refresh while fresh.
        manual_tx.send(Request::Refresh).unwrap();
        loop {
            if let Update::Idle { error, .. } = rx.recv().await.unwrap() {
                assert_eq!(error, None);
                break;
            }
        }
        assert_eq!(fake.actions()[9..], FULL);

        // The dashboard: the account only, as the data is fresh.
        manual_tx.send(Request::Account).unwrap();
        loop {
            match rx.recv().await.unwrap() {
                Update::Account(_) => {}
                Update::Idle { error, .. } => {
                    assert_eq!(error, None);
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(fake.actions()[17..], ["account"]);
        drop(manual_tx);
        task.await.unwrap();
    }

    /// Runs the scheduler until it has gone idle `n` times, then stops it.
    /// The stage tables it sent, and the errors it went idle with.
    async fn run_until_idle(
        r: Refresher,
        mut rx: mpsc::UnboundedReceiver<Update>,
        manual: &[Request],
        n: usize,
    ) -> (Vec<Stages>, Vec<Option<String>>) {
        let (manual_tx, manual_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(r.run(manual_rx));
        let (mut tables, mut errors) = (Vec::new(), Vec::new());
        while errors.len() < n {
            let update = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("scheduler stalled")
                .unwrap();
            match update {
                Update::Stages(s) => tables.push(s),
                Update::Idle { error, .. } => {
                    errors.push(error);
                    // After the launch pass: what `r` would send.
                    if errors.len() == 1 {
                        for m in manual {
                            manual_tx.send(m.clone()).unwrap();
                        }
                    }
                }
                _ => {}
            }
        }
        drop(manual_tx);
        task.await.unwrap();
        (tables, errors)
    }

    fn states(table: &Stages) -> Vec<StageState> {
        table.iter().map(|(_, s)| s.clone()).collect()
    }

    /// `r` on fresh data, with a 503 on the movie list: the stages before
    /// it are done, it backs off, and the retry runs it and the series,
    /// although neither is stale.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_backoff_keeps_the_stages_left() {
        static LISTS: AtomicUsize = AtomicUsize::new(0);
        let fake = FakeXtream::start(|target: &str| {
            if fake_xtream::action(target) == "get_vod_streams"
                && LISTS.fetch_add(1, Ordering::SeqCst) == 1
            {
                (503, String::new())
            } else {
                fake_xtream::active(target)
            }
        })
        .await;
        let setup = setup();
        let (r, rx) = refresher(&fake, &setup, 12);
        let (tables, errors) = run_until_idle(r, rx, &[Request::Refresh], 3).await;
        assert_eq!(errors[0], None);
        assert!(errors[1].as_deref().unwrap().contains("503"), "{errors:?}");
        assert_eq!(errors[2], None);

        let order: Vec<Stage> = tables[0].iter().map(|(s, _)| s.clone()).collect();
        assert_eq!(
            order,
            [Stage::Channels, Stage::Epg, Stage::Movies, Stage::Series]
        );
        assert_eq!(states(&tables[0]), [const { StageState::Waiting }; 4]);
        let backoff = tables
            .iter()
            .position(|t| matches!(t[2].1, StageState::Backoff { .. }))
            .expect("a backoff");
        let t = &tables[backoff];
        assert_eq!(
            t[..2],
            [
                (Stage::Channels, StageState::Done(2)),
                (Stage::Epg, StageState::Done(2))
            ]
        );
        assert_eq!(t[3].1, StageState::Waiting);
        // The retry starts with what was done, and finishes the rest.
        assert_eq!(
            states(&tables[backoff + 1]),
            [
                StageState::Done(2),
                StageState::Done(2),
                StageState::Waiting,
                StageState::Waiting
            ]
        );
        assert_eq!(
            states(tables.last().unwrap()),
            [
                StageState::Done(2),
                StageState::Done(2),
                StageState::Done(1),
                StageState::Done(1)
            ]
        );
        assert_eq!(
            fake.actions()[8..],
            [
                "account",
                "get_live_categories",
                "get_live_streams",
                "xmltv",
                "get_vod_categories",
                "get_vod_streams",
                "account",
                "get_vod_categories",
                "get_vod_streams",
                "get_series_categories",
                "get_series",
            ]
        );
    }

    /// A 404 on the guide is not retried: the guide fails with the reason,
    /// and the stages after it never start.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failure_says_why_on_its_stage() {
        let fake = FakeXtream::start(|target: &str| {
            if fake_xtream::action(target) == "xmltv" {
                (404, String::new())
            } else {
                fake_xtream::active(target)
            }
        })
        .await;
        let setup = setup();
        let (r, rx) = refresher(&fake, &setup, 12);
        let (tables, errors) = run_until_idle(r, rx, &[], 1).await;
        let error = errors[0].as_deref().unwrap();
        assert!(error.contains("404"), "{error}");
        let last = states(tables.last().unwrap());
        assert_eq!(
            last,
            [
                StageState::Done(2),
                StageState::Failed(error.into()),
                StageState::Waiting,
                StageState::Waiting
            ]
        );
    }

    /// A dashboard opened during a full refresh gets a check of its own
    /// once the refresh ends; `r` presses meanwhile are dropped.
    #[tokio::test(flavor = "multi_thread")]
    async fn dashboard_during_refresh_gets_its_own_check() {
        let fake = FakeXtream::start(fake_xtream::active).await;
        let setup = setup();
        let (r, mut rx) = refresher(&fake, &setup, 12);
        let (manual_tx, manual_rx) = mpsc::unbounded_channel();
        // Queued before the launch pass, so they arrive during it.
        manual_tx.send(Request::Refresh).unwrap();
        manual_tx.send(Request::Account).unwrap();
        let task = tokio::spawn(r.run(manual_rx));
        let mut idles = 0;
        while idles < 2 {
            let update = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("scheduler stalled")
                .unwrap();
            idles += usize::from(matches!(update, Update::Idle { .. }));
        }
        assert_eq!(fake.actions()[..8], FULL);
        assert_eq!(fake.actions()[8..], ["account"]);
        drop(manual_tx);
        task.await.unwrap();
    }

    /// The last stages sent, and every update.
    fn stages_sent(rx: &mut mpsc::UnboundedReceiver<Update>) -> (Stages, Vec<Update>) {
        let updates = drain(rx);
        let stages = updates
            .iter()
            .filter_map(|u| match u {
                Update::Stages(s) => Some(s.clone()),
                _ => None,
            })
            .next_back()
            .unwrap_or_default();
        (stages, updates)
    }

    fn m3u_refresher(setup: &Setup, url: String) -> (Refresher, mpsc::UnboundedReceiver<Update>) {
        let provider = Provider {
            id: "p".into(),
            name: "Lists".into(),
            url,
            username: String::new(),
            password: String::new(),
            epg: Vec::new(),
            source: Source::M3u,
            user_agent: Some("Example/1.0".into()),
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let r = Refresher {
            client: Arc::new(Client::with_limits(provider.clone(), FAST)),
            writer: setup.writer.clone(),
            provider,
            max_age: 12 * HOUR,
            check_every: Duration::from_secs(3600),
            updates: tx,
        };
        (r, rx)
    }

    const M3U: &str = "#EXTM3U\n#EXTINF:-1 tvg-id=\"harbour.uk\",Harbour One\nhttp://s/h\n#EXTINF:-1,Lost\n#EXTINF:-1 group-title=\"News\",Juniper\nhttp://s/j\n";

    /// An M3U provider by URL: one GET with its user agent, no account,
    /// no movies or series, and its URL nowhere in the log (M3U ticket 02).
    #[tokio::test(flavor = "multi_thread")]
    async fn an_m3u_url_is_one_get_with_its_user_agent() {
        let logs = fake_xtream::capture_logs();
        let heads: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen = heads.clone();
        let fake = FakeXtream::start_head(move |head| {
            seen.lock().unwrap().push(head.to_ascii_lowercase());
            let reply = fake_xtream::Reply {
                status: 200,
                body: M3U.into(),
                delay: Duration::ZERO,
            };
            (reply, String::new())
        })
        .await;
        let setup = setup();
        let url = format!("{}/lists/tv.m3u?token=tok-9x", fake.url());
        let (r, mut rx) = m3u_refresher(&setup, url);
        assert_eq!(r.once(false, true).await, Ok(true));
        assert_eq!(fake.targets(), ["/lists/tv.m3u?token=tok-9x"]);
        assert!(heads.lock().unwrap()[0].contains("user-agent: example/1.0"));
        assert_eq!(channel_count(&setup), 2);
        let (stages, updates) = stages_sent(&mut rx);
        assert_eq!(
            stages,
            [
                (
                    Stage::Channels,
                    StageState::DoneSkipping {
                        rows: 2,
                        skipped: 1
                    }
                ),
                (Stage::Epg, StageState::Done(0)),
            ]
        );
        assert!(!updates.iter().any(|u| matches!(u, Update::Account(_))));
        let logged = logs.lock().unwrap().join("\n");
        assert!(
            !logged.contains("tok-9x") && !logged.contains("/lists/"),
            "{logged}"
        );

        // Fresh, and the dashboard asking: nothing to do.
        assert_eq!(r.once(false, true).await, Ok(false));
        assert_eq!(fake.targets().len(), 1);
        // Stale by age, as an Xtream list is.
        age(&setup, 13);
        r.once(false, false).await.unwrap();
        assert_eq!(fake.targets().len(), 2);
    }

    /// The playlist's guide feeds are imported in the same refresh that
    /// first reads them, each with its own row (M3U ticket 05).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_playlists_feeds_import_with_it() {
        let playlist = Arc::new(Mutex::new(String::new()));
        let serve = playlist.clone();
        let now = import::unix_now();
        let feed = format!(
            r#"<tv><channel id="h"><display-name>Harbour One</display-name></channel><programme start="{}" stop="{}" channel="h"><title>News</title></programme></tv>"#,
            fake_xtream::xmltv_stamp(now - HOUR, 0),
            fake_xtream::xmltv_stamp(now + HOUR, 0)
        );
        let xz = import::xz(feed.as_bytes());
        let fake = FakeXtream::start_with(move |target| fake_xtream::Reply {
            status: 200,
            body: if target.starts_with("/g.xml.xz") {
                xz.clone()
            } else {
                serve.lock().unwrap().clone().into_bytes()
            },
            delay: Duration::ZERO,
        })
        .await;
        *playlist.lock().unwrap() = M3U.replace(
            "#EXTM3U",
            &format!("#EXTM3U x-tvg-url=\"{}/g.xml.xz?token=tok-9x\"", fake.url()),
        );
        let setup = setup();
        let (r, mut rx) = m3u_refresher(&setup, format!("{}/tv.m3u", fake.url()));
        assert_eq!(r.once(false, false).await, Ok(true));
        let (stages, _) = stages_sent(&mut rx);
        assert_eq!(
            stages[1..],
            [
                (Stage::Epg, StageState::Done(1)),
                (
                    Stage::Source {
                        label: "g.xml.xz".into(),
                        reimport: false
                    },
                    StageState::Done(1)
                ),
            ]
        );
        let conn = setup.writer.lock().unwrap();
        let guided: i64 = conn
            .query_row(
                "SELECT count(*) FROM channel WHERE guide_id IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(guided, 1);
    }

    /// A rejected playlist keeps the channels; `run` shows the reason on
    /// the stage, as for any failed stage.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_rejected_playlist_keeps_the_channels() {
        let body = Arc::new(Mutex::new(M3U.to_string()));
        let serve = body.clone();
        let fake = FakeXtream::start_with(move |_| fake_xtream::Reply {
            status: 200,
            body: serve.lock().unwrap().clone().into_bytes(),
            delay: Duration::ZERO,
        })
        .await;
        let setup = setup();
        let (r, mut rx) = m3u_refresher(&setup, format!("{}/tv.m3u", fake.url()));
        r.once(false, false).await.unwrap();
        *body.lock().unwrap() = "#EXTM3U\n#EXT-X-TARGETDURATION:6\nseg.ts\n".into();
        drain(&mut rx);
        let err = r.once(true, false).await.unwrap_err();
        assert!(err.contains("a stream, not a channel list"), "{err}");
        assert_eq!(channel_count(&setup), 2);
    }

    /// A playlist path is read again when its modification time changes,
    /// or on `r`, not by age.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_m3u_path_is_read_when_it_changes() {
        let setup = setup();
        let file = setup._dir.path().join("tv.m3u");
        std::fs::write(&file, M3U).unwrap();
        let (r, mut rx) = m3u_refresher(&setup, file.display().to_string());
        r.once(false, false).await.unwrap();
        assert_eq!(channel_count(&setup), 2);
        // Old, but unchanged: the guide is due, not the channels.
        age(&setup, 13);
        drain(&mut rx);
        r.once(false, false).await.unwrap();
        let (stages, _) = stages_sent(&mut rx);
        assert_eq!(stages, [(Stage::Epg, StageState::Done(0))]);
        // Changed: read again.
        std::fs::write(&file, format!("{M3U}#EXTINF:-1,Kestrel\nhttp://s/k\n")).unwrap();
        let later = std::time::SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(later)
            .unwrap();
        r.once(false, false).await.unwrap();
        assert_eq!(channel_count(&setup), 3);
        let (stages, _) = stages_sent(&mut rx);
        assert_eq!(stages[0].0, Stage::Channels);
        // `r` reads it unchanged.
        r.once(true, false).await.unwrap();
        let (stages, _) = stages_sent(&mut rx);
        assert_eq!(
            stages[0],
            (
                Stage::Channels,
                StageState::DoneSkipping {
                    rows: 3,
                    skipped: 1
                }
            )
        );
    }
}
