//! A movie's or series' info on demand (VOD tickets 04 and 05): the movie
//! list has no year, length, genre or plot, and the series list no
//! episodes (SPEC 6.1), so `get_vod_info` or `get_series_info` is fetched
//! when one is selected and stored, until the next import of its list
//! replaces it. The latest selection first, each title at most once per
//! list refresh, behind any refresh request (SPEC 6.2).

use crate::import::{self, Writer};
use crate::store::{self, Episode, Kind, MovieInfo, SeriesInfo};
use crate::xtream::{self, Client, Clients};
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use tokio::sync::mpsc;

/// A selected movie or series whose info is not known.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub provider_id: String,
    pub kind: Kind,
    /// A movie's stream id or a series' id.
    pub id: i64,
}

/// Its info is in (`ok`), or could not be had this time: the row and the
/// detail panel read it again.
#[derive(Clone, Debug, PartialEq)]
pub struct Filled {
    pub ask: Ask,
    pub ok: bool,
}

/// How many selections wait at most. Holding `j` selects a title a frame;
/// those passed over are dropped rather than fetched (SPEC 6.2).
const QUEUED: usize = 3;

/// Fetches info as titles are selected, until `asks` closes. `clients` by
/// provider id.
pub async fn run(
    clients: Clients,
    writer: Writer,
    mut asks: mpsc::UnboundedReceiver<Ask>,
    filled: mpsc::UnboundedSender<Filled>,
) {
    let mut queue = VecDeque::new();
    // Per provider and kind: the list refresh the fetched ones belong to,
    // and their ids.
    let mut fetched: HashMap<(String, Kind), (Option<i64>, HashSet<i64>)> = HashMap::new();
    loop {
        if queue.is_empty() {
            let Some(ask) = asks.recv().await else {
                return;
            };
            push(&mut queue, ask);
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
        let list_at = on_writer(&writer, move |c| store::last_vod_refreshes(c, &id)).await;
        let list_at = match (list_at, ask.kind) {
            (Ok((movies, _)), Kind::Movie) => movies,
            (Ok((_, series)), _) => series,
            (Err(e), _) => {
                log::error!("reading provider: {e}");
                continue;
            }
        };
        let key = (ask.provider_id.clone(), ask.kind);
        let (at, done) = fetched.entry(key.clone()).or_default();
        if *at != list_at {
            *at = list_at;
            done.clear();
        }
        if !done.insert(ask.id) {
            continue;
        }
        match fetch(&client, &writer, &ask).await {
            Ok(()) => {
                let _ = filled.send(Filled { ask, ok: true });
            }
            Err(e) => {
                log::warn!("{} info: {}", ask.kind.as_str(), xtream::redact(&e));
                // Selecting it again tries again.
                if let Some((_, done)) = fetched.get_mut(&key) {
                    done.remove(&ask.id);
                }
                let _ = filled.send(Filled { ask, ok: false });
            }
        }
    }
}

/// Puts `ask` first, once, keeping at most [`QUEUED`]. Asks for another
/// provider are dropped, as its list is gone.
fn push(queue: &mut VecDeque<Ask>, ask: Ask) {
    queue.retain(|q| q.provider_id == ask.provider_id && (q.kind, q.id) != (ask.kind, ask.id));
    queue.push_front(ask);
    queue.truncate(QUEUED);
}

/// Fetches and stores one movie's or series' info.
async fn fetch(client: &Client, writer: &Writer, ask: &Ask) -> Result<(), String> {
    let (provider_id, id) = (ask.provider_id.clone(), ask.id);
    let saved = if ask.kind == Kind::Series {
        let info = parse_series(&client.series_info(id).await?)?;
        on_writer(writer, move |c| {
            store::set_series_info(c, &provider_id, id, &info, import::unix_now())
        })
        .await
    } else {
        let info = parse(&client.vod_info(id).await?)?;
        on_writer(writer, move |c| {
            store::set_movie_info(c, &provider_id, id, &info, import::unix_now())
        })
        .await
    };
    saved.map_err(|e| format!("saving {} info: {e}", ask.kind.as_str()))
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

/// `get_vod_info`'s `info` (SPEC 6.1). `plot` and `description`, `cast`
/// and `actors` come in pairs holding the same; either may be missing.
#[derive(Default, Deserialize)]
struct Info {
    #[serde(default, deserialize_with = "xtream::lenient_string")]
    releasedate: Option<String>,
    #[serde(default, deserialize_with = "xtream::lenient_string")]
    duration: Option<String>,
    #[serde(default, deserialize_with = "xtream::lenient_string")]
    genre: Option<String>,
    #[serde(default, deserialize_with = "xtream::lenient_string")]
    plot: Option<String>,
    #[serde(default, deserialize_with = "xtream::lenient_string")]
    description: Option<String>,
    #[serde(default, deserialize_with = "xtream::lenient_string")]
    cast: Option<String>,
    #[serde(default, deserialize_with = "xtream::lenient_string")]
    actors: Option<String>,
    #[serde(default, deserialize_with = "xtream::lenient_string")]
    director: Option<String>,
}

/// A movie's info. The length is `duration` (`HH:MM:SS`), never
/// `duration_secs`, which is unreliable (SPEC 6.1). An `info` that is not
/// an object (panels send `[]`) is a movie with none.
pub fn parse(body: &[u8]) -> Result<MovieInfo, String> {
    // Position only: serde's message can quote values from the body.
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| {
        format!(
            "unexpected get_vod_info response at line {} column {}",
            e.line(),
            e.column()
        )
    })?;
    let info = match v.get("info") {
        Some(o @ serde_json::Value::Object(_)) => Info::deserialize(o).unwrap_or_default(),
        _ => Info::default(),
    };
    let text = |a: Option<String>, b: Option<String>| {
        a.into_iter()
            .chain(b)
            .map(|s| s.trim().to_string())
            .find(|s| !s.is_empty())
    };
    Ok(MovieInfo {
        year: info
            .releasedate
            .as_deref()
            .and_then(|d| d.trim().get(..4)?.parse().ok())
            .filter(|y| *y > 0),
        length: info.duration.as_deref().and_then(hms).filter(|s| *s > 0),
        genre: text(info.genre, None),
        plot: text(info.plot, info.description),
        actors: text(info.cast, info.actors),
        director: text(info.director, None),
    })
}

/// An episode in `get_series_info`'s `episodes` (SPEC 6.1).
#[derive(Deserialize)]
struct RawEpisode {
    #[serde(default, deserialize_with = "xtream::lenient_int")]
    id: Option<i64>,
    #[serde(default, deserialize_with = "xtream::lenient_int")]
    episode_num: Option<i64>,
    #[serde(default, deserialize_with = "xtream::lenient_string")]
    title: Option<String>,
    #[serde(default, deserialize_with = "xtream::lenient_string")]
    container_extension: Option<String>,
}

/// An episode's own `info`; `[]` on some.
#[derive(Default, Deserialize)]
struct EpisodeInfo {
    #[serde(default, deserialize_with = "xtream::lenient_string")]
    duration: Option<String>,
    #[serde(default, deserialize_with = "xtream::lenient_string")]
    air_date: Option<String>,
}

/// A series' episodes, from `episodes`: an object keyed by season number,
/// each a list. Its seasons are those keys, never `seasons`, which lists
/// the show's seasons, not the provider's (SPEC 6.1). An episode's stream
/// id is its `id`, not its info's. One without an `id` cannot play and is
/// left out; anything else that is not the shape is a series with none.
pub fn parse_series(body: &[u8]) -> Result<SeriesInfo, String> {
    // Position only, as `parse`.
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| {
        format!(
            "unexpected get_series_info response at line {} column {}",
            e.line(),
            e.column()
        )
    })?;
    let mut info = SeriesInfo::default();
    let Some(seasons) = v.get("episodes").and_then(|e| e.as_object()) else {
        return Ok(info);
    };
    for (key, list) in seasons {
        let Ok(season) = key.trim().parse::<i64>() else {
            continue;
        };
        for e in list.as_array().into_iter().flatten() {
            let Ok(raw) = RawEpisode::deserialize(e) else {
                continue;
            };
            let Some(stream_id) = raw.id else {
                continue;
            };
            let more = match e.get("info") {
                Some(o @ serde_json::Value::Object(_)) => {
                    EpisodeInfo::deserialize(o).unwrap_or_default()
                }
                _ => EpisodeInfo::default(),
            };
            let year = more
                .air_date
                .as_deref()
                .and_then(|d| d.trim().get(..4)?.parse::<i64>().ok())
                .filter(|y| *y > 0);
            info.last_year = info.last_year.max(year);
            let title = raw.title.map(|t| t.trim().to_string()).unwrap_or_default();
            info.episodes.push(Episode {
                season,
                num: raw.episode_num,
                stream_id,
                title,
                extension: raw.container_extension.map(|x| x.trim().to_string()),
                length: more.duration.as_deref().and_then(hms).filter(|s| *s > 0),
            });
        }
    }
    Ok(info)
}

