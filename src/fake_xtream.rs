//! A local fake Xtream server for tests (SPEC 11): refresh, backoff and
//! error paths. Answers one request per connection with whatever the
//! handler returns for the request target, and logs each request.

use crate::config::Provider;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A canned provider: request target to status and body.
pub type Handler = fn(&str) -> (u16, String);

/// `exp_date` of the fake's active account.
pub const EXPIRES: i64 = 4_102_444_800;

/// A response with any body, sent after `delay`.
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    pub delay: Duration,
}

pub struct FakeXtream {
    url: String,
    log: Arc<Mutex<Vec<(Instant, String)>>>,
    /// Requests being answered now, and the most there have been at once.
    in_flight: Arc<(AtomicUsize, AtomicUsize)>,
}

impl FakeXtream {
    pub async fn start(handler: impl Fn(&str) -> (u16, String) + Send + Sync + 'static) -> Self {
        Self::start_with(move |target| {
            let (status, body) = handler(target);
            Reply {
                status,
                body: body.into_bytes(),
                delay: Duration::ZERO,
            }
        })
        .await
    }

    pub async fn start_with(handler: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Self {
        Self::start_head(move |head| (handler(target(head)), String::new())).await
    }

    /// As [`Self::start_with`], the handler seeing the whole request head
    /// and giving header lines (`Name: value\r\n`) to add to its reply.
    pub async fn start_head(
        handler: impl Fn(&str) -> (Reply, String) + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let log: Arc<Mutex<Vec<(Instant, String)>>> = Arc::default();
        let in_flight: Arc<(AtomicUsize, AtomicUsize)> = Arc::default();
        let handler = Arc::new(handler);
        let (server_log, server_in_flight) = (log.clone(), in_flight.clone());
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let (log, handler) = (server_log.clone(), handler.clone());
                let in_flight = server_in_flight.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buf = [0; 1024];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&head);
                    log.lock()
                        .unwrap()
                        .push((Instant::now(), target(&head).to_string()));
                    let now = in_flight.0.fetch_add(1, Ordering::SeqCst) + 1;
                    in_flight.1.fetch_max(now, Ordering::SeqCst);
                    let (reply, headers) = handler(&head);
                    tokio::time::sleep(reply.delay).await;
                    // Before the reply, so the client cannot have it yet.
                    in_flight.0.fetch_sub(1, Ordering::SeqCst);
                    let head = format!(
                        "HTTP/1.1 {} Fake\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
                        reply.status,
                        reply.body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&reply.body).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Self {
            url,
            log,
            in_flight,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// The most requests answered at once so far.
    pub fn max_in_flight(&self) -> usize {
        self.in_flight.1.load(Ordering::SeqCst)
    }

    /// Request targets in order, e.g. `/logo/1.png`.
    pub fn targets(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .map(|(_, t)| t.clone())
            .collect()
    }

    pub fn provider(&self) -> Provider {
        Provider {
            id: "p".into(),
            name: "Fake".into(),
            url: self.url.clone(),
            username: "user".into(),
            password: "pass".into(),
            epg: Vec::new(),
            source: crate::config::Source::Xtream,
            user_agent: None,
        }
    }

    /// When each request arrived.
    pub fn times(&self) -> Vec<Instant> {
        self.log.lock().unwrap().iter().map(|(t, _)| *t).collect()
    }

    /// What each request asked for, in order: see [`action`].
    pub fn actions(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .map(|(_, target)| action(target).to_string())
            .collect()
    }
}

/// A request head's target, e.g. `/logo/1.png`.
fn target(head: &str) -> &str {
    head.split_whitespace().nth(1).unwrap_or("")
}

/// `account`, `xmltv`, or the `action` parameter of a `player_api.php` request.
pub fn action(target: &str) -> &str {
    if target.starts_with("/xmltv.php") {
        return "xmltv";
    }
    target
        .split(['?', '&'])
        .find_map(|p| p.strip_prefix("action="))
        .unwrap_or("account")
}

/// How far the fake server's clock runs ahead of UTC, as the user's does.
pub const SERVER_OFFSET: i64 = 7200;

fn account(status: &str, exp_date: i64) -> String {
    // The server's clock now, `2026-10-02 14:05:12`, from the XMLTV form.
    let now = xmltv_stamp(crate::import::unix_now(), SERVER_OFFSET / 60);
    let time_now = format!(
        "{}-{}-{} {}:{}:{}",
        &now[0..4],
        &now[4..6],
        &now[6..8],
        &now[8..10],
        &now[10..12],
        &now[12..14]
    );
    format!(
        r#"{{"user_info":{{"username":"user","password":"pass","auth":1,"status":"{status}","exp_date":"{exp_date}","max_connections":"2","active_cons":"1","is_trial":"0","created_at":"1600000000","allowed_output_formats":["m3u8","ts"],"message":"Welcome"}},"server_info":{{"port":"80","https_port":"443","server_protocol":"http","timezone":"Etc/GMT-2","timestamp_now":1600000000,"time_now":"{time_now}"}}}}"#
    )
}

/// A working provider: two channels with an archive (3 and 7 days), one
/// programme airing, and one that ended two days ago on each of `news1`,
/// which has the archive, and `news9`, which no channel has; a movie and a
/// series. Any `/timeshift/`, `/movie/` or `/series/` request is served.
pub fn active(target: &str) -> (u16, String) {
    if target.starts_with("/timeshift/") {
        return (200, "archive".into());
    }
    if target.starts_with("/movie/") {
        return (200, "movie".into());
    }
    if target.starts_with("/series/") {
        return (200, "episode".into());
    }
    let body = match action(target) {
        "account" => account("Active", EXPIRES),
        "get_live_categories" => r#"[{"category_id":"1","category_name":"News","parent_id":0}]"#.into(),
        "get_live_streams" => r#"[
            {"num":1,"name":"News One","stream_id":10,"category_id":"1","epg_channel_id":"news1","tv_archive":1,"tv_archive_duration":3},
            {"num":"2","name":"News Two","stream_id":"11","category_id":"1","epg_channel_id":null,"tv_archive":"1","tv_archive_duration":"7"}
        ]"#
        .into(),
        "xmltv" => {
            let stop = crate::import::unix_now() / 3600 * 3600 - 2 * 86_400;
            let (start, stop) = (xmltv_stamp(stop - 3600, 0), xmltv_stamp(stop, 0));
            format!(
                r#"<?xml version="1.0"?><tv>
            <programme start="20000101000000 +0000" stop="21000101000000 +0000" channel="news1"><title>Rolling news</title></programme>
            <programme start="{start}" stop="{stop}" channel="news1"><title>Morning bulletin</title></programme>
            <programme start="{start}" stop="{stop}" channel="news9"><title>Morning bulletin</title></programme>
        </tv>"#
            )
        }
        "get_simple_data_table" => table(crate::import::unix_now()),
        // Ids collide with the live ones on purpose: each kind has its own.
        "get_vod_categories" => r#"[{"category_id":"1","category_name":"EN - Drama","parent_id":0}]"#.into(),
        "get_vod_streams" => r#"[
            {"num":1,"name":"The Quiet Year","stream_type":"movie","stream_id":20,"stream_icon":"","rating":"7.1","rating_5based":3.6,"added":"1700000000","category_id":"1","category_ids":[1],"container_extension":"mkv","custom_sid":"","direct_source":""}
        ]"#
        .into(),
        "get_vod_info" => {
            let id = target.split("vod_id=").nth(1).and_then(|v| v.parse().ok());
            vod_info(id.unwrap_or(0))
        }
        "get_series_info" => series_info(),
        "get_series_categories" => r#"[{"category_id":"1","category_name":"Crime","parent_id":0}]"#.into(),
        "get_series" => r#"[
            {"num":1,"name":"Harbour Watch","series_id":10,"cover":"","plot":"","cast":"","director":"","genre":"Crime","releaseDate":"2021-03-04","release_date":"2021-03-04","last_modified":"1700000000","rating":"8","rating_5based":"4","backdrop_path":[],"youtube_trailer":"","episode_run_time":"45","category_id":"1","category_ids":[1]}
        ]"#
        .into(),
        _ => return (404, String::new()),
    };
    (200, body)
}

