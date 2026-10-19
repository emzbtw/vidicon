//! Xtream Codes client (SPEC 6): rate-limited HTTP and credential redaction.

use crate::config::Provider;
use bytes::Bytes;
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use serde::de::Deserializer;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify, mpsc};

/// Each provider's client by id, shared with the background tasks, so a
/// provider added while running is used at once (onboarding ticket 01).
#[derive(Clone, Default)]
pub struct Clients(Arc<std::sync::RwLock<std::collections::HashMap<String, Arc<Client>>>>);

impl Clients {
    pub fn get(&self, id: &str) -> Option<Arc<Client>> {
        self.0
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    pub fn insert(&self, id: String, client: Arc<Client>) {
        self.0
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, client);
    }

    pub fn remove(&self, id: &str) {
        self.0.write().unwrap_or_else(|e| e.into_inner()).remove(id);
    }

    /// The provider whose server is `host`, to tell its own logos from
    /// third-party ones.
    pub fn by_host(&self, host: &str) -> Option<Arc<Client>> {
        let clients = self.0.read().unwrap_or_else(|e| e.into_inner());
        clients
            .values()
            .find(|c| c.host().as_deref() == Some(host))
            .cloned()
    }
}

impl FromIterator<(String, Arc<Client>)> for Clients {
    fn from_iter<I: IntoIterator<Item = (String, Arc<Client>)>>(iter: I) -> Self {
        Self(Arc::new(std::sync::RwLock::new(iter.into_iter().collect())))
    }
}

/// Request spacing and backoff (SPEC 6.2).
#[derive(Clone, Copy)]
pub struct Limits {
    pub min_gap: Duration,
    pub backoff_start: Duration,
    pub backoff_max: Duration,
}

pub const SPEC_LIMITS: Limits = Limits {
    min_gap: Duration::from_secs(2),
    backoff_start: Duration::from_secs(30),
    backoff_max: Duration::from_secs(30 * 60),
};

/// Strips credentials from a URL before it is logged or shown: the values of
/// `username` and `password` query parameters, and the two path segments after
/// `/live/`, `/timeshift/`, `/movie/` or `/series/`. Works on any string, so it
/// is safe on error text too.
pub fn redact(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    // Query parameters.
    while let Some(i) = find_param(rest) {
        let (head, tail) = rest.split_at(i);
        let eq = tail.find('=').unwrap() + 1;
        out.push_str(head);
        out.push_str(&tail[..eq]);
        out.push_str("***");
        let end = tail[eq..]
            .find(['&', '#', ' ', '"', '\'', ')'])
            .map_or(tail.len(), |e| eq + e);
        rest = &tail[end..];
    }
    out.push_str(rest);
    // `/live/<user>/<pass>/` paths, and the same under the other prefixes.
    for prefix in ["/live/", "/timeshift/", "/movie/", "/series/"] {
        out = redact_path(&out, prefix);
    }
    out
}

/// Masks the two path segments after each `prefix`.
fn redact_path(s: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(prefix) {
        let start = i + prefix.len();
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let mut parts = tail.splitn(3, '/');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(u), Some(p), Some(_)) if !u.is_empty() && !p.is_empty() => {
                out.push_str("***/***/");
                rest = &tail[u.len() + 1 + p.len() + 1..];
            }
            _ => rest = tail,
        }
    }
    out.push_str(rest);
    out
}

/// Byte offset of the next `username=` or `password=` that starts a parameter.
fn find_param(s: &str) -> Option<usize> {
    let lower = s.to_ascii_lowercase();
    let mut best: Option<usize> = None;
    for key in ["username=", "password="] {
        let mut from = 0;
        while let Some(i) = lower[from..].find(key).map(|i| from + i) {
            if i > 0 && matches!(s.as_bytes()[i - 1], b'?' | b'&' | b';') {
                best = Some(best.map_or(i, |b| b.min(i)));
                break;
            }
            from = i + 1;
        }
    }
    best
}

/// Request spacing and backoff for one host.
pub struct Gate {
    pub next_at: Instant,
    backoff: Duration,
}

impl Gate {
    pub fn new() -> Self {
        Self {
            next_at: Instant::now(),
            backoff: Duration::ZERO,
        }
    }

    /// Records a response's status: a 429 or 5xx backs off, doubling from
    /// `backoff_start` up to `backoff_max` (SPEC 6.2); anything else ends
    /// the backoff. True if backing off.
    pub fn record(&mut self, status: StatusCode, limits: &Limits) -> bool {
        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            self.backoff = if self.backoff.is_zero() {
                limits.backoff_start
            } else {
                (self.backoff * 2).min(limits.backoff_max)
            };
            self.next_at = Instant::now() + self.backoff;
            true
        } else {
            self.backoff = Duration::ZERO;
            false
        }
    }
}

/// The HTTP client for provider and logo requests, with the honest
/// `User-Agent` (SPEC 6.2).
pub fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("vidicon/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()
        .expect("HTTP client")
}

/// GETs a small body, giving up past `max` bytes. For logos (ticket 13).
pub async fn get_small(
    http: &reqwest::Client,
    url: Url,
    max: usize,
) -> Result<(StatusCode, Vec<u8>), String> {
    let result = async {
        let mut resp = http
            .get(url)
            .timeout(Duration::from_secs(20))
            .send()
            .await?;
        let status = resp.status();
        let mut body = Vec::new();
        if status.is_success() {
            while let Some(chunk) = resp.chunk().await? {
                body.extend_from_slice(&chunk);
                if body.len() > max {
                    return Ok(Err(format!("larger than {max} bytes")));
                }
            }
        }
        Ok::<_, reqwest::Error>(Ok((status, body)))
    }
    .await;
    result.map_err(|e| e.without_url().to_string())?
}

/// The most of a `get_simple_data_table` body taken: about 100 KB measured
/// (SPEC 6.1).
const TABLE_MAX: usize = 4 * 1024 * 1024;

/// The most of a `get_vod_info` body taken: 1 to 3 KB measured (SPEC 6.1).
const INFO_MAX: usize = 1024 * 1024;

/// The most of a `get_series_info` body taken: 15 to 25 KB measured for
/// series of 6 to 10 episodes (SPEC 6.1), so room for a few thousand.
const SERIES_INFO_MAX: usize = 4 * 1024 * 1024;