/// `01:33:14` in seconds.
fn hms(s: &str) -> Option<i64> {
    let parts: Vec<i64> = s
        .trim()
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    match parts[..] {
        [h, m, s] if (0..60).contains(&m) && (0..60).contains(&s) && h >= 0 => {
            Some(h * 3600 + m * 60 + s)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_xtream::{self, FakeXtream};
    use crate::xtream::Limits;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;

    const FAST: Limits = Limits {
        min_gap: Duration::from_millis(10),
        backoff_start: Duration::from_millis(200),
        backoff_max: Duration::from_millis(800),
    };

    #[test]
    fn parses_the_providers_info() {
        let info = parse(fake_xtream::vod_info(20).as_bytes()).unwrap();
        assert_eq!(
            info,
            MovieInfo {
                year: Some(2019),
                length: Some(5594),
                genre: Some("Drama".into()),
                plot: Some("A lighthouse keeper counts the ships that never come.".into()),
                actors: Some("Ada Marsh, Tom Reyes".into()),
                director: Some("Iris Vale".into()),
            }
        );

        // Numbers for strings, the pairs' other half, empty and missing
        // fields, `duration_secs` ignored.
        let body = br#"{"info":{"releasedate":"","duration":"00:00:00","duration_secs":93,
            "genre":7,"plot":" ","description":"Told twice.","actors":"Mia Stone"},
            "movie_data":{"stream_id":"20"}}"#;
        let info = parse(body).unwrap();
        assert_eq!(
            info,
            MovieInfo {
                genre: Some("7".into()),
                plot: Some("Told twice.".into()),
                actors: Some("Mia Stone".into()),
                ..Default::default()
            }
        );
        let lengths = ["1:05:00", "100:00:01", "01:60:00", "93", "x"].map(hms);
        assert_eq!(lengths, [Some(3900), Some(360_001), None, None, None]);

        // No info at all, and shapes that are not info.
        assert_eq!(
            parse(br#"{"info":[],"movie_data":{}}"#).unwrap(),
            MovieInfo::default()
        );
        assert_eq!(
            parse(br#"{"info":{"duration":[1]}}"#).unwrap(),
            MovieInfo::default()
        );
        assert_eq!(parse(b"[]").unwrap(), MovieInfo::default());
        assert!(parse(b"<html>").is_err());
    }

    #[test]
    fn parses_a_series_episodes_by_the_episodes_keys() {
        let info = parse_series(fake_xtream::series_info().as_bytes()).unwrap();
        // `seasons` lists 2; the provider has only the first.
        let seasons: HashSet<i64> = info.episodes.iter().map(|e| e.season).collect();
        assert_eq!(seasons, HashSet::from([1]));
        assert_eq!(info.last_year, Some(2021));
        assert_eq!(
            info.episodes[0],
            Episode {
                season: 1,
                num: Some(2),
                stream_id: 31,
                title: "Harbour Watch - S01E02 - Low Water".into(),
                extension: Some("mp4".into()),
                length: Some(2712),
            }
        );
        assert_eq!(
            (info.episodes[1].num, info.episodes[1].stream_id),
            (Some(1), 30)
        );

        // No `id`: left out. `info` as `[]`, a season key that is not a
        // number, and numbers for strings.
        let body = br#"{"seasons":[],"episodes":{
            "2":[{"episode_num":1,"title":"No id"},
                 {"id":41,"episode_num":"x","title":7,"info":[]}],
            "Specials":[{"id":42,"episode_num":1,"title":"Extra"}]}}"#;
        let info = parse_series(body).unwrap();
        assert_eq!(
            info.episodes,
            [Episode {
                season: 2,
                stream_id: 41,
                title: "7".into(),
                ..Default::default()
            }]
        );
        assert_eq!(info.last_year, None);

        // A series with none, as panels send it.
        for body in [
            &br#"{"seasons":[],"info":[],"episodes":[]}"#[..],
            b"[]",
            b"{}",
        ] {
            assert_eq!(parse_series(body).unwrap(), SeriesInfo::default());
        }
        assert!(parse_series(b"<html>").is_err());
    }

    #[tokio::test]
    async fn fetches_a_series_episodes_and_stores_them() {
        let fake = FakeXtream::start(fake_xtream::active).await;
        let dir = tempfile::tempdir().unwrap();
        let writer: Writer = Arc::new(Mutex::new(store::open(&dir.path().join("v.db")).unwrap()));
        let client = Arc::new(Client::with_limits(fake.provider(), FAST));
        import::series(&client, &writer, &fake.provider())
            .await
            .unwrap();

        let (asks, rx) = mpsc::unbounded_channel();
        let (filled_tx, mut filled) = mpsc::unbounded_channel();
        let clients = [("p".to_string(), client)].into_iter().collect();
        tokio::spawn(run(clients, writer.clone(), rx, filled_tx));
        let series = Ask {
            kind: Kind::Series,
            ..ask(10)
        };
        asks.send(series.clone()).unwrap();
        assert_eq!(filled.recv().await.unwrap().ask, series);
        let conn = writer.lock().unwrap();
        let eps = store::episodes(&conn, "p", 10).unwrap();
        let order: Vec<(i64, Option<i64>)> = eps.iter().map(|e| (e.stream_id, e.num)).collect();
        assert_eq!(order, [(30, Some(1)), (31, Some(2))]);
        let key = store::title_keys(
            &conn,
            "p",
            Kind::Series,
            store::TitleView::All(store::Order::Provider),
        )
        .unwrap()[0];
        let row = store::title(&conn, Kind::Series, key).unwrap().unwrap();
        assert!(row.info);
        assert_eq!((row.seasons, row.last_year), (Some(1), Some(2021)));
    }

    fn ask(id: i64) -> Ask {
        Ask {
            provider_id: "p".into(),
            kind: Kind::Movie,
            id,
        }
    }

    #[test]
    fn latest_selection_goes_first_and_passed_over_ones_drop() {
        let mut queue = VecDeque::new();
        for id in [1, 2, 3, 2, 4] {
            push(&mut queue, ask(id));
        }
        // A series sharing a movie's id is another title.
        push(
            &mut queue,
            Ask {
                kind: Kind::Series,
                ..ask(2)
            },
        );
        let order: Vec<(Kind, i64)> = queue.iter().map(|a| (a.kind, a.id)).collect();
        assert_eq!(
            order,
            [(Kind::Series, 2), (Kind::Movie, 4), (Kind::Movie, 2)]
        );
        // Another provider's list replaced this one's.
        push(
            &mut queue,
            Ask {
                provider_id: "q".into(),
                ..ask(5)
            },
        );
        assert_eq!(queue.len(), 1);
    }

    /// The vod ids of the info asked of `fake`, in order.
    fn fetched(fake: &FakeXtream) -> Vec<i64> {
        fake.targets()
            .iter()
            .filter(|t| fake_xtream::action(t) == "get_vod_info")
            .filter_map(|t| t.split("vod_id=").nth(1)?.parse().ok())
            .collect()
    }

    #[tokio::test]
    async fn fetches_each_movie_once_per_list_refresh() {
        let fake = FakeXtream::start(fake_xtream::active).await;
        let dir = tempfile::tempdir().unwrap();
        let writer: Writer = Arc::new(Mutex::new(store::open(&dir.path().join("v.db")).unwrap()));
        let client = Arc::new(Client::with_limits(fake.provider(), FAST));
        let provider = fake.provider();
        import::movies(&client, &writer, &provider).await.unwrap();

        let (asks, rx) = mpsc::unbounded_channel();
        let (filled_tx, mut filled) = mpsc::unbounded_channel();
        let clients = [("p".to_string(), client)].into_iter().collect();
        tokio::spawn(run(clients, writer.clone(), rx, filled_tx));
        let read = |writer: &Writer| {
            let conn = writer.lock().unwrap();
            let key = store::title_keys(
                &conn,
                "p",
                store::Kind::Movie,
                store::TitleView::All(store::Order::Provider),
            )
            .unwrap()[0];
            store::title(&conn, store::Kind::Movie, key)
                .unwrap()
                .unwrap()
        };
        assert!(!read(&writer).info);

        asks.send(ask(20)).unwrap();
        assert_eq!(filled.recv().await.unwrap().ask, ask(20));
        let row = read(&writer);
        assert!(row.info);
        assert_eq!((row.year, row.length), (Some(2019), Some(5594)));

        // Once per refresh: 20 again is not fetched, an unknown one is
        // fetched and stored nowhere.
        asks.send(ask(20)).unwrap();
        asks.send(ask(21)).unwrap();
        assert_eq!(filled.recv().await.unwrap().ask, ask(21));
        assert_eq!(fetched(&fake), [20, 21]);

        // A new import clears it, and it is fetched again. Stamped a
        // second later, as a real refresh is.
        let client = Client::with_limits(fake.provider(), FAST);
        import::movies(&client, &writer, &provider).await.unwrap();
        writer
            .lock()
            .unwrap()
            .execute(
                "UPDATE provider SET last_movies_refresh = last_movies_refresh + 1",
                [],
            )
            .unwrap();
        assert!(!read(&writer).info);
        asks.send(ask(20)).unwrap();
        assert_eq!(filled.recv().await.unwrap().ask, ask(20));
        assert_eq!(fetched(&fake), [20, 21, 20]);
        assert!(read(&writer).info);
    }
}
