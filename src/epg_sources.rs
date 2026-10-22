//! External guide sources (EPG sources ticket 02): XMLTV feeds by URL or
//! path, set per provider in `credentials.toml` and ordered with its own
//! `xmltv.php`. Each is streamed, gzipped or plain, into one transaction:
//! its whole channel list, which provider channels it matches, and the
//! programmes of matched channels only, shifted and in the kept window
//! (SPEC 7.1). A failure rolls back to its last good data and never fails
//! the provider's refresh. URLs can hold a token, so logs and errors name a
//! source by its label only.

use crate::config::{EpgSource, Provider, epg_order};
use crate::import::{
    ChunkReader, EPG_KEEP_AHEAD, EPG_KEEP_PAST, FeedChannel, Item, Writer, days_from_civil,
    gunzipped, parse_xmltv_items, unix_now,
};
use crate::store::{self, NewProgramme, SourceFetch, name_norm};
use crate::xtream::{self, Chunk};
use reqwest::StatusCode;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, RETRY_AFTER};
use rusqlite::Connection;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;

/// An enabled external source, by its stored id. `due` when a scheduled
/// pass would fetch it now: the EPG stage is then due (SPEC 7.2).
pub struct Planned {
    pub id: i64,
    pub source: EpgSource,
    pub due: bool,
    /// A scheduled pass gives it a turn: due, or held back by its cap or
    /// a backoff, which the refresh panel says (EPG sources ticket 04).
    pub turn: bool,
    /// Marked for a whole fetch by a mapping (EPG sources ticket 05).
    pub reimport: bool,
}

/// How far a source's fetch has got, for its refresh panel row (EPG
/// sources ticket 04): body bytes read, and the length if the server sent
/// one, else 0. Its requests do not go through the provider's client.
#[derive(Debug, Default)]
pub struct Meter {
    read: AtomicU64,
    len: AtomicU64,
}

impl Meter {
    /// Bytes read, and the read and the length while the length is known.
    pub fn progress(&self) -> (u64, Option<(u64, u64)>) {
        let (read, len) = (
            self.read.load(Ordering::Relaxed),
            self.len.load(Ordering::Relaxed),
        );
        (read, (len > 0).then_some((read, len)))
    }
}

/// What a source's turn in the EPG stage came to (EPG sources ticket 03).
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// One conditional request said the feed has not changed: nothing
    /// imported. A file whose modification time has not is not due.
    Unchanged,
    /// Fetched or read and imported: the programmes kept.
    Fetched(usize),
    /// Not asked: `count` of `max_per_day` requests made in the last 24 h,
    /// and when the next may go.
    AtCap {
        count: usize,
        max: u32,
        next: i64,
    },
    /// A 429 or 503 with `Retry-After`: not asked until `retry_at`.
    Backoff {
        retry_at: i64,
    },
    Failed(String),
}

/// What a pass does with one source.
#[derive(Debug, PartialEq)]
enum Plan {
    Fetch,
    /// Not due: left out.
    Wait,
    /// Due but held back.
    Skip(Outcome),
}

/// Requests a scheduled fetch leaves under `max_per_day`, kept for `r`.
const RESERVE: usize = 2;

/// Whether source `source`, last fetched as `f`, is fetched at `now`. A
/// URL is due once `every_hours` have passed since its last request, a
/// path when its modification time `mtime` is not the one last read; a new
/// one, one marked for a re-import, one whose backoff has ended, or any
/// with `force` (`r`) at once.
/// Then a backoff holds it, and so does `max_per_day` over the last 24 h,
/// less [`RESERVE`] unless `force`.
fn plan(source: &EpgSource, f: &SourceFetch, mtime: Option<i64>, force: bool, now: i64) -> Plan {
    let due = force
        || f.new
        || f.reimport
        || f.retry_at.is_some()
        || match source.url {
            Some(_) => f.fetched.is_none_or(|t| now - t >= source.every()),
            None => mtime != f.mtime,
        };
    if !due {
        return Plan::Wait;
    }
    if let Some(retry_at) = f.retry_at.filter(|&t| t > now) {
        return Plan::Skip(Outcome::Backoff { retry_at });
    }
    if let Some(max) = source.max_per_day {
        let limit = if force {
            max as usize
        } else {
            (max as usize).saturating_sub(RESERVE)
        };
        let count = f.requests.len();
        if count >= limit {
            // When enough requests are 24 h old to bring it under the limit.
            let next = f.requests.get(count - limit).map_or(now, |t| t + 86_400);
            return Plan::Skip(Outcome::AtCap { count, max, next });
        }
    }
    Plan::Fetch
}

/// A file's modification time in nanoseconds, if it can be read.
fn file_mtime(path: &Path) -> Option<i64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(d.as_nanos()).ok()
}

/// Stores the provider's sources in their order, its own guide first unless
/// an entry places it, and returns the enabled external ones in order.
pub fn sync(conn: &mut Connection, provider: &Provider, now: i64) -> Result<Vec<Planned>, String> {
    let sources = epg_order(provider);
    let idents: Vec<String> = sources.iter().map(EpgSource::ident).collect();
    let entries: Vec<store::SourceEntry> = sources
        .iter()
        .zip(&idents)
        .map(|(s, ident)| store::SourceEntry {
            ident,
            enabled: s.enabled,
            shift_minutes: s.shift_minutes,
        })
        .collect();
    let ids = store::sync_sources(conn, &provider.id, &entries)
        .map_err(|e| format!("saving guide sources: {e}"))?;
    ids.into_iter()
        .zip(sources)
        .filter(|(_, s)| s.enabled && !s.provider_guide)
        .map(|(id, source)| {
            let f = store::source_fetch(conn, id, now)
                .map_err(|e| format!("reading guide sources: {e}"))?;
            let mtime = source.path.as_deref().and_then(file_mtime);
            let plan = plan(&source, &f, mtime, false, now);
            Ok(Planned {
                id,
                source,
                due: plan == Plan::Fetch,
                turn: plan != Plan::Wait,
                reimport: f.reimport,
            })
        })
        .collect()
}