/// One client per provider. The gate mutex is held for the whole request, so at
/// most one request is in flight, and it spaces requests per SPEC 6.2.
pub struct Client {
    http: reqwest::Client,
    /// Replaced when the provider is edited (onboarding ticket 02).
    provider: std::sync::RwLock<Provider>,
    limits: Limits,
    gate: Mutex<Gate>,
    /// API requests waiting for the gate; logos and guide tables wait until
    /// there are none.
    api_waiting: AtomicU64,
    api_idle: Notify,
    /// Body bytes received so far, for progress.
    received: AtomicU64,
    /// The list or guide being streamed: bytes read, and its length if the
    /// provider sent one, else 0 (refresh panel ticket 01).
    stream_read: AtomicU64,
    stream_len: AtomicU64,
}

#[derive(Debug, Deserialize)]
pub struct Account {
    pub user_info: UserInfo,
    #[serde(default, deserialize_with = "lenient_object")]
    pub server_info: ServerInfo,
}

/// Everything but `auth` is optional, and panels mistype the rest. The
/// response also echoes the username and password: never parsed, so they
/// cannot reach the DB, logs or UI from here (ticket 21).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct UserInfo {
    #[serde(default, deserialize_with = "lenient_int")]
    pub auth: Option<i64>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub status: Option<String>,
    #[serde(default, deserialize_with = "lenient_int")]
    pub exp_date: Option<i64>,
    #[serde(default, deserialize_with = "lenient_int")]
    pub max_connections: Option<i64>,
    #[serde(default, deserialize_with = "lenient_int")]
    pub active_cons: Option<i64>,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub is_trial: Option<bool>,
    #[serde(default, deserialize_with = "lenient_int")]
    pub created_at: Option<i64>,
    #[serde(default, deserialize_with = "lenient_strings")]
    pub allowed_output_formats: Vec<String>,
    /// The provider's message to its users.
    #[serde(default, deserialize_with = "lenient_string")]
    pub message: Option<String>,
}

/// `server_info`, all optional. Its `url` is the provider's host, which the
/// dashboard keeps masked with the credentials' URL, so it is not parsed.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct ServerInfo {
    #[serde(default, deserialize_with = "lenient_string")]
    pub port: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub https_port: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub rtmp_port: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub server_protocol: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub timezone: Option<String>,
    /// The server's local time as it writes it, e.g. `2026-09-30 14:05:12`.
    #[serde(default, deserialize_with = "lenient_string")]
    pub time_now: Option<String>,
}

/// What the add-provider form's account check found (onboarding ticket 01,
/// SPEC 6.3). Messages never hold the credentials.
#[derive(Debug)]
pub enum Check {
    /// Logged in. Expired or disabled ([`UserInfo::blocked`]) only warns.
    Account(Box<Account>),
    /// `auth` 0, or HTTP 513: the username or password is wrong.
    Rejected,
    /// No answer, or the server's own trouble (429, 5xx but 513): why.
    Unreachable(String),
    /// HTTP 401 or 403: the server is there but refuses this request,
    /// e.g. a stale address or a blocked IP.
    Forbidden(StatusCode),
    /// An answer that is not an Xtream account: why.
    NotXtream(String),
}

/// Why an account cannot refresh (SPEC 6.3).
#[derive(Clone, Debug, PartialEq)]
pub enum Blocked {
    Expired,
    /// The provider's status text, e.g. `Disabled` or `Banned`.
    Disabled(String),
}

impl UserInfo {
    /// Xtream's `status` is `Active`, `Expired`, `Disabled` or `Banned`. Some
    /// panels leave it `Active` past `exp_date`, so the date counts too.
    pub fn blocked(&self, now: i64) -> Option<Blocked> {
        match self.status.as_deref() {
            Some(s) if s.eq_ignore_ascii_case("expired") => Some(Blocked::Expired),
            None | Some("") => None,
            Some(s) if !s.eq_ignore_ascii_case("active") => Some(Blocked::Disabled(s.into())),
            _ => None,
        }
        .or_else(|| {
            self.exp_date
                .filter(|&t| t > 0 && t <= now)
                .map(|_| Blocked::Expired)
        })
    }

    /// SPEC 6.1: prefer `ts`, then `m3u8`.
    pub fn stream_extension(&self) -> &'static str {
        let has = |f: &str| self.allowed_output_formats.iter().any(|x| x == f);
        if has("ts") || !has("m3u8") {
            "ts"
        } else {
            "m3u8"
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Category {
    #[serde(deserialize_with = "lenient_string")]
    pub category_id: Option<String>,
    pub category_name: String,
}

#[derive(Debug, Deserialize)]
pub struct Stream {
    #[serde(deserialize_with = "lenient_int")]
    pub stream_id: Option<i64>,
    #[serde(default, deserialize_with = "lenient_int")]
    pub num: Option<i64>,
    #[serde(default)]
    pub name: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub category_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub epg_channel_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub stream_icon: Option<String>,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub tv_archive: Option<bool>,
    #[serde(default, deserialize_with = "lenient_int")]
    pub tv_archive_duration: Option<i64>,
}

impl Stream {
    /// How many days back the provider keeps the channel's archive, 0 for
    /// none: `tv_archive_duration` when `tv_archive` is 1 (SPEC 5).
    pub fn archive_days(&self) -> i64 {
        match self.tv_archive {
            Some(true) => self.tv_archive_duration.unwrap_or(0).max(0),
            _ => 0,
        }
    }
}

/// A movie in `get_vod_streams` (SPEC 6.1). The list has no year, length,
/// genre or plot; those come with its info.
#[derive(Debug, Deserialize)]
pub struct Movie {
    #[serde(default, deserialize_with = "lenient_int")]
    pub stream_id: Option<i64>,
    #[serde(default, deserialize_with = "lenient_int")]
    pub num: Option<i64>,
    #[serde(default)]
    pub name: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub category_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_float")]
    pub rating: Option<f64>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub stream_icon: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub container_extension: Option<String>,
    #[serde(default, deserialize_with = "lenient_int")]
    pub added: Option<i64>,
}

/// A series in `get_series` (SPEC 6.1).
#[derive(Debug, Deserialize)]
pub struct Series {
    #[serde(default, deserialize_with = "lenient_int")]
    pub series_id: Option<i64>,
    #[serde(default, deserialize_with = "lenient_int")]
    pub num: Option<i64>,
    #[serde(default)]
    pub name: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub category_id: Option<String>,
    /// `YYYY-MM-DD`. The list sends it twice, as `releaseDate` too; an
    /// alias would make that a duplicate field.
    #[serde(default, deserialize_with = "lenient_string")]
    pub release_date: Option<String>,
    #[serde(default, rename = "releaseDate", deserialize_with = "lenient_string")]
    pub release_date_alt: Option<String>,
    #[serde(default, deserialize_with = "lenient_float")]
    pub rating: Option<f64>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub cover: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub genre: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub plot: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub cast: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub director: Option<String>,
    /// Minutes.
    #[serde(default, deserialize_with = "lenient_int")]
    pub episode_run_time: Option<i64>,
    #[serde(default, deserialize_with = "lenient_int")]
    pub last_modified: Option<i64>,
}

impl Series {
    /// The year of `release_date`.
    pub fn year(&self) -> Option<i64> {
        let date = self
            .release_date
            .as_ref()
            .or(self.release_date_alt.as_ref())?;
        date.trim().get(..4)?.parse().ok()
    }
}

/// `2026-10-01 20:00:00` as if UTC: a wall clock reading in seconds.
pub fn wall(s: &str) -> Option<i64> {
    let n: Vec<i64> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let [y, mo, d, h, mi, se] = n[..] else {
        return None;
    };
    Some(crate::import::days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se)
}

/// A wall clock reading as the archive URL's `YYYY-MM-DD:HH-MM`.
pub fn url_time(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}:{:02}-{:02}",
        secs / 3600,
        secs % 3600 / 60
    )
}

