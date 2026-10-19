//! The guide's older days (catch-up ticket 06): `xmltv.php` reaches about a
//! day back (SPEC 6.1), so when the guide grid shows time before that on a
//! channel with an archive, its `get_simple_data_table` is fetched and
//! stored before what the guide has. Only for rows on screen, the selected
//! one first, each channel at most once per EPG refresh, behind any
//! refresh request (SPEC 6.2).

use crate::import::{self, Writer};
use crate::store::{self, TableProgramme};
use crate::xtream::{self, Client, Clients};
use base64::Engine;
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;
use tokio::sync::mpsc;

/// A grid row asking for its channel's table.
#[derive(Clone, Debug)]
pub struct Ask {
    pub provider_id: String,
    /// The row's channel key, sent back once the table is in.
    pub key: i64,
    pub stream_id: i64,
    pub guide_id: String,
    pub archive_days: i64,
    /// How far the server's clock runs ahead of UTC: the table's times are
    /// in it.
    pub offset: i64,
    /// The selected row: fetched before the rest.
    pub first: bool,
}

/// A table is in: the row with `key` reads its guide again.
#[derive(Debug, PartialEq)]
pub struct Filled {
    pub provider_id: String,
    pub key: i64,
}

/// How long asks gather before the first goes out: the rows on screen ask
/// within a frame, the selected one not necessarily first.
const GATHER: Duration = Duration::from_millis(100);

/// Fetches tables as rows ask for them, until `asks` closes. `clients` by
/// provider id.
pub async fn run(
    clients: Clients,
    writer: Writer,
    mut asks: mpsc::UnboundedReceiver<Ask>,
    filled: mpsc::UnboundedSender<Filled>,
) {
    let mut queue = VecDeque::new();
    // Per provider: the EPG refresh the fetched channels belong to, and
    // those channels' stream ids.
    let mut fetched: HashMap<String, (Option<i64>, HashSet<i64>)> = HashMap::new();
    loop {
        if queue.is_empty() {
            let Some(ask) = asks.recv().await else {
                return;
            };
            push(&mut queue, ask);
            tokio::time::sleep(GATHER).await;
        }
        while let Ok(ask) = asks.try_recv() {
            push(&mut queue, ask);
        }
        let Some(ask) = queue.pop_front() else {
            continue;
        };
        let Some(client) = clients.get(&ask.provider_id) else {
            continue;
        };
        let id = ask.provider_id.clone();
        let read = on_writer(&writer, move |c| {
            Ok::<_, rusqlite::Error>((
                store::last_refreshes(c, &id)?,
                store::provider_guide(c, &id)?,
            ))
        })
        .await;
        let (epg_at, shift) = match read {
            Ok(((_, at), (true, shift))) => (at, shift * 60),
            // The provider's guide is off, tables with it (EPG sources ticket 02).
            Ok((_, (false, _))) => continue,
            Err(e) => {
                log::error!("reading provider: {e}");
                continue;
            }
        };
        let (at, done) = fetched.entry(ask.provider_id.clone()).or_default();
        if *at != epg_at {
            *at = epg_at;
            done.clear();
        }
        if !done.insert(ask.stream_id) {
            continue;
        }
        match fetch(&client, &writer, &ask, shift).await {
            Ok(added) => {
                log::debug!(
                    "guide table for {}: {added} older programmes",
                    ask.stream_id
                );
                let _ = filled.send(Filled {
                    provider_id: ask.provider_id,
                    key: ask.key,
                });
            }
            Err(e) => log::warn!("guide table: {}", xtream::redact(&e)),
        }
    }
}

/// Queues `ask` once per channel; the selected row's goes to the front.
/// Asks for another provider are dropped, as its grid is gone.
fn push(queue: &mut VecDeque<Ask>, ask: Ask) {
    queue.retain(|q| q.provider_id == ask.provider_id);
    let queued = queue.iter().position(|q| q.stream_id == ask.stream_id);
    match (queued, ask.first) {
        (Some(i), true) => {
            queue.remove(i);
            queue.push_front(ask);
        }
        (Some(_), false) => {}
        (None, true) => queue.push_front(ask),
        (None, false) => queue.push_back(ask),
    }
}