/// Fetches or reads one source and imports it, if [`plan`] says so: `None`
/// when it is not due. Every request to a URL is counted first. A failure
/// is stored with the source; its last good data stays.
pub async fn import(
    writer: &Writer,
    provider_id: &str,
    planned: &Planned,
    force: bool,
    meter: &Meter,
) -> Option<Outcome> {
    let now = unix_now();
    let (id, source) = (planned.id, &planned.source);
    let mtime = source.path.as_deref().and_then(file_mtime);
    let result = {
        let conn = writer.lock().unwrap_or_else(|e| e.into_inner());
        store::source_fetch(&conn, id, now).and_then(|f| {
            let plan = plan(source, &f, mtime, force, now);
            if plan == Plan::Fetch && source.url.is_some() {
                store::source_requested(&conn, id, now)?;
            }
            Ok((f, plan))
        })
    };
    let f = match result {
        Ok((f, Plan::Fetch)) => f,
        Ok((_, Plan::Wait)) => return None,
        Ok((_, Plan::Skip(outcome))) => {
            log::info!("guide source {}: skipped, {outcome:?}", source.label());
            return Some(outcome);
        }
        Err(e) => return Some(Outcome::Failed(format!("reading guide sources: {e}"))),
    };
    let shift = source.shift_minutes * 60;
    let provider_id = provider_id.to_owned();
    let result = match (&source.url, &source.path) {
        (Some(url), _) => fetch(writer, &provider_id, id, url, &f, shift, now, meter).await,
        (None, Some(path)) => {
            let (job_writer, path) = (writer.clone(), path.clone());
            let read = tokio::task::spawn_blocking(move || {
                let file = std::fs::File::open(&path).map_err(|e| format!("cannot read: {e}"))?;
                let mut conn = job_writer.lock().unwrap_or_else(|e| e.into_inner());
                write_source(&mut conn, &provider_id, id, file, shift, unix_now())
            })
            .await
            .map_err(|e| format!("import task failed: {e}"))
            .and_then(|r| r);
            // Read or not, this file is done with until it changes.
            let conn = writer.lock().unwrap_or_else(|e| e.into_inner());
            store::source_mtime(&conn, id, mtime)
                .map_err(|e| format!("saving guide source: {e}"))
                .and(read)
                .map(Outcome::Fetched)
        }
        (None, None) => Err("no url or path".into()),
    };
    let label = source.label();
    Some(match result {
        Ok(outcome) => {
            log::info!("guide source {label}: {outcome:?}");
            outcome
        }
        Err(e) => {
            log::warn!("guide source {label}: {e}");
            let conn = writer.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = store::source_failed(&conn, id, &e) {
                log::error!("saving guide source: {e}");
            }
            Outcome::Failed(e)
        }
    })
}

/// Asks `url` with `f`'s validators and imports a new feed. A 304 keeps
/// the data; a 429 or 503 with `Retry-After` backs the source off. Errors
/// carry no URL.
#[allow(clippy::too_many_arguments)]
async fn fetch(
    writer: &Writer,
    provider_id: &str,
    id: i64,
    url: &str,
    f: &SourceFetch,
    shift: i64,
    now: i64,
    meter: &Meter,
) -> Result<Outcome, String> {
    let mut req = xtream::http().get(url);
    if let Some(etag) = &f.etag {
        req = req.header(IF_NONE_MATCH, etag);
    }
    if let Some(modified) = &f.last_modified {
        req = req.header(IF_MODIFIED_SINCE, modified);
    }
    let resp = req.send().await.map_err(|e| e.without_url().to_string())?;
    let status = resp.status();
    let header = |name| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let (etag, modified) = (header(ETAG), header(LAST_MODIFIED));
    let db = |e: rusqlite::Error| format!("saving guide source: {e}");
    if matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
    ) && let Some(retry_at) = header(RETRY_AFTER).and_then(|v| retry_after(&v, now))
    {
        let conn = writer.lock().unwrap_or_else(|e| e.into_inner());
        store::source_backoff(&conn, id, retry_at).map_err(db)?;
        return Ok(Outcome::Backoff { retry_at });
    }
    if status == StatusCode::NOT_MODIFIED {
        let etag = etag.or_else(|| f.etag.clone());
        let modified = modified.or_else(|| f.last_modified.clone());
        let conn = writer.lock().unwrap_or_else(|e| e.into_inner());
        store::source_current(&conn, id, etag.as_deref(), modified.as_deref()).map_err(db)?;
        return Ok(Outcome::Unchanged);
    }
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let (tx, rx) = mpsc::channel(8);
    let job = {
        let (writer, provider_id) = (writer.clone(), provider_id.to_owned());
        tokio::task::spawn_blocking(move || {
            let mut conn = writer.lock().unwrap_or_else(|e| e.into_inner());
            write_source(
                &mut conn,
                &provider_id,
                id,
                ChunkReader::new(rx),
                shift,
                now,
            )
        })
    };
    meter
        .len
        .store(resp.content_length().unwrap_or(0), Ordering::Relaxed);
    let streamed = stream(resp, tx, meter).await;
    let stored = job.await.map_err(|e| format!("import task failed: {e}"));
    // A network error explains a parse error, so it wins.
    let n = streamed.and(stored.and_then(|r| r))?;
    let conn = writer.lock().unwrap_or_else(|e| e.into_inner());
    store::source_current(&conn, id, etag.as_deref(), modified.as_deref()).map_err(db)?;
    Ok(Outcome::Fetched(n))
}

/// Streams `resp`'s body into `tx`, counting it on `meter`. Errors carry
/// no URL.
async fn stream(
    mut resp: reqwest::Response,
    tx: mpsc::Sender<Chunk>,
    meter: &Meter,
) -> Result<(), String> {
    let streamed = async {
        while let Some(chunk) = resp.chunk().await? {
            meter.read.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            if tx.send(Ok(chunk)).await.is_err() {
                break;
            }
        }
        Ok::<_, reqwest::Error>(())
    }
    .await
    .map_err(|e| e.without_url().to_string());
    if let Err(e) = &streamed {
        let _ = tx.send(Err(e.clone())).await;
    }
    streamed
}

/// A `Retry-After` value, seconds or an HTTP date
/// (`Sun, 06 Nov 1994 08:49:37 GMT`), as a time.
fn retry_after(value: &str, now: i64) -> Option<i64> {
    let value = value.trim();
    if let Ok(secs) = value.parse::<u32>() {
        return Some(now + i64::from(secs));
    }
    let [_, day, month, year, time, "GMT"] = value.split_whitespace().collect::<Vec<_>>()[..]
    else {
        return None;
    };
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = MONTHS.iter().position(|&m| m == month)? as i64 + 1;
    let hms: Vec<i64> = time
        .split(':')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let [h, m, s] = hms[..] else { return None };
    let days = days_from_civil(year.parse().ok()?, month, day.parse().ok()?);
    Some(days * 86_400 + h * 3600 + m * 60 + s)
}