/// A movie's `get_vod_info` in the shape the user's provider sends (SPEC
/// 6.1): numbers as strings, `duration_secs` in minutes, cast and country
/// empty on some.
pub fn vod_info(stream_id: i64) -> String {
    format!(
        r#"{{"info":{{"kinopoisk_url":"","tmdb_id":"4021","name":"The Quiet Year","o_name":"The Quiet Year","cover_big":"","movie_image":"","releasedate":"2019-04-12","episode_run_time":"93","youtube_trailer":"","director":"Iris Vale","actors":"Ada Marsh, Tom Reyes","cast":"Ada Marsh, Tom Reyes","description":"A lighthouse keeper counts the ships that never come.","plot":"A lighthouse keeper counts the ships that never come.","age":"","country":"","genre":"Drama","backdrop_path":[],"duration_secs":93,"duration":"01:33:14","bitrate":4012,"rating":"7.1","runtime":"93","status":"Released"}},"movie_data":{{"stream_id":{stream_id},"name":"The Quiet Year","added":"1700000000","category_id":"1","category_ids":[1],"container_extension":"mkv","custom_sid":"","direct_source":""}}}}"#
    )
}

/// Series 10's `get_series_info` in the shape the user's provider sends
/// (SPEC 6.1): `seasons` lists two, `episodes` holds only the first, and
/// numbers come as strings on some; the titles are the provider's whole
/// ones; an episode's info `id` is not its stream id.
pub fn series_info() -> String {
    r#"{"seasons":[
        {"air_date":"2021-03-04","episode_count":2,"id":501,"name":"Season 1","overview":"","season_number":1,"cover":"","cover_big":""},
        {"air_date":"2022-03-04","episode_count":8,"id":502,"name":"Season 2","overview":"","season_number":2,"cover":"","cover_big":""}],
      "info":{"name":"Harbour Watch","cover":"","plot":"","cast":"","director":"","genre":"Crime","releaseDate":"2021-03-04","last_modified":"1700000000","rating":"8","backdrop_path":[],"episode_run_time":"45","category_id":"1"},
      "episodes":{"1":[
        {"id":"31","episode_num":"2","title":"Harbour Watch - S01E02 - Low Water","container_extension":"mp4","info":{"air_date":"2021-03-11","duration_secs":2712,"duration":"00:45:12","rating":0,"id":9002},"custom_sid":"","added":"1700000000","season":1,"direct_source":""},
        {"id":"30","episode_num":1,"title":"Harbour Watch - S01E01 - The Night Ferry","container_extension":"mkv","info":{"air_date":"2021-03-04","duration_secs":2833,"duration":"00:47:13","rating":0,"id":9001},"custom_sid":"","added":"1700000000","season":1,"direct_source":""}
      ]}}"#
        .into()
}