/// How far the server's clock runs ahead of UTC (SPEC 6.1): the account
/// check's `time_now`, read as if UTC, against the clock when the check
/// ran, rounded to 15 min.
pub fn server_offset(time_now: &str, checked: i64) -> Option<i64> {
    let ahead = wall(time_now)? - checked;
    Some((ahead as f64 / 900.0).round() as i64 * 900)
}

/// A past programme's archive request (SPEC 6.1): from its start, to the
/// minute below, in the server's clock `offset` ahead of UTC, for as many
/// minutes as cover it plus `after` seconds for a guide running late.
/// Returns the start as the URL writes it, the minutes, and the start in
/// Unix seconds, where mpv's `time-pos` 0 falls.
pub fn archive_request(start: i64, stop: i64, offset: i64, after: i64) -> (String, i64, i64) {
    let from = start - start.rem_euclid(60);
    let minutes = (stop + after - from + 59) / 60;
    (url_time(from + offset), minutes, from)
}

/// Body chunks as they arrive. An `Err` means the download failed part way.
pub type Chunk = Result<Bytes, String>;

/// Providers send numbers as strings and strings as numbers; accept both.
pub(crate) fn lenient_int<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

pub(crate) fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// A number, or a string holding one; empty or anything else is none.
pub(crate) fn lenient_float<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|f: &f64| f.is_finite()))
}

/// `1`, `"1"`, `true` and `"true"` are true; `0`, `"0"`, `false`, `"false"` false.
pub(crate) fn lenient_bool<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Bool(b) => Some(b),
        serde_json::Value::Number(n) => n.as_i64().map(|n| n != 0),
        serde_json::Value::String(s) => match s.trim() {
            "1" | "true" => Some(true),
            "0" | "false" => Some(false),
            _ => None,
        },
        _ => None,
    })
}

/// A list of strings or numbers; anything else is empty.
fn lenient_strings<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|v| match v {
                serde_json::Value::String(s) if !s.is_empty() => Some(s),
                serde_json::Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    })
}

/// An object whose fields are all lenient; anything else (panels send `[]`)
/// is the default.
fn lenient_object<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    Ok(match serde_json::Value::deserialize(d)? {
        v @ serde_json::Value::Object(_) => serde_json::from_value(v).unwrap_or_default(),
        _ => T::default(),
    })
}

impl Client {
    pub fn new(provider: Provider) -> Self {
        Self::with_limits(provider, SPEC_LIMITS)
    }

    /// Tests shrink the limits to keep backoff timings short.
    pub fn with_limits(provider: Provider, limits: Limits) -> Self {
        Self {
            http: http(),
            provider: std::sync::RwLock::new(provider),
            limits,
            gate: Mutex::new(Gate::new()),
            api_waiting: AtomicU64::new(0),
            api_idle: Notify::new(),
            received: AtomicU64::new(0),
            stream_read: AtomicU64::new(0),
            stream_len: AtomicU64::new(0),
        }
    }