/// Replaces source `source`'s channels, matches and programmes with those
/// in `body`, an XMLTV document, gzipped or not, in one transaction. The
/// provider's channels are matched once the feed's channels are in, at its
/// first programme (XMLTV lists channels first). Programmes of matched
/// channels are kept, moved by `shift` seconds, from as far back as the
/// longest archive among the channels matched to each (else 1 h) to 3 days
/// ahead. Returns how many were.
pub fn write_source(
    conn: &mut Connection,
    provider_id: &str,
    source: i64,
    body: impl Read,
    shift: i64,
    now: i64,
) -> Result<usize, String> {
    let db = |e: rusqlite::Error| format!("saving guide: {e}");
    let tx = conn.transaction().map_err(db)?;
    store::begin_source_import(&tx, provider_id, source, now).map_err(db)?;
    let xml = gunzipped(body).map_err(|e| format!("download interrupted: {e}"))?;
    let mut channels = Vec::new();
    // Feed channel id to how far back its programmes are kept.
    let mut keep: Option<HashMap<String, i64>> = None;
    let until = now + EPG_KEEP_AHEAD;
    let (mut kept, mut outside, mut unmatched) = (0, 0, 0);
    let skipped = parse_xmltv_items(xml, |item| match item {
        Item::Channel(c) => {
            store::insert_source_channel(&tx, source, &c.id, &c.names, c.icon.as_deref())
                .map_err(db)?;
            channels.push(c.clone());
            Ok(())
        }
        Item::Programme(p) => {
            if keep.is_none() {
                let all = std::mem::take(&mut channels);
                keep = Some(match_channels(&tx, provider_id, source, &all).map_err(db)?);
            }
            let Some(&past) = keep.as_ref().and_then(|k| k.get(p.epg_channel_id)) else {
                unmatched += 1;
                return Ok(());
            };
            let (start, stop) = (p.start + shift, p.stop + shift);
            if stop <= now - past || start >= until {
                outside += 1;
                return Ok(());
            }
            kept += 1;
            let guide_id = store::source_guide_id(source, p.epg_channel_id);
            let p = NewProgramme {
                epg_channel_id: &guide_id,
                start,
                stop,
                ..*p
            };
            store::insert_programme(&tx, provider_id, &p).map_err(db)
        }
    })?;
    if keep.is_none() {
        match_channels(&tx, provider_id, source, &channels).map_err(db)?;
    }
    log::info!(
        "{kept} source programmes imported, {outside} outside the window, {unmatched} on unmatched channels, {skipped} unusable"
    );
    store::finish_source_import(&tx, provider_id, source, now).map_err(db)?;
    tx.commit().map_err(db)?;
    Ok(kept)
}

/// Matches each of the provider's channels to `channels`: its manual
/// mapping while the feed has that channel, else by [`Index`]; stores the
/// matches and the ambiguous. Returns each matched feed channel's kept
/// past, in seconds.
fn match_channels(
    tx: &Connection,
    provider_id: &str,
    source: i64,
    channels: &[FeedChannel],
) -> rusqlite::Result<HashMap<String, i64>> {
    let index = Index::new(channels, &[name_norm, epg_norm]);
    let mapped = store::source_mappings(tx, provider_id, source)?;
    let mut keep = HashMap::new();
    store::each_channel(
        tx,
        provider_id,
        |stream_id, guide_id, name, archive_days| {
            let manual = mapped.get(&stream_id).and_then(|id| index.id(id));
            match manual.map_or_else(|| index.find(guide_id, name), Match::Id) {
                Match::Id(i) | Match::Name(i) => {
                    let id = &channels[i].id;
                    let guide_id = store::source_guide_id(source, id);
                    store::insert_source_match(tx, source, stream_id, Some(&guide_id))?;
                    let past = match archive_days {
                        0 => EPG_KEEP_PAST,
                        days => days * 86_400,
                    };
                    let e = keep.entry(id.clone()).or_insert(past);
                    *e = (*e).max(past);
                }
                Match::Ambiguous => store::insert_source_match(tx, source, stream_id, None)?,
                Match::None => {}
            }
            Ok(())
        },
    )?;
    Ok(keep)
}

/// How one provider channel matched a feed: the feed channel's index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Match {
    Id(usize),
    Name(usize),
    Ambiguous,
    None,
}

/// A display name normaliser.
pub type Norm = fn(&str) -> String;

/// A feed's channels by id and by each normalised display name, in order.
/// Each holds distinct feed channels, so a step matches only on exactly
/// one.
pub struct Index {
    ids: HashMap<String, Vec<usize>>,
    steps: Vec<(Norm, HashMap<String, Vec<usize>>)>,
}

impl Index {
    pub fn new(channels: &[FeedChannel], norms: &[Norm]) -> Self {
        let mut ids = HashMap::<_, Vec<_>>::new();
        for (i, c) in channels.iter().enumerate() {
            ids.entry(c.id.clone()).or_default().push(i);
        }
        let steps = norms
            .iter()
            .map(|&norm| {
                let mut names = HashMap::<_, Vec<_>>::new();
                for (i, c) in channels.iter().enumerate() {
                    for n in &c.names {
                        let v = names.entry(norm(n)).or_default();
                        if v.last() != Some(&i) {
                            v.push(i);
                        }
                    }
                }
                names.remove("");
                (norm, names)
            })
            .collect();
        Self { ids, steps }
    }

    /// The first feed channel with id `id`.
    pub fn id(&self, id: &str) -> Option<usize> {
        self.ids.get(id).and_then(|v| v.first().copied())
    }

    /// Exact id first, then each normalised name. Two or more at a step is
    /// ambiguous, never a guess.
    pub fn find(&self, guide_id: Option<&str>, name: &str) -> Match {
        if let Some(found) = guide_id.and_then(|g| self.ids.get(g)) {
            return match found[..] {
                [i] => Match::Id(i),
                _ => Match::Ambiguous,
            };
        }
        for (norm, names) in &self.steps {
            match names.get(&norm(name)).map(|v| &v[..]) {
                Some(&[i]) => return Match::Name(i),
                Some(_) => return Match::Ambiguous,
                None => {}
            }
        }
        Match::None
    }
}

/// [`name_norm`] after dropping a region prefix (`UK| `, `|UK| `, `[UK] `,
/// `US: `), bracketed text and punctuation: the looser name step, tried
/// when `name_norm` finds nothing (SPEC 6.1).
pub fn epg_norm(name: &str) -> String {
    let s = strip_region(name.trim());
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            c if c.is_alphanumeric() || c == '+' => out.push(c),
            _ => out.push(' '),
        }
    }
    name_norm(&out)
}