/// Fetches and stores one channel's table, its times moved by `shift`
/// seconds as the provider's guide's are. Returns how many programmes it
/// added.
async fn fetch(client: &Client, writer: &Writer, ask: &Ask, shift: i64) -> Result<usize, String> {
    let body = client.guide_table(ask.stream_id).await?;
    let mut progs = parse(&body)?;
    for p in &mut progs {
        p.start += shift - ask.offset;
        p.stop += shift - ask.offset;
    }
    let (id, guide, days) = (
        ask.provider_id.clone(),
        ask.guide_id.clone(),
        ask.archive_days,
    );
    on_writer(writer, move |c| {
        store::add_table(c, &id, &guide, days, &progs, import::unix_now())
    })
    .await
    .map_err(|e| format!("saving guide table: {e}"))
}

/// Runs `f` on the writer connection, off the async threads.
async fn on_writer<T: Send + 'static>(
    writer: &Writer,
    f: impl FnOnce(&mut Connection) -> T + Send + 'static,
) -> T {
    let writer = writer.clone();
    tokio::task::spawn_blocking(move || f(&mut writer.lock().unwrap_or_else(|e| e.into_inner())))
        .await
        .expect("writer task")
}

/// One of a table's `epg_listings` (SPEC 6.1).
#[derive(Deserialize)]
struct Listing {
    #[serde(default, deserialize_with = "xtream::lenient_int")]
    start_timestamp: Option<i64>,
    #[serde(default, deserialize_with = "xtream::lenient_int")]
    stop_timestamp: Option<i64>,
    #[serde(default, deserialize_with = "xtream::lenient_string")]
    title: Option<String>,
    #[serde(default, deserialize_with = "xtream::lenient_string")]
    description: Option<String>,
    #[serde(default, deserialize_with = "xtream::lenient_bool")]
    has_archive: Option<bool>,
}

/// A table's programmes, timed as sent: the server's clock read as UTC
/// (SPEC 6.1). Titles and descriptions come base64-encoded; a listing
/// without times or a title is skipped.
pub fn parse(body: &[u8]) -> Result<Vec<TableProgramme>, String> {
    // Position only: serde's message can quote values from the body.
    let table: serde_json::Value = serde_json::from_slice(body).map_err(|e| {
        format!(
            "unexpected get_simple_data_table response at line {} column {}",
            e.line(),
            e.column()
        )
    })?;
    let Some(list) = table.get("epg_listings").and_then(|l| l.as_array()) else {
        return Ok(Vec::new());
    };
    Ok(list
        .iter()
        .filter_map(|l| {
            let l = Listing::deserialize(l).ok()?;
            Some(TableProgramme {
                start: l.start_timestamp?,
                stop: l.stop_timestamp?,
                title: l
                    .title
                    .as_deref()
                    .and_then(text)
                    .filter(|t| !t.is_empty())?,
                description: l.description.as_deref().and_then(text),
                archive: l.has_archive,
            })
        })
        .collect())
}