    /// The provider as last set.
    pub fn provider(&self) -> Provider {
        self.provider
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The provider edited in Settings (onboarding ticket 02): its name,
    /// URL and credentials are used from the next request, under the same
    /// limits and backoff.
    pub fn set_provider(&self, provider: Provider) {
        *self.provider.write().unwrap_or_else(|e| e.into_inner()) = provider;
    }

    /// The provider's host, e.g. to tell its own logos from third-party ones.
    pub fn host(&self) -> Option<String> {
        self.base().ok()?.host_str().map(str::to_owned)
    }

    /// Body bytes received over the client's lifetime.
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    /// The list or guide being streamed, while its length is known: bytes
    /// read and the length. A gzipped body's is not, as reqwest unpacks it.
    pub fn stream_progress(&self) -> Option<(u64, u64)> {
        let len = self.stream_len.load(Ordering::Relaxed);
        (len > 0).then(|| (self.stream_read.load(Ordering::Relaxed), len))
    }

    /// When the next request may go out, if the provider asked us to back
    /// off (429 or 5xx). `None` while a request is in flight.
    pub fn backoff_until(&self) -> Option<Instant> {
        let gate = self.gate.try_lock().ok()?;
        (!gate.backoff.is_zero()).then_some(gate.next_at)
    }

    pub async fn account(&self) -> Result<Account, String> {
        let account: Account = self.api(None).await?;
        if account.user_info.auth != Some(1) {
            return Err(format!(
                "{}: login rejected by provider",
                self.provider().name
            ));
        }
        Ok(account)
    }

    /// The add-provider form's account check (onboarding ticket 01): the
    /// account request, through the limiter, its outcome told apart.
    pub async fn check(&self) -> Check {
        let url = match self.api_url(None) {
            Ok(url) => url,
            Err(e) => return Check::NotXtream(e),
        };
        let (tx, mut rx) = mpsc::channel::<Chunk>(8);
        let collect = async {
            let mut body = Vec::new();
            while let Some(Ok(chunk)) = rx.recv().await {
                body.extend_from_slice(&chunk);
            }
            body
        };
        let (fetched, body) = tokio::join!(self.get_status(url, tx, false), collect);
        match fetched {
            // Not the server's trouble: some providers refuse a wrong
            // password this way rather than with `auth` 0.
            Err((Some(s), _)) if s.as_u16() == 513 => return Check::Rejected,
            Err((Some(s), _)) if matches!(s.as_u16(), 401 | 403) => return Check::Forbidden(s),
            Err((Some(s), e)) if s != StatusCode::TOO_MANY_REQUESTS && !s.is_server_error() => {
                return Check::NotXtream(e);
            }
            Err((_, e)) => return Check::Unreachable(e),
            Ok(_) => {}
        }
        match serde_json::from_slice::<Account>(&body) {
            Ok(a) if a.user_info.auth == Some(1) => Check::Account(Box::new(a)),
            Ok(_) => Check::Rejected,
            // Position only, as in `json`: the body echoes the password.
            Err(e) => Check::NotXtream(format!(
                "{}: not an Xtream account response (line {} column {})",
                self.provider().name,
                e.line(),
                e.column()
            )),
        }
    }

    pub async fn live_categories(&self) -> Result<Vec<Category>, String> {
        self.api(Some("get_live_categories")).await
    }

    /// Streams the raw `get_live_streams` body into `tx`; the list is too big
    /// to buffer (ADR 0003). Returns the byte count.
    pub async fn live_streams(&self, tx: mpsc::Sender<Chunk>) -> Result<usize, String> {
        self.get(self.api_url(Some("get_live_streams"))?, tx, true)
            .await
    }

    /// A movie or series category list: `get_vod_categories` or
    /// `get_series_categories`.
    pub async fn categories(&self, action: &str) -> Result<Vec<Category>, String> {
        self.api(Some(action)).await
    }

    /// Streams the raw body of a list too big to buffer, `get_vod_streams`
    /// or `get_series` (about 70 MB each, SPEC 6.1), into `tx`. Returns the
    /// byte count.
    pub async fn list(&self, action: &str, tx: mpsc::Sender<Chunk>) -> Result<usize, String> {
        self.get(self.api_url(Some(action))?, tx, true).await
    }

    /// Streams the raw `xmltv.php` body into `tx`: the full guide, often tens
    /// of MB. Returns the byte count.
    pub async fn xmltv(&self, tx: mpsc::Sender<Chunk>) -> Result<usize, String> {
        self.get(self.url("xmltv.php", None)?, tx, true).await
    }

    /// A logo on the provider's own host (ticket 13): one of its requests
    /// under SPEC 6.2, behind any API request waiting. Not counted in
    /// [`Client::received`].
    pub async fn logo(&self, url: Url, max: usize) -> Result<(StatusCode, Vec<u8>), String> {
        self.behind_api(url, max, "logo").await
    }

    /// `get_simple_data_table` for one channel, about 100 KB (SPEC 6.1):
    /// its guide's older days (catch-up ticket 06). Waits behind any API
    /// request, as a logo does, so a refresh goes first.
    pub async fn guide_table(&self, stream_id: i64) -> Result<Vec<u8>, String> {
        let mut url = self.api_url(Some("get_simple_data_table"))?;
        url.query_pairs_mut()
            .append_pair("stream_id", &stream_id.to_string());
        let name = self.provider().name;
        let (status, body) = self
            .behind_api(url, TABLE_MAX, "get_simple_data_table")
            .await
            .map_err(|e| format!("{name}: {e}"))?;
        if !status.is_success() {
            return Err(format!("{name}: provider returned HTTP {status}"));
        }
        Ok(body)
    }

    /// `get_vod_info` for one movie, 1 to 3 KB (SPEC 6.1): its year,
    /// length, plot and cast (VOD ticket 04). Waits behind any API request,
    /// as a guide table does.
    pub async fn vod_info(&self, vod_id: i64) -> Result<Vec<u8>, String> {
        let mut url = self.api_url(Some("get_vod_info"))?;
        url.query_pairs_mut()
            .append_pair("vod_id", &vod_id.to_string());
        let name = self.provider().name;
        let (status, body) = self
            .behind_api(url, INFO_MAX, "get_vod_info")
            .await
            .map_err(|e| format!("{name}: {e}"))?;
        if !status.is_success() {
            return Err(format!("{name}: provider returned HTTP {status}"));
        }
        Ok(body)
    }

    /// `get_series_info` for one series, 15 to 25 KB (SPEC 6.1): its
    /// seasons and episodes (VOD ticket 05). Waits behind any API request,
    /// as a movie's info does.
    pub async fn series_info(&self, series_id: i64) -> Result<Vec<u8>, String> {
        let mut url = self.api_url(Some("get_series_info"))?;
        url.query_pairs_mut()
            .append_pair("series_id", &series_id.to_string());
        let name = self.provider().name;
        let (status, body) = self
            .behind_api(url, SERIES_INFO_MAX, "get_series_info")
            .await
            .map_err(|e| format!("{name}: {e}"))?;
        if !status.is_success() {
            return Err(format!("{name}: provider returned HTTP {status}"));
        }
        Ok(body)
    }

    /// One small request under SPEC 6.2 that waits until no API request
    /// does: the user waits on a refresh, not on a logo or a table.
    async fn behind_api(
        &self,
        url: Url,
        max: usize,
        what: &str,
    ) -> Result<(StatusCode, Vec<u8>), String> {
        loop {
            let idle = self.api_idle.notified();
            if self.api_waiting.load(Ordering::SeqCst) > 0 {
                idle.await;
                continue;
            }
            let mut gate = self.gate.lock().await;
            // Wait without the gate, so an API request can take it, and
            // `backoff_until` still answers.
            if gate.next_at > Instant::now() || self.api_waiting.load(Ordering::SeqCst) > 0 {
                let at = gate.next_at;
                drop(gate);
                tokio::time::sleep_until(at.into()).await;
                continue;
            }
            let result = get_small(&self.http, url.clone(), max).await;
            gate.next_at = Instant::now() + self.limits.min_gap;
            if let Ok((status, _)) = &result
                && gate.record(*status, &self.limits)
            {
                log::warn!(
                    "{}: {what} HTTP {status}, backing off {}s",
                    self.provider().name,
                    gate.backoff.as_secs()
                );
            }
            return result;
        }
    }

    /// `{url}/live/{username}/{password}/{stream_id}.{ext}`. Never log it unredacted.
    pub fn stream_url(&self, stream_id: i64, ext: &str) -> Result<Url, String> {
        let mut url = self.base()?;
        url.path_segments_mut()
            .map_err(|_| "provider url cannot be a base".to_string())?
            .pop_if_empty()
            .extend([
                "live",
                &self.provider().username,
                &self.provider().password,
                &format!("{stream_id}.{ext}"),
            ]);
        Ok(url)
    }

    /// `{url}/{kind}/{username}/{password}/{id}.{ext}`, `kind` `movie` or
    /// `series` (an episode's id). Never log it unredacted.
    pub fn vod_url(&self, kind: &str, id: i64, ext: &str) -> Result<Url, String> {
        let mut url = self.base()?;
        url.path_segments_mut()
            .map_err(|_| "provider url cannot be a base".to_string())?
            .pop_if_empty()
            .extend([
                kind,
                &self.provider().username,
                &self.provider().password,
                &format!("{id}.{ext}"),
            ]);
        Ok(url)
    }

    /// `{url}/timeshift/{username}/{password}/{minutes}/{start}/{stream_id}.{ext}`,
    /// `start` as `YYYY-MM-DD:HH-MM`: the archive (catch-up ticket 01). Never
    /// log it unredacted.
    pub fn timeshift_url(
        &self,
        stream_id: i64,
        start: &str,
        minutes: i64,
        ext: &str,
    ) -> Result<Url, String> {
        let mut url = self.base()?;
        url.path_segments_mut()
            .map_err(|_| "provider url cannot be a base".to_string())?
            .pop_if_empty()
            .extend([
                "timeshift",
                &self.provider().username,
                &self.provider().password,
                &minutes.to_string(),
                start,
                &format!("{stream_id}.{ext}"),
            ]);
        Ok(url)
    }

    /// `{url}/streaming/timeshift.php?username=&password=&stream=&start=&duration=`,
    /// the archive's other common form.
    pub fn timeshift_php_url(
        &self,
        stream_id: i64,
        start: &str,
        minutes: i64,
    ) -> Result<Url, String> {
        let mut url = self.url("streaming/timeshift.php", None)?;
        url.query_pairs_mut()
            .append_pair("stream", &stream_id.to_string())
            .append_pair("start", start)
            .append_pair("duration", &minutes.to_string());
        Ok(url)
    }

    /// `get_simple_data_table` for one channel: its full guide, past
    /// included, with archive flags.
    pub async fn simple_data_table(&self, stream_id: i64) -> Result<serde_json::Value, String> {
        let mut url = self.api_url(Some("get_simple_data_table"))?;
        url.query_pairs_mut()
            .append_pair("stream_id", &stream_id.to_string());
        self.json(url, "get_simple_data_table").await
    }

    fn base(&self) -> Result<Url, String> {
        Url::parse(&self.provider().url)
            .map_err(|e| format!("{}: invalid url: {e}", self.provider().name))
    }

    pub(crate) fn api_url(&self, action: Option<&str>) -> Result<Url, String> {
        self.url("player_api.php", action)
    }

    /// `{url}/{file}?username=&password=[&action=]`.
    fn url(&self, file: &str, action: Option<&str>) -> Result<Url, String> {
        let mut url = self.base()?.join(file).map_err(|e| e.to_string())?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("username", &self.provider().username)
                .append_pair("password", &self.provider().password);
            if let Some(a) = action {
                q.append_pair("action", a);
            }
        }
        Ok(url)
    }