/// A channel's `get_simple_data_table` as the user's provider sends it
/// (SPEC 6.1): hour-long programmes from 4 days before `now` to 2 h after,
/// titles base64, `has_archive` on those starting within 3 days, times on
/// the server's clock ([`SERVER_OFFSET`]) read as UTC.
pub fn table(now: i64) -> String {
    use base64::Engine;
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    let first = now / 3600 * 3600 - 4 * 86_400;
    let listings: Vec<String> = (0..4 * 24 + 2)
        .map(|k| {
            let start = first + k * 3600;
            format!(
                r#"{{"id":"{k}","epg_id":"1","title":"{}","lang":"en","start":"-","end":"-","description":"{}","channel_id":"news1","start_timestamp":"{}","stop_timestamp":"{}","now_playing":0,"has_archive":{}}}"#,
                b64(&format!("Bulletin {k}")),
                b64("The day's news."),
                start + SERVER_OFFSET,
                start + 3600 + SERVER_OFFSET,
                i64::from(start >= now - 3 * 86_400),
            )
        })
        .collect();
    format!(r#"{{"epg_listings":[{}]}}"#, listings.join(","))
}

/// Formats Unix seconds as XMLTV time in the given UTC offset (minutes).
pub fn xmltv_stamp(t: i64, offset_min: i64) -> String {
    let local = t + offset_min * 60;
    // Inverse of `days_from_civil` (Howard Hinnant's `civil_from_days`).
    let z = local.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let secs = local.rem_euclid(86_400);
    let sign = if offset_min < 0 { '-' } else { '+' };
    let off = offset_min.abs();
    format!(
        "{y:04}{m:02}{d:02}{:02}{:02}{:02} {sign}{:02}{:02}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60,
        off / 60,
        off % 60
    )
}

/// Wrong credentials: `auth` 0, as Xtream answers them.
pub fn rejected(_target: &str) -> (u16, String) {
    (200, r#"{"user_info":{"auth":0}}"#.into())
}

/// Every log line written in this test process from now on, at every
/// level, to check none holds a credential.
pub fn capture_logs() -> Arc<Mutex<Vec<String>>> {
    struct Capture(Arc<Mutex<Vec<String>>>);
    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            self.0.lock().unwrap().push(record.args().to_string());
        }
        fn flush(&self) {}
    }
    static LINES: std::sync::OnceLock<Arc<Mutex<Vec<String>>>> = std::sync::OnceLock::new();
    LINES
        .get_or_init(|| {
            let lines = Arc::default();
            let _ = log::set_boxed_logger(Box::new(Capture(Arc::clone(&lines))));
            log::set_max_level(log::LevelFilter::Trace);
            lines
        })
        .clone()
}