/// The name without a leading 2 to 4 letter or digit tag closed by `|`,
/// `:`, `]`, `)` or `-`, if anything is left.
fn strip_region(s: &str) -> &str {
    let t = s.trim_start_matches(['|', '[', '(']).trim_start();
    let tag = t.find(|c: char| !c.is_alphanumeric()).unwrap_or(t.len());
    if !(2..=4).contains(&t[..tag].chars().count()) {
        return s;
    }
    let rest = t[tag..].trim_start();
    match rest.strip_prefix(['|', ':', ']', ')', '-']) {
        Some(r) if !r.trim().is_empty() => r.trim_start(),
        _ => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_xtream::{FakeXtream, Reply, xmltv_stamp};
    use crate::import::write_epg;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const NOW: i64 = 1_700_000_000;
    const H: i64 = 3600;

    fn url(u: &str) -> EpgSource {
        EpgSource {
            url: Some(u.into()),
            path: None,
            provider_guide: false,
            name: None,
            shift_minutes: 0,
            enabled: true,
            every_hours: None,
            max_per_day: None,
        }
    }

    fn provider_guide(enabled: bool) -> EpgSource {
        EpgSource {
            url: None,
            provider_guide: true,
            enabled,
            ..url("")
        }
    }

    fn provider(epg: Vec<EpgSource>) -> Provider {
        Provider {
            id: "p".into(),
            name: "P".into(),
            url: "http://h".into(),
            username: "u".into(),
            password: "pw".into(),
            epg,
            source: crate::config::Source::Xtream,
            user_agent: None,
        }
    }

    /// Channels `(stream_id, name, guide id, archive days)`: Harbour One
    /// with the provider's guide and a 3-day archive, Kestrel with a guide
    /// id the provider's guide lacks, Summit and Juniper with none.
    fn db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = store::open(&dir.path().join("v.db")).unwrap();
        import_channels(
            &mut conn,
            &[
                (1, "UK: Harbour One", Some("h1"), 3),
                (2, "UK: Kestrel", Some("k1"), 0),
                (3, "UK: Summit", None, 0),
                (4, "UK: Juniper", None, 0),
            ],
        );
        let guide = doc(&[], &[("h1", NOW - H / 2, NOW + H / 2)]);
        write_epg(&mut conn, "p", guide.as_bytes(), 0, NOW).unwrap();
        (dir, conn)
    }

    /// Replaces the provider's channels with `(stream_id, name, guide id,
    /// archive days)`.
    fn import_channels(conn: &mut Connection, chans: &[(i64, &str, Option<&str>, i64)]) {
        let tx = conn.transaction().unwrap();
        store::begin_channel_import(&tx, "p", "P", []).unwrap();
        for &(id, name, epg, archive_days) in chans {
            let c = store::NewChannel {
                stream_id: id,
                num: Some(id),
                name,
                category_id: None,
                epg_channel_id: epg,
                icon_url: None,
                extension: "ts",
                archive_days,
                url: None,
                tvg_name: None,
                headers: None,
            };
            store::insert_channel(&tx, "p", &c).unwrap();
        }
        store::finish_channel_import(&tx, "p", NOW).unwrap();
        tx.commit().unwrap();
    }

    /// An XMLTV document of channels `(id, name)` and programmes `(channel,
    /// start, stop)`, in UTC.
    fn doc(channels: &[(&str, &str)], progs: &[(&str, i64, i64)]) -> String {
        let mut out = String::from("<tv>");
        for (id, name) in channels {
            out += &format!(
                r#"<channel id="{id}"><display-name>{name}</display-name><icon src="http://img/{id}.png"/></channel>"#
            );
        }
        for (id, a, b) in progs {
            out += &format!(
                r#"<programme start="{}" stop="{}" channel="{id}"><title>{id} show</title></programme>"#,
                xmltv_stamp(*a, 0),
                xmltv_stamp(*b, 0)
            );
        }
        out + "</tv>"
    }

    /// Source A: Harbour One by id, Kestrel and Juniper by name, Summit
    /// twice; each airing now, Harbour One and Kestrel also two days back.
    fn feed() -> String {
        doc(
            &[
                ("h1", "Harbour One"),
                ("kestrel.ext", "Kestrel"),
                ("summit.a", "Summit"),
                ("summit.b", "Summit HD"),
                ("jun", "Juniper"),
            ],
            &[
                ("h1", NOW - 2 * 86_400, NOW - 2 * 86_400 + H),
                ("kestrel.ext", NOW - 2 * 86_400, NOW - 2 * 86_400 + H),
                ("h1", NOW - H / 2, NOW + H / 2),
                ("kestrel.ext", NOW - H / 2, NOW + H / 2),
                ("summit.a", NOW - H / 2, NOW + H / 2),
                ("summit.b", NOW - H / 2, NOW + H / 2),
                ("jun", NOW - H / 2, NOW + H / 2),
            ],
        )
    }

    /// Each channel's guide id, by stream id.
    fn guides(conn: &Connection) -> Vec<Option<String>> {
        conn.prepare("SELECT guide_id FROM channel ORDER BY stream_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// Syncs `epg` and imports source A from [`feed`]; returns its id.
    fn with_source(conn: &mut Connection, epg: Vec<EpgSource>) -> i64 {
        let planned = sync(conn, &provider(epg), NOW).unwrap();
        let a = planned[0].id;
        let shift = planned[0].source.shift_minutes * 60;
        write_source(conn, "p", a, feed().as_bytes(), shift, NOW).unwrap();
        a
    }

    #[test]
    fn order_and_fallback_take_the_first_source_with_programmes() {
        let (_dir, mut conn) = db();
        let a = with_source(
            &mut conn,
            vec![url("https://epg.example.net/a.xml?token=1")],
        );
        let gid = |id: &str| Some(store::source_guide_id(a, id));
        // The provider's guide first; Kestrel's guide id has nothing there.
        // Summit matches two feed channels, so none.
        assert_eq!(
            guides(&conn),
            [Some("h1".into()), gid("kestrel.ext"), None, gid("jun")]
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM source_match WHERE guide_id IS NULL"
            ),
            1,
            "Summit kept as ambiguous"
        );
        assert_eq!(count(&conn, "SELECT count(*) FROM source_channel"), 5);
        // Kestrel's guide reads as any other.
        let key = store::channel_key(&conn, "p", 2).unwrap().unwrap();
        let row = store::channel(&conn, key, NOW).unwrap().unwrap();
        assert_eq!(row.now.unwrap().title, "kestrel.ext show");

        // A renewed token is the same source; the provider's guide moved
        // below it gives way.
        let epg = vec![
            url("https://epg.example.net/a.xml?token=2"),
            provider_guide(true),
        ];
        assert_eq!(sync(&mut conn, &provider(epg), NOW).unwrap()[0].id, a);
        assert_eq!(
            guides(&conn),
            [gid("h1"), gid("kestrel.ext"), None, gid("jun")]
        );
        // Search and the grid list the channels with a guide.
        let view = store::View::All(store::Order::Provider);
        assert_eq!(store::guide_keys(&conn, "p", view).unwrap().len(), 3);
    }

    #[test]
    fn shift_is_applied_and_the_past_kept_by_archive() {
        let (_dir, mut conn) = db();
        let mut a = url("https://epg.example.net/a.xml");
        a.shift_minutes = 60;
        let a = with_source(&mut conn, vec![provider_guide(true), a]);
        let starts = |id: &str| -> Vec<i64> {
            conn.prepare("SELECT start FROM programme WHERE epg_channel_id = ?1 ORDER BY start")
                .unwrap()
                .query_map([store::source_guide_id(a, id)], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        // Harbour One's 3-day archive keeps two days back; Kestrel has none.
        assert_eq!(
            starts("h1"),
            [NOW - 2 * 86_400 + H, NOW + H / 2],
            "an hour later"
        );
        assert_eq!(starts("kestrel.ext"), [NOW + H / 2]);
        // Summit's are on no matched channel.
        assert!(starts("summit.a").is_empty());
    }

    #[test]
    fn catch_up_is_offered_on_an_external_programme_by_the_archive() {
        let (_dir, mut conn) = db();
        with_source(
            &mut conn,
            vec![url("https://epg.example.net/a.xml"), provider_guide(true)],
        );
        let past = |stream_id: i64| {
            let key = store::channel_key(&conn, "p", stream_id).unwrap().unwrap();
            let row = store::channel(&conn, key, NOW).unwrap().unwrap();
            let gid = row.guide_id.unwrap();
            let p = store::programmes(&conn, "p", &gid, NOW - 3 * 86_400, NOW)
                .unwrap()
                .remove(0);
            crate::ui::replayable(&p, p.stop, row.archive_days, NOW)
        };
        // Harbour One's 3-day archive holds the one two days back.
        assert_eq!(past(1), Some(Ok(())));
        // Kestrel has no archive: its past is the one airing now, not ended.
        assert_eq!(past(2), None);
    }

    #[test]
    fn a_failed_import_keeps_the_last_good_data() {
        let (_dir, mut conn) = db();
        let a = with_source(&mut conn, vec![url("https://epg.example.net/a.xml")]);
        let before = (
            guides(&conn),
            count(&conn, "SELECT count(*) FROM programme"),
        );
        let body = feed();
        let cut = &body.as_bytes()[..body.len() - 10];
        let err = write_source(&mut conn, "p", a, cut, 0, NOW + H).unwrap_err();
        assert!(
            err.contains("invalid XMLTV") || err.contains("incomplete"),
            "{err}"
        );
        assert_eq!(
            before,
            (
                guides(&conn),
                count(&conn, "SELECT count(*) FROM programme")
            )
        );
    }

    #[tokio::test]
    async fn a_failed_source_says_why_and_keeps_its_data() {
        let (dir, mut conn) = db();
        let feed_file = dir.path().join("a.xml");
        std::fs::write(&feed_file, feed()).unwrap();
        let mut a = url("");
        (a.url, a.path) = (None, Some(feed_file.clone()));
        let planned = sync(&mut conn, &provider(vec![a]), NOW).unwrap();
        let writer: Writer = std::sync::Arc::new(std::sync::Mutex::new(conn));
        // The real clock: the fixture's programmes are long past.
        assert_eq!(
            import(&writer, "p", &planned[0], false, &Meter::default()).await,
            Some(Outcome::Fetched(0))
        );
        std::fs::remove_file(&feed_file).unwrap();
        let Some(Outcome::Failed(err)) =
            import(&writer, "p", &planned[0], false, &Meter::default()).await
        else {
            panic!("read a removed file");
        };
        assert!(err.contains("cannot read"), "{err}");
        let conn = writer.lock().unwrap();
        let error: Option<String> = conn
            .query_row(
                "SELECT error FROM epg_source WHERE id = ?1",
                [planned[0].id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(error, Some(err));
        assert_eq!(count(&conn, "SELECT count(*) FROM source_channel"), 5);
        assert_eq!(count(&conn, "SELECT count(*) FROM source_match"), 4);
    }

    #[test]
    fn a_disabled_source_is_skipped_and_kept_and_a_removed_one_deleted() {
        let (_dir, mut conn) = db();
        let a = with_source(&mut conn, vec![url("https://epg.example.net/a.xml")]);
        let programmes = count(&conn, "SELECT count(*) FROM programme");
        let mut off = url("https://epg.example.net/a.xml");
        off.enabled = false;
        assert!(
            sync(&mut conn, &provider(vec![off]), NOW)
                .unwrap()
                .is_empty()
        );
        assert_eq!(guides(&conn), [Some("h1".into()), None, None, None]);
        assert_eq!(count(&conn, "SELECT count(*) FROM programme"), programmes);
        // Back on at once, with no import.
        sync(
            &mut conn,
            &provider(vec![url("https://epg.example.net/a.xml")]),
            NOW,
        )
        .unwrap();
        assert_eq!(guides(&conn)[3], Some(store::source_guide_id(a, "jun")));
        // Gone from the file: its data goes.
        sync(&mut conn, &provider(vec![]), NOW).unwrap();
        assert_eq!(count(&conn, "SELECT count(*) FROM programme"), 1);
        assert_eq!(count(&conn, "SELECT count(*) FROM source_channel"), 0);
        assert_eq!(count(&conn, "SELECT count(*) FROM source_match"), 0);
    }

    /// Settings › EPG's counts, and the favourites the provider's guide
    /// off would leave with none (EPG sources ticket 04).
    #[test]
    fn settings_counts_matches_and_the_favourites_left_without_a_guide() {
        let (_dir, mut conn) = db();
        let a = url("https://epg.example.net/a.xml");
        with_source(&mut conn, vec![a.clone()]);
        let (channels, stats) = store::source_stats(&conn, "p", NOW).unwrap();
        assert_eq!(channels, 4);
        let s = &stats[&a.ident()];
        assert_eq!((s.matched, s.ambiguous, s.requests), (3, 1, 0));
        // Harbour One's guide is the provider's, first in the order.
        assert_eq!((s.guiding, stats[""].guiding), (2, 1));
        assert_eq!(s.imported, Some(NOW));
        assert_eq!((stats[""].matched, stats[""].imported.is_some()), (1, true));

        // Harbour One's guide is the provider's, Kestrel's the source's.
        for id in [1, 2] {
            store::toggle_favourite(&conn, "p", store::Kind::Live, id).unwrap();
        }
        // Source A has Harbour One too, until it is off.
        assert_eq!(store::only_provider_guide(&conn, "p").unwrap(), 0);
        let off = EpgSource {
            enabled: false,
            ..a
        };
        sync(&mut conn, &provider(vec![off]), NOW).unwrap();
        assert_eq!(store::only_provider_guide(&conn, "p").unwrap(), 1);
    }

    #[test]
    fn the_providers_guide_off_deletes_its_programmes() {
        let (_dir, mut conn) = db();
        let a = with_source(
            &mut conn,
            vec![provider_guide(false), url("https://epg.example.net/a.xml")],
        );
        assert_eq!(store::provider_guide(&conn, "p").unwrap(), (false, 0));
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM programme WHERE epg_channel_id = 'h1'"
            ),
            0
        );
        assert_eq!(guides(&conn)[0], Some(store::source_guide_id(a, "h1")));
    }

    /// The start of the first programme under `guide` in the last day:
    /// the one around NOW, as shifted.
    fn start_now(conn: &Connection, guide: &str) -> Option<i64> {
        use rusqlite::OptionalExtension;
        conn.query_row(
            "SELECT start FROM programme WHERE provider_id = 'p' AND epg_channel_id = ?1
             AND start > ?2 ORDER BY start LIMIT 1",
            rusqlite::params![guide, NOW - 86_400],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
    }

    #[test]
    fn a_pin_overrides_the_order_with_its_shift_added() {
        let (_dir, mut conn) = db();
        let mut source = url("https://epg.example.net/a.xml");
        source.shift_minutes = 10;
        let a = with_source(&mut conn, vec![source.clone()]);
        let own = store::channel_sources(&conn, "p", 1).unwrap()[0].id;
        assert_eq!(guides(&conn)[0].as_deref(), Some("h1"));

        // Harbour One pinned to A, which comes after the provider's guide.
        store::set_pin(&mut conn, "p", 1, Some((a, 0))).unwrap();
        let g = guides(&conn)[0].clone().unwrap();
        assert_eq!(g, store::source_guide_id(a, "h1"));
        assert_eq!(start_now(&conn, &g), Some(NOW - H / 2 + 600));

        // With its own shift, added to the source's, from what is stored.
        store::set_pin(&mut conn, "p", 1, Some((a, 60))).unwrap();
        let g = guides(&conn)[0].clone().unwrap();
        assert!(g.starts_with(&format!("\u{1}{a}\u{1}\u{1}")), "{g:?}");
        assert_eq!(start_now(&conn, &g), Some(NOW - H / 2 + 600 + 3600));
        assert_eq!(
            store::source_ident(&conn, &g).unwrap().as_deref(),
            Some("https://epg.example.net/a.xml")
        );
        // Its copy is made again by a re-import.
        write_source(&mut conn, "p", a, feed().as_bytes(), 600, NOW).unwrap();
        assert_eq!(guides(&conn)[0].as_deref(), Some(g.as_str()));
        assert_eq!(start_now(&conn, &g), Some(NOW - H / 2 + 600 + 3600));

        // Off, the pin is skipped and kept; on again, it is back.
        let mut off = source.clone();
        off.enabled = false;
        sync(&mut conn, &provider(vec![off]), NOW).unwrap();
        assert_eq!(guides(&conn)[0].as_deref(), Some("h1"));
        assert_eq!(count(&conn, "SELECT count(*) FROM source_pin"), 1);
        sync(&mut conn, &provider(vec![source.clone()]), NOW).unwrap();
        assert_eq!(guides(&conn)[0].as_deref(), Some(g.as_str()));

        // Pinned to the provider's guide below A, with a shift: a copy of
        // the provider's programmes, made again by its import.
        let order = vec![source.clone(), provider_guide(true)];
        sync(&mut conn, &provider(order.clone()), NOW).unwrap();
        store::set_pin(&mut conn, "p", 1, Some((own, -30))).unwrap();
        let g = guides(&conn)[0].clone().unwrap();
        assert_eq!(start_now(&conn, &g), Some(NOW - H / 2 - 1800));
        assert_eq!(store::source_ident(&conn, &g).unwrap(), None);
        let guide = doc(&[], &[("h1", NOW - H / 2, NOW + H / 2)]);
        write_epg(&mut conn, "p", guide.as_bytes(), 0, NOW).unwrap();
        assert_eq!(start_now(&conn, &g), Some(NOW - H / 2 - 1800));
        assert_eq!(guides(&conn)[0].as_deref(), Some(g.as_str()));

        // Unpinned, it follows the order: A first now.
        store::set_pin(&mut conn, "p", 1, None).unwrap();
        assert_eq!(guides(&conn)[0], Some(store::source_guide_id(a, "h1")));
        assert_eq!(start_now(&conn, &g), None, "the copy goes with the pin");

        // A pin to a source removed goes with it.
        store::set_pin(&mut conn, "p", 2, Some((a, 5))).unwrap();
        sync(&mut conn, &provider(vec![]), NOW).unwrap();
        assert_eq!(count(&conn, "SELECT count(*) FROM source_pin"), 0);
    }

    #[test]
    fn a_mapping_marks_a_reimport_and_survives_it_and_a_new_id() {
        let (_dir, mut conn) = db();
        let source = url("https://epg.example.net/a.xml");
        let a = with_source(&mut conn, vec![source.clone()]);
        let fetch = |conn: &Connection| store::source_fetch(conn, a, NOW).unwrap();
        let gid = |id: &str| Some(store::source_guide_id(a, id));
        assert!(!fetch(&conn).reimport);
        // Summit is ambiguous: mapped by hand, its guide waits for the
        // re-import, which the source is marked for and due by.
        store::set_mapping(&mut conn, "p", a, 3, Some("summit.b")).unwrap();
        assert_eq!(guides(&conn)[2], None);
        assert!(fetch(&conn).reimport);
        assert_eq!(fetch(&conn).etag, None);
        let planned = sync(&mut conn, &provider(vec![source.clone()]), NOW).unwrap();
        assert!(planned[0].due && planned[0].reimport);
        let sources = store::channel_sources(&conn, "p", 3).unwrap();
        assert_eq!(sources[1].mapped.as_deref(), Some("summit.b"));
        assert_eq!(sources[1].channel_name.as_deref(), Some("Summit HD"));
        assert!(sources[1].reimport && !sources[1].programmes);

        write_source(&mut conn, "p", a, feed().as_bytes(), 0, NOW).unwrap();
        assert_eq!(guides(&conn)[2], gid("summit.b"));
        assert!(!fetch(&conn).reimport);
        // A mapping to a channel stored already shows at once: Juniper to
        // Kestrel's.
        store::set_mapping(&mut conn, "p", a, 4, Some("kestrel.ext")).unwrap();
        assert_eq!(guides(&conn)[3], gid("kestrel.ext"));
        store::set_mapping(&mut conn, "p", a, 4, None).unwrap();
        write_source(&mut conn, "p", a, feed().as_bytes(), 0, NOW).unwrap();
        assert_eq!(guides(&conn)[3], gid("jun"));

        // The provider renumbers Summit: the mapping follows its name.
        import_channels(
            &mut conn,
            &[
                (1, "UK: Harbour One", Some("h1"), 3),
                (2, "UK: Kestrel", Some("k1"), 0),
                (33, "UK: Summit", None, 0),
                (4, "UK: Juniper", None, 0),
            ],
        );
        write_source(&mut conn, "p", a, feed().as_bytes(), 0, NOW).unwrap();
        assert_eq!(guides(&conn)[3], gid("summit.b"), "stream 33, last by id");
        assert_eq!(count(&conn, "SELECT stream_id FROM source_mapping"), 33);

        // Removed, its mappings stay for a source with the same identity;
        // forgotten, they go.
        sync(&mut conn, &provider(vec![]), NOW).unwrap();
        assert_eq!(count(&conn, "SELECT count(*) FROM source_mapping"), 1);
        let renewed = url("https://epg.example.net/a.xml?token=2");
        let b = with_source(&mut conn, vec![renewed]);
        assert_eq!(
            guides(&conn)[3],
            Some(store::source_guide_id(b, "summit.b"))
        );
        let ident = source.ident();
        assert_eq!(store::forget_mappings(&conn, "p", &ident).unwrap(), 1);
    }

    #[test]
    fn the_picker_finds_a_sources_channels_by_id_and_names() {
        let (_dir, mut conn) = db();
        let a = with_source(&mut conn, vec![url("https://epg.example.net/a.xml")]);
        let ids = |q: &str| -> Vec<String> {
            store::find_source_channels(&conn, a, q, 10)
                .unwrap()
                .into_iter()
                .map(|c| c.id)
                .collect()
        };
        assert_eq!(ids("summit"), ["summit.a", "summit.b"]);
        assert_eq!(ids("SUMMIT hd"), ["summit.b"]);
        assert_eq!(ids("kestrel.e"), ["kestrel.ext"]);
        assert_eq!(ids("50%"), Vec::<String>::new());
        assert_eq!(ids("").len(), 5);
        assert_eq!(
            store::ambiguous_channels(&conn, "p", a).unwrap(),
            [(3, "UK: Summit".to_owned())]
        );
    }

    #[test]
    fn epg_norm_drops_prefix_brackets_and_punctuation() {
        assert_eq!(epg_norm("UK| Harbour One HD"), "harbour one");
        assert_eq!(epg_norm("|UK| Harbour One"), "harbour one");
        assert_eq!(epg_norm("[UK] Harbour One (Backup)"), "harbour one");
        assert_eq!(epg_norm("US: Summit-News FHD"), "summit news");
        assert_eq!(epg_norm("Harbour One +1"), "harbour one +1");
        assert_eq!(epg_norm("DE: KANAL VIER ᴴᴰ"), "kanal vier");
        // Not a tag: too long, or nothing after it.
        assert_eq!(epg_norm("Northwind: Arena"), "northwind arena");
        assert_eq!(epg_norm("UK:"), "uk");
    }

    fn channel(id: &str, names: &[&str]) -> FeedChannel {
        FeedChannel {
            id: id.into(),
            names: names.iter().map(|n| n.to_string()).collect(),
            icon: None,
        }
    }

    #[test]
    fn matching_takes_exactly_one() {
        let feed = [
            channel("harbour.one", &["Harbour One", "Harbour 1"]),
            channel("summit", &["Summit"]),
            channel("summit.west", &["Summit"]),
            channel("rivage", &["Rivage"]),
            channel("rivage", &["Rivage Info"]),
        ];
        let ix = Index::new(&feed, &[epg_norm]);
        assert_eq!(ix.find(Some("summit"), "anything"), Match::Id(1));
        assert_eq!(ix.find(Some("rivage"), "Rivage"), Match::Ambiguous);
        assert_eq!(
            ix.find(Some("unknown"), "UK| HARBOUR ONE HD"),
            Match::Name(0)
        );
        assert_eq!(ix.find(None, "UK: Summit"), Match::Ambiguous);
        assert_eq!(ix.find(None, "Cobalt"), Match::None);
        let plain = Index::new(&feed, &[name_norm]);
        assert_eq!(plain.find(None, "UK| HARBOUR ONE HD"), Match::None);
        // The finer rule first: unique there, though the second would merge.
        let feed = [
            channel("a", &["Summit (East)"]),
            channel("b", &["Summit (West)"]),
        ];
        let both = Index::new(&feed, &[name_norm, epg_norm]);
        assert_eq!(both.find(None, "Summit (West)"), Match::Name(1));
        assert_eq!(both.find(None, "UK| Summit"), Match::Ambiguous);
    }

    /// A source fetched at each of `requests`, the last 24 h's.
    fn fetched(requests: &[i64]) -> SourceFetch {
        SourceFetch {
            fetched: requests.last().copied(),
            requests: requests.to_vec(),
            ..SourceFetch::default()
        }
    }

    #[test]
    fn interval_and_cap_keep_two_for_r() {
        let mut a = url("https://epg.example.net/a.xml");
        a.every_hours = Some(2);
        a.max_per_day = Some(5);
        assert_eq!(plan(&a, &fetched(&[]), None, false, NOW), Plan::Fetch);
        // An hour since the last: waits, unless `r`.
        let f = fetched(&[NOW - H]);
        assert_eq!(plan(&a, &f, None, false, NOW), Plan::Wait);
        assert_eq!(plan(&a, &f, None, true, NOW), Plan::Fetch);
        // Three in 24 h: scheduled fetches stop 2 short of 5, until the
        // oldest is a day old; `r` may use the 2.
        let three = fetched(&[NOW - 23 * H, NOW - 10 * H, NOW - 3 * H]);
        let at_cap = |count, next| {
            Plan::Skip(Outcome::AtCap {
                count,
                max: 5,
                next,
            })
        };
        assert_eq!(plan(&a, &three, None, false, NOW), at_cap(3, NOW + H));
        assert_eq!(plan(&a, &three, None, true, NOW), Plan::Fetch);
        // Five: `r` skips too.
        let five = fetched(&[
            NOW - 23 * H,
            NOW - 20 * H,
            NOW - 10 * H,
            NOW - 3 * H,
            NOW - H,
        ]);
        assert_eq!(plan(&a, &five, None, true, NOW), at_cap(5, NOW + H));
        assert_eq!(
            plan(&a, &five, None, false, NOW + H),
            at_cap(5, NOW + 14 * H)
        );
        // No cap: the interval alone.
        a.max_per_day = None;
        assert_eq!(plan(&a, &five, None, false, NOW + H), Plan::Fetch);
    }

    #[test]
    fn a_path_is_read_when_its_mtime_changes() {
        let mut p = url("");
        (p.url, p.path) = (None, Some("/srv/epg/guide.xml".into()));
        let f = SourceFetch {
            mtime: Some(5),
            ..SourceFetch::default()
        };
        assert_eq!(plan(&p, &f, Some(5), false, NOW), Plan::Wait);
        assert_eq!(plan(&p, &f, Some(6), false, NOW), Plan::Fetch);
        assert_eq!(plan(&p, &f, None, false, NOW), Plan::Fetch, "gone");
        assert_eq!(plan(&p, &f, Some(5), true, NOW), Plan::Fetch);
    }

    #[test]
    fn retry_after_takes_seconds_or_a_date() {
        assert_eq!(retry_after(" 120 ", NOW), Some(NOW + 120));
        assert_eq!(
            retry_after("Sun, 06 Nov 1994 08:49:37 GMT", NOW),
            Some(784_111_777)
        );
        assert_eq!(retry_after("soon", NOW), None);
    }

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            status,
            body: body.as_bytes().to_vec(),
            delay: Duration::ZERO,
        }
    }

    /// A 304 imports nothing, the count of requests outlives a restart,
    /// `r` uses the 2 kept and then skips, and a new shift asks without
    /// the validators.
    #[tokio::test]
    async fn conditional_fetches_count_and_survive_a_restart() {
        let now = unix_now();
        let body = doc(
            &[("h1", "Harbour One")],
            &[("h1", now - H / 2, now + H / 2)],
        );
        let fake = FakeXtream::start_head(move |head| {
            if head.to_ascii_lowercase().contains("if-none-match: \"v1\"") {
                (reply(304, ""), String::new())
            } else {
                (reply(200, &body), "ETag: \"v1\"\r\n".into())
            }
        })
        .await;
        let (dir, mut conn) = db();
        let mut a = url(&format!("{}/guide.xml?token=tok-9x", fake.url()));
        a.max_per_day = Some(4);
        let planned = sync(&mut conn, &provider(vec![a.clone()]), now).unwrap();
        assert!(planned[0].due, "new");
        let writer: Writer = Arc::new(Mutex::new(conn));
        let meter = Meter::default();
        let fetch = |force| import(&writer, "p", &planned[0], force, &meter);
        assert_eq!(fetch(false).await, Some(Outcome::Fetched(1)));
        // Not due for 12 h; `r` asks with the ETag.
        assert_eq!(fetch(false).await, None);
        assert_eq!(fetch(true).await, Some(Outcome::Unchanged));
        let guide = "SELECT count(*) FROM programme WHERE epg_channel_id != 'h1'";
        assert_eq!(count(&writer.lock().unwrap(), guide), 1);
        drop(writer);

        let mut conn = store::open(&dir.path().join("v.db")).unwrap();
        let planned = sync(&mut conn, &provider(vec![a.clone()]), now).unwrap();
        assert!(!planned[0].due);
        let writer: Writer = Arc::new(Mutex::new(conn));
        assert_eq!(
            import(&writer, "p", &planned[0], true, &Meter::default()).await,
            Some(Outcome::Unchanged)
        );
        a.shift_minutes = 30;
        let planned = sync(&mut writer.lock().unwrap(), &provider(vec![a]), now).unwrap();
        assert_eq!(
            import(&writer, "p", &planned[0], true, &Meter::default()).await,
            Some(Outcome::Fetched(1))
        );
        let Some(Outcome::AtCap {
            count: 4,
            max: 4,
            next,
        }) = import(&writer, "p", &planned[0], true, &Meter::default()).await
        else {
            panic!("asked past the cap");
        };
        assert!((now + 86_400..now + 86_410).contains(&next), "{next}");
        assert_eq!(fake.targets().len(), 4);
        let ident: String = writer
            .lock()
            .unwrap()
            .query_row("SELECT group_concat(ident) FROM epg_source", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(!ident.contains("tok-9x"), "{ident}");
    }

    #[tokio::test]
    async fn retry_after_backs_the_source_off() {
        let fake =
            FakeXtream::start_head(|_| (reply(429, ""), "Retry-After: 120\r\n".into())).await;
        let (_dir, mut conn) = db();
        let now = unix_now();
        let p = provider(vec![url(&format!("{}/guide.xml", fake.url()))]);
        let planned = sync(&mut conn, &p, now).unwrap();
        let writer: Writer = Arc::new(Mutex::new(conn));
        let Some(Outcome::Backoff { retry_at }) =
            import(&writer, "p", &planned[0], false, &Meter::default()).await
        else {
            panic!("no backoff");
        };
        assert!((now + 120..now + 130).contains(&retry_at), "{retry_at}");
        // Not asked before then, `r` or not.
        assert_eq!(
            import(&writer, "p", &planned[0], true, &Meter::default()).await,
            Some(Outcome::Backoff { retry_at })
        );
        assert_eq!(fake.targets().len(), 1);
        let due = |at| sync(&mut writer.lock().unwrap(), &p, at).unwrap()[0].due;
        assert!(!due(now));
        // Then due at once, not in 12 h.
        assert!(due(retry_at));
    }
}