    /// Small responses only: buffers the whole body.
    async fn api<T: for<'de> Deserialize<'de>>(&self, action: Option<&str>) -> Result<T, String> {
        self.json(self.api_url(action)?, action.unwrap_or("account"))
            .await
    }

    async fn json<T: for<'de> Deserialize<'de>>(&self, url: Url, what: &str) -> Result<T, String> {
        let (tx, mut rx) = mpsc::channel::<Chunk>(8);
        let collect = async {
            let mut body = Vec::new();
            while let Some(Ok(chunk)) = rx.recv().await {
                body.extend_from_slice(&chunk);
            }
            body
        };
        let (fetched, body) = tokio::join!(self.fetch(url, tx), collect);
        fetched?;
        // Position only: serde's message can quote values from the body.
        serde_json::from_slice(&body).map_err(|e| {
            format!(
                "{}: unexpected {what} response at line {} column {}",
                self.provider().name,
                e.line(),
                e.column()
            )
        })
    }

    /// Sends the body to `tx` chunk by chunk. The gate is held until the body
    /// ends, so a slow download still counts as the one request in flight.
    /// Failures are also sent down `tx`, so the consumer never mistakes a cut
    /// body for a complete one. Stops early if the consumer hangs up.
    pub async fn fetch(&self, url: Url, tx: mpsc::Sender<Chunk>) -> Result<usize, String> {
        self.get(url, tx, false).await
    }

    /// [`Client::fetch`]; `track` shows the body in
    /// [`Client::stream_progress`].
    async fn get(&self, url: Url, tx: mpsc::Sender<Chunk>, track: bool) -> Result<usize, String> {
        self.get_status(url, tx, track).await.map_err(|(_, e)| e)
    }