/// Base64 text, decoded.
fn text(b64: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_xtream::{self, FakeXtream};
    use crate::xtream::Limits;
    use std::sync::Arc;
    use std::sync::Mutex;

    const FAST: Limits = Limits {
        min_gap: Duration::from_millis(10),
        backoff_start: Duration::from_millis(200),
        backoff_max: Duration::from_millis(800),
    };

    #[test]
    fn parses_the_providers_table() {
        let now = 1_700_000_000;
        let progs = parse(fake_xtream::table(now).as_bytes()).unwrap();
        assert_eq!(progs.len(), 4 * 24 + 2);
        let p = &progs[0];
        assert_eq!(p.title, "Bulletin 0");
        assert_eq!(p.description.as_deref(), Some("The day's news."));
        assert_eq!(p.stop - p.start, 3600);
        assert_eq!(p.archive, Some(false), "4 days back, past the archive");
        assert_eq!(progs.last().unwrap().archive, Some(true));

        // Numbers for strings, no title, no times, and shapes that are not
        // a table.
        let body = br#"{"epg_listings":[
            {"title":"QQ==","start_timestamp":100,"stop_timestamp":200},
            {"title":"","start_timestamp":100,"stop_timestamp":200},
            {"title":"QQ==","start_timestamp":"x","stop_timestamp":200},
            "nonsense"]}"#;
        let progs = parse(body).unwrap();
        assert_eq!(progs.len(), 1);
        assert_eq!((progs[0].title.as_str(), progs[0].archive), ("A", None));
        assert!(parse(b"[]").unwrap().is_empty());
        assert!(parse(br#"{"epg_listings":false}"#).unwrap().is_empty());
        assert!(parse(b"<html>").is_err());
    }

    fn ask(stream_id: i64, first: bool) -> Ask {
        Ask {
            provider_id: "p".into(),
            key: stream_id,
            stream_id,
            guide_id: "news1".into(),
            archive_days: 3,
            offset: fake_xtream::SERVER_OFFSET,
            first,
        }
    }

    #[test]
    fn selected_row_goes_first_and_each_channel_once() {
        let mut queue = VecDeque::new();
        for a in [
            ask(1, false),
            ask(2, false),
            ask(1, false),
            ask(3, true),
            ask(2, true),
        ] {
            push(&mut queue, a);
        }
        let order: Vec<i64> = queue.iter().map(|a| a.stream_id).collect();
        assert_eq!(order, [2, 3, 1]);
        // Another provider's grid replaced this one's.
        push(
            &mut queue,
            Ask {
                provider_id: "q".into(),
                ..ask(4, false)
            },
        );
        assert_eq!(queue.len(), 1);
    }

    /// The stream ids of the tables asked of `fake`, in order.
    fn fetched(fake: &FakeXtream) -> Vec<i64> {
        fake.targets()
            .iter()
            .filter(|t| fake_xtream::action(t) == "get_simple_data_table")
            .filter_map(|t| t.split("stream_id=").nth(1)?.parse().ok())
            .collect()
    }

    #[tokio::test]
    async fn fetches_the_selected_row_first_and_each_once_per_refresh() {
        // The guide reaches a day back on `news1`, as `xmltv.php` does.
        let fake = FakeXtream::start(|target| match fake_xtream::action(target) {
            "xmltv" => {
                let now = import::unix_now();
                let (a, b) = (now - 86_400, now + 86_400);
                let stamp = |t| fake_xtream::xmltv_stamp(t, 0);
                let body = format!(
                    r#"<tv><programme start="{}" stop="{}" channel="news1"><title>Rolling news</title></programme></tv>"#,
                    stamp(a),
                    stamp(b)
                );
                (200, body)
            }
            _ => fake_xtream::active(target),
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let writer: Writer = Arc::new(Mutex::new(store::open(&dir.path().join("v.db")).unwrap()));
        let client = Arc::new(Client::with_limits(fake.provider(), FAST));
        let provider = fake.provider();
        import::channels(&client, &writer, &provider, "ts")
            .await
            .unwrap();
        import::epg(&client, &writer, &provider, 0).await.unwrap();

        let (asks, rx) = mpsc::unbounded_channel();
        let (filled_tx, mut filled) = mpsc::unbounded_channel();
        let clients = [("p".to_string(), client)].into_iter().collect();
        tokio::spawn(run(clients, writer.clone(), rx, filled_tx));
        let mut wait = async |n: usize| {
            let mut keys = Vec::new();
            for _ in 0..n {
                keys.push(filled.recv().await.unwrap().key);
            }
            keys
        };

        // Rows on screen ask in order; the selected one is the third.
        for a in [ask(10, false), ask(11, false), ask(12, true)] {
            asks.send(a).unwrap();
        }
        assert_eq!(wait(3).await, [12, 10, 11]);
        assert_eq!(fetched(&fake), [12, 10, 11]);

        // The older days are stored before the guide's, within 3 days.
        let now = import::unix_now();
        let (count, oldest, flagged, inside): (i64, i64, i64, i64) = writer
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*), min(stop), sum(archive), sum(start >= ?1) FROM programme
                 WHERE epg_channel_id = 'news1' AND title LIKE 'Bulletin %'",
                [now - 3 * 86_400],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert!((47..=48).contains(&count), "{count}");
        assert!(oldest > now - 3 * 86_400, "{oldest}");
        // One starting before the archive is in the table without it.
        assert_eq!(flagged, inside);
        // At their own times, not the server clock's (SPEC 6.1).
        let (title, start): (String, i64) = writer
            .lock()
            .unwrap()
            .query_row(
                "SELECT title, start FROM programme WHERE title LIKE 'Bulletin %' LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let k: i64 = title["Bulletin ".len()..].parse().unwrap();
        assert_eq!(start, now / 3600 * 3600 - 4 * 86_400 + k * 3600);

        // Once per refresh: 10 again is not fetched.
        asks.send(ask(10, false)).unwrap();
        asks.send(ask(13, false)).unwrap();
        assert_eq!(wait(1).await, [13]);
        assert_eq!(fetched(&fake), [12, 10, 11, 13]);

        // A new EPG refresh, and it is.
        writer
            .lock()
            .unwrap()
            .execute(
                "UPDATE provider SET last_epg_refresh = last_epg_refresh + 1",
                [],
            )
            .unwrap();
        asks.send(ask(10, false)).unwrap();
        assert_eq!(wait(1).await, [10]);
        assert_eq!(fetched(&fake), [12, 10, 11, 13, 10]);
    }
}