pub fn expired(target: &str) -> (u16, String) {
    match action(target) {
        "account" => (200, account("Expired", 1_600_000_000)),
        _ => active(target),
    }
}

/// Still `Active`, but past its `exp_date`.
pub fn past_expiry(target: &str) -> (u16, String) {
    match action(target) {
        "account" => (200, account("Active", 1_600_000_000)),
        _ => active(target),
    }
}

pub fn disabled(target: &str) -> (u16, String) {
    match action(target) {
        "account" => (200, account("Disabled", EXPIRES)),
        _ => active(target),
    }
}

/// How [`FakeFile`] serves its file.
#[derive(Clone, Default)]
pub struct FileOpts {
    /// Bytes per write, and the pause before each.
    pub chunk: usize,
    pub every: Duration,
    /// Answer a `Range` with the whole file, as some servers do.
    pub ignore_range: bool,
    /// Close the connection once this far into the file.
    pub cut_at: Option<usize>,
    /// Answer with this status and no body instead.
    pub status: Option<u16>,
}

/// A local server for downloads (VOD ticket 10): every request gets the
/// same file, honouring `Range: bytes=N-`, sent in timed chunks.
pub struct FakeFile {
    url: String,
    /// Each request's `Range` start, 0 without one.
    ranges: Arc<Mutex<Vec<u64>>>,
    in_flight: Arc<(AtomicUsize, AtomicUsize)>,
}

impl FakeFile {
    pub async fn start(body: Vec<u8>, opts: FileOpts) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let ranges: Arc<Mutex<Vec<u64>>> = Arc::default();
        let in_flight: Arc<(AtomicUsize, AtomicUsize)> = Arc::default();
        let body = Arc::new(body);
        let (server_ranges, server_in_flight) = (ranges.clone(), in_flight.clone());
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let (ranges, in_flight) = (server_ranges.clone(), server_in_flight.clone());
                let (body, opts) = (body.clone(), opts.clone());
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buf = [0; 1024];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&head).to_lowercase();
                    let from: u64 = head
                        .lines()
                        .find_map(|l| l.strip_prefix("range: bytes="))
                        .and_then(|r| r.trim().trim_end_matches('-').parse().ok())
                        .unwrap_or(0);
                    ranges.lock().unwrap().push(from);
                    if let Some(status) = opts.status {
                        let head = format!(
                            "HTTP/1.1 {status} Fake\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                        let _ = sock.write_all(head.as_bytes()).await;
                        return;
                    }
                    let len = body.len() as u64;
                    let (start, head) = if from > 0 && !opts.ignore_range {
                        (
                            from,
                            format!(
                                "HTTP/1.1 206 Fake\r\nContent-Range: bytes {from}-{}/{len}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                len - 1,
                                len - from
                            ),
                        )
                    } else {
                        (
                            0,
                            format!(
                                "HTTP/1.1 200 Fake\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
                            ),
                        )
                    };
                    let now = in_flight.0.fetch_add(1, Ordering::SeqCst) + 1;
                    in_flight.1.fetch_max(now, Ordering::SeqCst);
                    let mut at = start as usize;
                    let mut ok = sock.write_all(head.as_bytes()).await.is_ok();
                    let end = opts.cut_at.unwrap_or(body.len()).min(body.len());
                    while ok && at < end {
                        tokio::time::sleep(opts.every).await;
                        let to = (at + opts.chunk.max(1)).min(end);
                        ok = sock.write_all(&body[at..to]).await.is_ok();
                        at = to;
                    }
                    let _ = sock.shutdown().await;
                    in_flight.0.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Self {
            url,
            ranges,
            in_flight,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Each request's `Range` start so far, 0 without one.
    pub fn ranges(&self) -> Vec<u64> {
        self.ranges.lock().unwrap().clone()
    }

    /// The most files sent at once so far.
    pub fn max_in_flight(&self) -> usize {
        self.in_flight.1.load(Ordering::SeqCst)
    }
}