    /// [`Client::get`], a failure saying the HTTP status if the server
    /// answered with one.
    async fn get_status(
        &self,
        url: Url,
        tx: mpsc::Sender<Chunk>,
        track: bool,
    ) -> Result<usize, (Option<StatusCode>, String)> {
        self.api_waiting.fetch_add(1, Ordering::SeqCst);
        let mut gate = self.gate.lock().await;
        if self.api_waiting.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.api_idle.notify_waiters();
        }
        tokio::time::sleep_until(gate.next_at.into()).await;
        let shown = redact(url.as_str());
        let t = Instant::now();
        log::debug!("GET {shown}");
        let result = async {
            let mut resp = self.http.get(url).send().await?;
            let status = resp.status();
            let mut n = 0;
            if status.is_success() {
                if track {
                    self.stream_read.store(0, Ordering::Relaxed);
                    let len = resp.content_length().unwrap_or(0);
                    self.stream_len.store(len, Ordering::Relaxed);
                }
                while let Some(chunk) = resp.chunk().await? {
                    n += chunk.len();
                    self.received
                        .fetch_add(chunk.len() as u64, Ordering::Relaxed);
                    if track {
                        self.stream_read
                            .fetch_add(chunk.len() as u64, Ordering::Relaxed);
                    }
                    if tx.send(Ok(chunk)).await.is_err() {
                        break;
                    }
                }
            }
            Ok::<_, reqwest::Error>((status, n))
        }
        .await;
        if track {
            self.stream_len.store(0, Ordering::Relaxed);
        }
        gate.next_at = Instant::now() + self.limits.min_gap;
        let fail = |status: Option<StatusCode>, msg: String| async move {
            let _ = tx.send(Err(msg.clone())).await;
            Err((status, msg))
        };
        let (status, n) = match result {
            Ok(r) => r,
            Err(e) => {
                let msg = format!(
                    "{}: request failed: {}",
                    self.provider().name,
                    e.without_url()
                );
                return fail(None, msg).await;
            }
        };
        if gate.record(status, &self.limits) {
            log::warn!(
                "{shown}: HTTP {status}, backing off {}s",
                gate.backoff.as_secs()
            );
        }
        if !status.is_success() {
            let msg = format!("{}: provider returned HTTP {status}", self.provider().name);
            return fail(Some(status), msg).await;
        }
        log::info!("{shown}: {n} bytes in {} ms", t.elapsed().as_millis());
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::{archive_request, redact, server_offset, url_time, wall};

    fn parse(body: &str) -> super::Account {
        serde_json::from_str(body).unwrap()
    }

    #[test]
    fn account_fields_missing() {
        let a = parse(r#"{"user_info":{"auth":1}}"#);
        assert_eq!(a.user_info.auth, Some(1));
        assert_eq!(
            a.user_info,
            super::UserInfo {
                auth: Some(1),
                ..Default::default()
            }
        );
        assert_eq!(a.server_info, super::ServerInfo::default());
    }

    #[test]
    fn account_fields_as_strings() {
        let a = parse(
            r#"{"user_info":{"auth":"1","status":"Active","exp_date":"1804632028",
                "max_connections":"2","active_cons":"1","is_trial":"0",
                "created_at":"1773360000","allowed_output_formats":["m3u8","ts"],
                "message":"Welcome","username":"alice","password":"hunter2"},
               "server_info":{"url":"h.example","port":"8080","https_port":"8443",
                "rtmp_port":"1935","server_protocol":"http","timezone":"UTC",
                "timestamp_now":"1790000000","time_now":"2026-09-30 14:05:12"}}"#,
        );
        let u = &a.user_info;
        assert_eq!(u.auth, Some(1));
        assert_eq!(u.status.as_deref(), Some("Active"));
        assert_eq!(u.exp_date, Some(1_804_632_028));
        assert_eq!((u.max_connections, u.active_cons), (Some(2), Some(1)));
        assert_eq!(u.is_trial, Some(false));
        assert_eq!(u.created_at, Some(1_773_360_000));
        assert_eq!(u.allowed_output_formats, ["m3u8", "ts"]);
        assert_eq!(u.message.as_deref(), Some("Welcome"));
        let s = &a.server_info;
        assert_eq!(s.port.as_deref(), Some("8080"));
        assert_eq!(s.https_port.as_deref(), Some("8443"));
        assert_eq!(s.rtmp_port.as_deref(), Some("1935"));
        assert_eq!(s.server_protocol.as_deref(), Some("http"));
        assert_eq!(s.timezone.as_deref(), Some("UTC"));
        assert_eq!(s.time_now.as_deref(), Some("2026-09-30 14:05:12"));
        // The echoed credentials are not kept.
        let debug = format!("{a:?}");
        assert!(
            !debug.contains("alice") && !debug.contains("hunter2"),
            "{debug}"
        );
    }

    #[test]
    fn account_fields_as_numbers() {
        let a = parse(
            r#"{"user_info":{"auth":1,"status":"Active","exp_date":1804632028,
                "max_connections":2,"active_cons":0,"is_trial":1,"created_at":1773360000,
                "allowed_output_formats":["ts",1],"message":""},
               "server_info":{"port":8080,"https_port":8443,"rtmp_port":1935}}"#,
        );
        let u = &a.user_info;
        assert_eq!(u.exp_date, Some(1_804_632_028));
        assert_eq!((u.max_connections, u.active_cons), (Some(2), Some(0)));
        assert_eq!(u.is_trial, Some(true));
        assert_eq!(u.allowed_output_formats, ["ts", "1"]);
        assert_eq!(u.message, None);
        assert_eq!(a.server_info.port.as_deref(), Some("8080"));
    }

    #[test]
    fn archive_days_from_numbers_strings_zero_and_missing() {
        let days = |fields: &str| {
            let s: super::Stream =
                serde_json::from_str(&format!(r#"{{"stream_id":1{fields}}}"#)).unwrap();
            s.archive_days()
        };
        assert_eq!(days(r#","tv_archive":1,"tv_archive_duration":3"#), 3);
        assert_eq!(days(r#","tv_archive":"1","tv_archive_duration":"7""#), 7);
        assert_eq!(days(r#","tv_archive":1,"tv_archive_duration":" 3 ""#), 3);
        // A duration without the flag is no archive.
        assert_eq!(days(r#","tv_archive":0,"tv_archive_duration":3"#), 0);
        assert_eq!(days(r#","tv_archive":"0","tv_archive_duration":"3""#), 0);
        assert_eq!(days(r#","tv_archive_duration":3"#), 0);
        // The flag without a usable duration.
        assert_eq!(days(r#","tv_archive":1,"tv_archive_duration":0"#), 0);
        assert_eq!(days(r#","tv_archive":1"#), 0);
        assert_eq!(days(r#","tv_archive":1,"tv_archive_duration":"""#), 0);
        assert_eq!(days(r#","tv_archive":1,"tv_archive_duration":-2"#), 0);
        assert_eq!(days(r#","tv_archive":null,"tv_archive_duration":null"#), 0);
        assert_eq!(days(""), 0);
    }

    #[test]
    fn account_fields_mistyped() {
        let a = parse(
            r#"{"user_info":{"auth":1,"status":null,"exp_date":null,"is_trial":"maybe",
                "allowed_output_formats":"ts","message":{},"active_cons":[]},
               "server_info":[]}"#,
        );
        assert_eq!(
            a.user_info,
            super::UserInfo {
                auth: Some(1),
                ..Default::default()
            }
        );
        assert_eq!(a.server_info, super::ServerInfo::default());
        let a = parse(r#"{"user_info":{"auth":1,"is_trial":true},"server_info":{"port":[]}}"#);
        assert_eq!(a.user_info.is_trial, Some(true));
        assert_eq!(a.server_info.port, None);
    }

    #[test]
    fn redacts_query_credentials() {
        let r = redact(
            "http://h:8080/player_api.php?username=alice&password=s3cr%26t&action=get_live_streams",
        );
        assert_eq!(
            r,
            "http://h:8080/player_api.php?username=***&password=***&action=get_live_streams"
        );
        let r = redact("http://h/xmltv.php?password=pw&username=bob");
        assert_eq!(r, "http://h/xmltv.php?password=***&username=***");
        let r = redact("http://h/player_api.php?Username=alice&PASSWORD=pw#frag");
        assert_eq!(r, "http://h/player_api.php?Username=***&PASSWORD=***#frag");
    }

    #[test]
    fn leaves_lookalike_params_alone() {
        let r = redact("http://h/x?myusername=keep&action=a");
        assert_eq!(r, "http://h/x?myusername=keep&action=a");
    }

    #[test]
    fn redacts_live_path_credentials() {
        let r = redact("http://h:8080/live/alice/pw/12345.ts");
        assert_eq!(r, "http://h:8080/live/***/***/12345.ts");
        let r = redact("error for http://h/live/alice/pw/1.m3u8 and http://h/live/bob/pw2/2.ts");
        assert_eq!(
            r,
            "error for http://h/live/***/***/1.m3u8 and http://h/live/***/***/2.ts"
        );
        let r = redact("http://h/movie/alice/pw/7.mkv http://h/series/alice/pw/8.mp4");
        assert_eq!(
            r,
            "http://h/movie/***/***/7.mkv http://h/series/***/***/8.mp4"
        );
    }

    #[test]
    fn no_credential_survives() {
        for url in [
            "http://h/player_api.php?username=alice&password=hunter2",
            "https://h/live/alice/hunter2/7.ts",
            "http://h/live/alice/hunter2/7.ts?username=alice&password=hunter2",
        ] {
            let r = redact(url);
            assert!(!r.contains("alice") && !r.contains("hunter2"), "{r}");
        }
    }

    #[test]
    fn url_time_round_trips_wall() {
        let t = wall("2026-10-01 20:05:00").unwrap();
        assert_eq!(url_time(t), "2026-10-01:20-05");
        assert_eq!(
            url_time(wall("2024-02-29 00:00:00").unwrap()),
            "2024-02-29:00-00"
        );
    }

    #[test]
    fn server_offset_is_rounded_to_15_min() {
        let utc = wall("2026-10-02 12:00:00").unwrap();
        assert_eq!(server_offset("2026-10-02 14:00:07", utc), Some(7200));
        // A clock a few minutes off still lands on the zone.
        assert_eq!(server_offset("2026-10-02 13:53:00", utc), Some(7200));
        assert_eq!(server_offset("2026-10-02 06:30:00", utc), Some(-19_800));
        assert_eq!(server_offset("2026-10-02 12:00:00", utc), Some(0));
        assert_eq!(server_offset("soon", utc), None);
    }

    #[test]
    fn archive_request_covers_the_programme_in_server_time() {
        let start = wall("2026-10-01 20:00:30").unwrap();
        let stop = wall("2026-10-01 20:45:00").unwrap();
        let (at, minutes, from) = archive_request(start, stop, 7200, 300);
        // From the minute below, in UTC+2, with 5 min after.
        assert_eq!(at, "2026-10-01:22-00");
        assert_eq!(from, wall("2026-10-01 20:00:00").unwrap());
        assert_eq!(minutes, 50);
        // Across midnight, rounded up to whole minutes.
        let (at, minutes, _) = archive_request(start + 3 * 3600, stop + 3 * 3600 + 20, 7200, 0);
        assert_eq!(at, "2026-10-02:01-00");
        assert_eq!(minutes, 46);
    }

    /// The offset from the account check, then a past programme's archive
    /// Onboarding ticket 02: an edited provider's URL and credentials are
    /// used from the next request, its other requests going on as before.
    #[tokio::test]
    async fn an_edited_provider_is_used_from_the_next_request() {
        use crate::fake_xtream::{self, FakeXtream};
        let old = FakeXtream::start(fake_xtream::active).await;
        let new = FakeXtream::start(fake_xtream::active).await;
        use std::time::Duration;
        let fast = super::Limits {
            min_gap: Duration::from_millis(10),
            backoff_start: Duration::from_millis(200),
            backoff_max: Duration::from_millis(800),
        };
        let c = super::Client::with_limits(old.provider(), fast);
        c.account().await.unwrap();
        c.set_provider(crate::config::Provider {
            password: "changed".into(),
            ..new.provider()
        });
        c.account().await.unwrap();
        assert_eq!(old.targets().len(), 1);
        let target = new.targets().pop().unwrap();
        assert!(target.contains("password=changed"), "{target}");
        let url = c.stream_url(10, "ts").unwrap();
        assert!(url.as_str().starts_with(new.url()), "{url}");
        assert!(url.path().contains("/changed/"), "{url}");
    }

    /// URL in ticket 01's form, as the fake provider receives it.
    #[tokio::test]
    async fn archive_url_from_the_account_check() {
        use crate::fake_xtream::{self, FakeXtream};
        let fake = FakeXtream::start(fake_xtream::active).await;
        let c = super::Client::new(fake.provider());
        let checked = crate::import::unix_now();
        let account = c.account().await.unwrap();
        let time_now = account.server_info.time_now.unwrap();
        let offset = server_offset(&time_now, checked).unwrap();
        assert_eq!(offset, fake_xtream::SERVER_OFFSET);

        let stop = checked - checked % 3600 - 86_400;
        let start = stop - 2700;
        let (at, minutes, _) = archive_request(start, stop, offset, 300);
        let url = c.timeshift_url(10, &at, minutes, "ts").unwrap();
        let status = reqwest::get(url).await.unwrap().status();
        assert_eq!(status, 200);
        let target = fake.targets().pop().unwrap();
        assert_eq!(
            target,
            format!("/timeshift/user/pass/50/{}/10.ts", url_time(start + 7200))
        );
    }

    #[test]
    fn stream_url_round_trips_through_redactor() {
        let c = super::Client::new(crate::config::Provider {
            id: "main".into(),
            name: "P".into(),
            url: "http://h:8080".into(),
            username: "al ice".into(),
            password: "p/w?".into(),
            epg: Vec::new(),
        });
        let url = c.stream_url(42, "ts").unwrap();
        assert_eq!(redact(url.as_str()), "http://h:8080/live/***/***/42.ts");
        let url = c.timeshift_url(42, "2026-10-01:20-00", 60, "ts").unwrap();
        assert_eq!(
            redact(url.as_str()),
            "http://h:8080/timeshift/***/***/60/2026-10-01:20-00/42.ts"
        );
        let url = c.timeshift_php_url(42, "2026-10-01:20-00", 60).unwrap();
        let shown = redact(url.as_str());
        assert!(!shown.contains("al") && !shown.contains("p%2Fw"), "{shown}");
        assert!(
            shown.contains("stream=42&start=2026-10-01%3A20-00&duration=60"),
            "{shown}"
        );
    }

    /// The add-provider form's check (onboarding ticket 01): each outcome
    /// told apart, and no credential in its messages or the log.
    #[tokio::test]
    async fn account_check_tells_each_outcome_apart() {
        use super::{Blocked, Check, Client};
        use crate::fake_xtream::{self, FakeXtream};
        let logs = fake_xtream::capture_logs();
        let secret = |url: String| crate::config::Provider {
            id: "new".into(),
            name: "New".into(),
            url,
            username: "alice-7f3k".into(),
            password: "hunter2-9q".into(),
            epg: Vec::new(),
        };
        let check = |handler: fn(&str) -> (u16, String)| async move {
            let fake = FakeXtream::start(handler).await;
            Client::new(secret(fake.url().into())).check().await
        };
        let now = crate::import::unix_now();
        let mut said = Vec::new();
        match check(fake_xtream::active).await {
            Check::Account(a) => {
                assert_eq!(a.user_info.blocked(now), None);
                assert_eq!(a.user_info.max_connections, Some(2));
            }
            other => panic!("{other:?}"),
        }
        match check(fake_xtream::expired).await {
            Check::Account(a) => assert_eq!(a.user_info.blocked(now), Some(Blocked::Expired)),
            other => panic!("{other:?}"),
        }
        match check(fake_xtream::disabled).await {
            Check::Account(a) => assert!(matches!(
                a.user_info.blocked(now),
                Some(Blocked::Disabled(_))
            )),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            check(fake_xtream::rejected).await,
            Check::Rejected
        ));
        // Some providers refuse a wrong password with HTTP 513.
        assert!(matches!(
            check(|_| (513, String::new())).await,
            Check::Rejected
        ));
        for handler in [
            |_: &str| (404, "<html>Not found</html>".to_string()),
            |_: &str| (200, "<html>A router's login page</html>".to_string()),
            |_: &str| (200, r#"{"echo":"alice-7f3k hunter2-9q"}"#.to_string()),
        ] {
            match check(handler).await {
                Check::NotXtream(e) => said.push(e),
                other => panic!("{other:?}"),
            }
        }
        for (code, handler) in [
            (
                401,
                (|_: &str| (401, String::new())) as fn(&str) -> (u16, String),
            ),
            (403, |_: &str| (403, String::new())),
        ] {
            match check(handler).await {
                Check::Forbidden(s) => assert_eq!(s.as_u16(), code),
                other => panic!("{other:?}"),
            }
        }
        match check(|_| (503, String::new())).await {
            Check::Unreachable(e) => said.push(e),
            other => panic!("{other:?}"),
        }
        // A port nothing listens on.
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        match Client::new(secret(url)).check().await {
            Check::Unreachable(e) => said.push(e),
            other => panic!("{other:?}"),
        }
        let logs = logs.lock().unwrap();
        assert!(logs.iter().any(|l| l.contains("GET ")), "{logs:?}");
        for line in said.iter().chain(logs.iter()) {
            assert!(
                !line.contains("alice-7f3k") && !line.contains("hunter2-9q"),
                "{line}"
            );
        }
    }

    /// SPEC 6.2 with the times scaled down: 2 s gap, backoff from 30 s
    /// doubling to 30 min, reset by a success.
    #[tokio::test(flavor = "multi_thread")]
    async fn backoff_timings() {
        use crate::fake_xtream::{self, FakeXtream};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;
        const MS: Duration = Duration::from_millis(1);
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        // 503, 429, 500, 503, then fine.
        let fake = FakeXtream::start(|target: &str| match CALLS.fetch_add(1, Ordering::SeqCst) {
            0 | 3 => (503, String::new()),
            1 => (429, String::new()),
            2 => (500, String::new()),
            _ => fake_xtream::active(target),
        })
        .await;
        let limits = super::Limits {
            min_gap: 30 * MS,
            backoff_start: 150 * MS,
            backoff_max: 400 * MS,
        };
        let c = super::Client::with_limits(fake.provider(), limits);
        for i in 0..4 {
            let err = c.account().await.unwrap_err();
            assert!(err.contains("HTTP"), "{i}: {err}");
            assert!(c.backoff_until().is_some());
        }
        c.account().await.unwrap();
        assert!(c.backoff_until().is_none());
        c.account().await.unwrap();

        let t = fake.times();
        let gaps: Vec<Duration> = t.windows(2).map(|w| w[1] - w[0]).collect();
        let expected = [150, 300, 400, 400, 30];
        for (gap, want) in gaps.iter().zip(expected) {
            let want = want * MS;
            assert!(
                *gap >= want && *gap < want + 100 * MS,
                "gaps {gaps:?}, expected {expected:?} ms"
            );
        }
    }
}
