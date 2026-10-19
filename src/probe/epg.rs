//! `--probe-epg <url-or-path>` (`devtools` only, EPG sources ticket 01): how
//! an external XMLTV feed matches the provider's channels. Streams the feed
//! once, gzipped or plain, and prints counts only: size, channels,
//! programmes, reach, time offsets; then matches against the database by
//! exact id and by normalised name, and compares programme times with the
//! provider's guide where both have the channel. Never URLs, hosts, ids or
//! names, so the output can be pasted anywhere.

use crate::config::{self, Provider};
use crate::epg_sources::{Index, Match, Norm, epg_norm};
use crate::import::{ChunkReader, FeedChannel, unix_now, xmltv_time};
use crate::store::{self, name_norm};
use crate::xtream::http;
use flate2::bufread::MultiGzDecoder;
use quick_xml::events::Event;
use reqwest::Url;
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::rc::Rc;
use std::time::Instant;
use tokio::sync::mpsc;

/// Feed programmes starting this close to now are kept for the shift check.
const SAMPLE: i64 = 6 * 3600;
/// Matched channels whose times are compared with the provider's guide.
const SHIFT_CHANNELS: usize = 500;

pub async fn run(provider: Provider, source: String) -> Result<(), String> {
    let db = store::open_reader(&config::database_path()).map_err(|e| e.to_string())?;
    let channels = provider_channels(&db, &provider.id)?;
    if channels.is_empty() {
        return Err("no channels stored for the first provider: run the app once".into());
    }
    let now = unix_now();
    let t = Instant::now();
    let feed = match Url::parse(&source) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => fetch(url, now).await?,
        _ => {
            let file = std::fs::File::open(&source).map_err(|e| format!("feed: {e}"))?;
            tokio::task::spawn_blocking(move || scan(file, now))
                .await
                .map_err(|e| format!("scan task failed: {e}"))??
        }
    };
    print_feed(&feed, now, t.elapsed().as_secs_f64());

    let guided: HashSet<String> = db
        .prepare("SELECT DISTINCT epg_channel_id FROM programme WHERE provider_id = ?1")
        .and_then(|mut s| s.query_map([&provider.id], |r| r.get(0))?.collect())
        .map_err(|e| e.to_string())?;
    let has_guide = |c: &Channel| c.guide_id.as_ref().is_some_and(|g| guided.contains(g));
    let rules: [(&str, &[Norm]); 3] = [
        ("name_norm", &[name_norm]),
        ("epg_norm", &[epg_norm]),
        ("name_norm, then epg_norm", &[name_norm, epg_norm]),
    ];
    let mut last = Vec::new();
    for set in ["all", "shown", "favourites"] {
        let keep = |c: &Channel| match set {
            "shown" => c.shown,
            "favourites" => c.favourite,
            _ => true,
        };
        let total = channels.iter().filter(|c| keep(c)).count();
        let guide = channels.iter().filter(|c| keep(c) && has_guide(c)).count();
        println!("match, {set} ({total} channels, {guide} with the provider's guide):");
        for (rule, norms) in rules {
            let index = Index::new(&feed.channels, norms);
            let matches: Vec<Match> = channels
                .iter()
                .map(|c| index.find(c.guide_id.as_deref(), &c.name))
                .collect();
            let (mut n, mut used) = ([0usize; 6], HashSet::new());
            for (c, m) in channels.iter().zip(&matches).filter(|(c, _)| keep(c)) {
                let found = match *m {
                    Match::Id(i) => (n[0] += 1, Some(i)).1,
                    Match::Name(i) => (n[1] += 1, Some(i)).1,
                    Match::Ambiguous => (n[2] += 1, None).1,
                    Match::None => (n[3] += 1, None).1,
                };
                if let Some(i) = found {
                    used.insert(i);
                    if has_guide(c) {
                        n[4] += 1;
                    } else if feed.reach.contains_key(&feed.channels[i].id) {
                        n[5] += 1;
                    }
                }
            }
            println!(
                "  {rule}: by id {}, by name {}, ambiguous {}, none {}; {} feed channels used; of matched, {} have the provider's guide, {} gain one",
                n[0],
                n[1],
                n[2],
                n[3],
                used.len(),
                n[4],
                n[5]
            );
            last = matches;
        }
    }

    // Programme times against the provider's guide, on channels both have,
    // matched by the last rule.
    for (how, by_id) in [("matched by id", true), ("matched by name", false)] {
        let mut pairs: Vec<(usize, &str)> = channels
            .iter()
            .zip(&last)
            .filter_map(|(c, m)| match (*m, by_id) {
                (Match::Id(i), true) | (Match::Name(i), false) => c
                    .guide_id
                    .as_deref()
                    .filter(|g| guided.contains(*g))
                    .map(|g| (i, g)),
                _ => None,
            })
            .collect();
        pairs.sort_unstable();
        pairs.dedup();
        pairs.truncate(SHIFT_CHANNELS);
        shifts(&db, &provider.id, &feed, &pairs, now, how)?;
    }
    Ok(())
}

/// What the store has about one of the provider's channels.
struct Channel {
    name: String,
    guide_id: Option<String>,
    shown: bool,
    favourite: bool,
}

fn provider_channels(db: &rusqlite::Connection, provider_id: &str) -> Result<Vec<Channel>, String> {
    db.prepare(
        "SELECT c.name, NULLIF(c.epg_channel_id, ''), c.shown AND coalesce(k.shown, 1),
             f.stream_id IS NOT NULL
         FROM channel c LEFT JOIN favourite f ON f.provider_id = c.provider_id
             AND f.kind = 'live' AND f.stream_id = c.stream_id
         LEFT JOIN category k ON k.provider_id = c.provider_id AND k.kind = 'live'
             AND k.category_id = c.category_id
         WHERE c.provider_id = ?1",
    )
    .and_then(|mut s| {
        s.query_map([provider_id], |r| {
            Ok(Channel {
                name: r.get(0)?,
                guide_id: r.get(1)?,
                shown: r.get(2)?,
                favourite: r.get(3)?,
            })
        })?
        .collect()
    })
    .map_err(|e| e.to_string())
}

/// Streams the feed through [`scan`]. Errors carry no URL.
async fn fetch(url: Url, now: i64) -> Result<Feed, String> {
    let (tx, rx) = mpsc::channel(8);
    let job = tokio::task::spawn_blocking(move || scan(ChunkReader::new(rx), now));
    let fetched = async {
        let mut resp = http().get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Ok(Err(format!("feed: HTTP {status}")));
        }
        while let Some(chunk) = resp.chunk().await? {
            if tx.send(Ok(chunk)).await.is_err() {
                break;
            }
        }
        Ok::<_, reqwest::Error>(Ok(()))
    }
    .await
    .map_err(|e| format!("feed: {}", e.without_url()))
    .and_then(|r| r);
    if let Err(e) = &fetched {
        let _ = tx.send(Err(e.clone())).await;
    }
    drop(tx);
    let scanned = job.await.map_err(|e| format!("scan task failed: {e}"))?;
    fetched?;
    scanned
}

/// A feed programme near now: start and lowercased title.
type Sampled = (i64, String);

#[derive(Default)]
struct Feed {
    /// Bytes read, and after gunzip when the body was gzipped.
    read: u64,
    xml: u64,
    gzipped: bool,
    channels: Vec<FeedChannel>,
    /// Per programme channel id: first start, last stop, count.
    reach: HashMap<String, (i64, i64, usize)>,
    programmes: usize,
    unusable: usize,
    /// A programme start's zone suffix (`+0000`, `none`) to its count.
    zones: BTreeMap<String, usize>,
    /// Per channel id, programmes starting within [`SAMPLE`] of now.
    sample: HashMap<String, Vec<Sampled>>,
}

/// A `Read` that counts what passes through it.
struct Counting<R> {
    inner: R,
    n: Rc<Cell<u64>>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let k = self.inner.read(buf)?;
        self.n.set(self.n.get() + k as u64);
        Ok(k)
    }
}

/// Which text is being collected.
#[derive(PartialEq)]
enum Field {
    Other,
    Name,
    Title,
}

/// Pull-parses a whole XMLTV body, gzipped or not, into counts. Only
/// channel ids and names, per-channel reach, and programmes near now are
/// held.
fn scan(body: impl Read, now: i64) -> Result<Feed, String> {
    let (read, xml) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let mut body = BufReader::with_capacity(
        64 * 1024,
        Counting {
            inner: body,
            n: read.clone(),
        },
    );
    let gzipped = body
        .fill_buf()
        .map_err(|e| format!("feed: {e}"))?
        .starts_with(&[0x1f, 0x8b]);
    let inner: Box<dyn Read> = if gzipped {
        Box::new(MultiGzDecoder::new(body))
    } else {
        Box::new(body)
    };
    let src = BufReader::with_capacity(
        64 * 1024,
        Counting {
            inner,
            n: xml.clone(),
        },
    );
    let mut feed = Feed {
        gzipped,
        ..Feed::default()
    };
    let mut reader = quick_xml::Reader::from_reader(src);
    let mut buf = Vec::new();
    let mut channel: Option<FeedChannel> = None;
    let (mut prog, mut start, mut stop, mut zone) = (None::<String>, None, None, String::new());
    let (mut text, mut has_title) = (String::new(), false);
    let mut field = Field::Other;
    let mut complete = false;
    loop {
        let event = reader.read_event_into(&mut buf).map_err(|e| match e {
            quick_xml::Error::Io(e) => format!("feed interrupted: {e}"),
            _ => format!("invalid XMLTV at byte {}", reader.error_position()),
        })?;
        match event {
            Event::Start(e) => match e.name().as_ref() {
                "channel" => {
                    let mut c = FeedChannel::default();
                    for a in e.attributes().flatten() {
                        if a.key.as_ref() == "id"
                            && let Ok(v) = a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        {
                            c.id = v.into_owned();
                        }
                    }
                    channel = Some(c);
                }
                "display-name" if channel.is_some() => {
                    field = Field::Name;
                    text.clear();
                }
                "programme" => {
                    let mut c = String::new();
                    (start, stop) = (None, None);
                    zone = "none".into();
                    for a in e.attributes().flatten() {
                        let Ok(v) = a.normalized_value(quick_xml::XmlVersion::Implicit1_0) else {
                            continue;
                        };
                        match a.key.as_ref() {
                            "channel" => c = v.into_owned(),
                            "start" => {
                                start = xmltv_time(&v);
                                let z = v
                                    .trim_start()
                                    .trim_start_matches(|c: char| c.is_ascii_digit());
                                if !z.trim().is_empty() {
                                    zone = z.trim().to_owned();
                                }
                            }
                            "stop" => stop = xmltv_time(&v),
                            _ => {}
                        }
                    }
                    prog = Some(c);
                    has_title = false;
                    text.clear();
                }
                "title" if prog.is_some() && !has_title => field = Field::Title,
                _ => {}
            },
            Event::Empty(e) => match e.name().as_ref() {
                "channel" => feed.channels.push(FeedChannel::default()),
                "programme" => feed.unusable += 1,
                _ => {}
            },
            Event::Text(t) if field != Field::Other => text.push_str(&t.xml10_content()),
            Event::CData(t) if field != Field::Other => text.push_str(&t.xml10_content()),
            Event::GeneralRef(r) if field != Field::Other => match r.resolve_char_ref() {
                Ok(Some(c)) => text.push(c),
                _ => {
                    if let Some(s) = quick_xml::escape::resolve_predefined_entity(&r) {
                        text.push_str(s);
                    }
                }
            },
            Event::End(e) => match e.name().as_ref() {
                "display-name" if field == Field::Name => {
                    field = Field::Other;
                    let name = text.trim();
                    if let Some(c) = &mut channel
                        && !name.is_empty()
                    {
                        c.names.push(name.to_owned());
                    }
                }
                "title" if field == Field::Title => {
                    field = Field::Other;
                    has_title = true;
                }
                "channel" => feed.channels.extend(channel.take()),
                "programme" => {
                    let c = prog.take().unwrap_or_default();
                    let title = text.trim();
                    match (start, stop) {
                        (Some(start), Some(stop))
                            if stop > start && !c.is_empty() && !title.is_empty() =>
                        {
                            feed.programmes += 1;
                            *feed.zones.entry(std::mem::take(&mut zone)).or_default() += 1;
                            if (start - now).abs() <= SAMPLE {
                                feed.sample
                                    .entry(c.clone())
                                    .or_default()
                                    .push((start, title.to_lowercase()));
                            }
                            let r = feed.reach.entry(c).or_insert((start, stop, 0));
                            r.0 = r.0.min(start);
                            r.1 = r.1.max(stop);
                            r.2 += 1;
                        }
                        _ => feed.unusable += 1,
                    }
                }
                "tv" => complete = true,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !complete {
        return Err("feed incomplete: no closing </tv>".into());
    }
    drop(reader);
    (feed.read, feed.xml) = (read.get(), xml.get());
    Ok(feed)
}

fn print_feed(feed: &Feed, now: i64, secs: f64) {
    let mb = |b: u64| b as f64 / 1e6;
    if feed.gzipped {
        println!(
            "feed: {:.1} MB gzipped, {:.1} MB XML, read in {secs:.1} s",
            mb(feed.read),
            mb(feed.xml)
        );
    } else {
        println!("feed: {:.1} MB XML, read in {secs:.1} s", mb(feed.xml));
    }
    let ids: HashSet<&str> = feed.channels.iter().map(|c| c.id.as_str()).collect();
    let with_programmes = ids.iter().filter(|i| feed.reach.contains_key(**i)).count();
    let names: usize = feed.channels.iter().map(|c| c.names.len()).sum();
    println!(
        "  channels {} ({} distinct ids, {names} display names, {with_programmes} with programmes); programmes {} ({} unusable), on {} channel ids ({} not declared)",
        feed.channels.len(),
        ids.len(),
        feed.programmes,
        feed.unusable,
        feed.reach.len(),
        feed.reach
            .keys()
            .filter(|k| !ids.contains(k.as_str()))
            .count(),
    );
    let first = feed.reach.values().map(|r| r.0).min();
    let last = feed.reach.values().map(|r| r.1).max();
    if let (Some(first), Some(last)) = (first, last) {
        println!(
            "  reach: {:.1} days back, {:.1} days ahead at most",
            (now - first) as f64 / 86_400.0,
            (last - now) as f64 / 86_400.0
        );
    }
    let mut back = [0; 6];
    let mut ahead = [0; 5];
    for &(first, last, _) in feed.reach.values() {
        back[match (now - first) / 3600 {
            ..1 => 0,
            1..6 => 1,
            6..24 => 2,
            24..72 => 3,
            72..168 => 4,
            _ => 5,
        }] += 1;
        ahead[match (last - now) / 3600 {
            ..24 => 0,
            24..72 => 1,
            72..144 => 2,
            144..192 => 3,
            _ => 4,
        }] += 1;
    }
    println!(
        "  per channel back: <1h {} | 1-6h {} | 6-24h {} | 1-3d {} | 3-7d {} | >7d {}",
        back[0], back[1], back[2], back[3], back[4], back[5]
    );
    println!(
        "  per channel ahead: <1d {} | 1-3d {} | 3-6d {} | 6-8d {} | >8d {}",
        ahead[0], ahead[1], ahead[2], ahead[3], ahead[4]
    );
    let zones: Vec<String> = feed.zones.iter().map(|(z, n)| format!("{z} {n}")).collect();
    println!("  start offsets: {}", zones.join(", "));
}

/// For each matched channel with the provider's guide, each feed programme
/// near now is paired with the provider's programme of the same title
/// starting nearest it, within 12 h. Prints how far apart, feed minus
/// provider, as each channel's most common difference in 15 min steps.
fn shifts(
    db: &rusqlite::Connection,
    provider_id: &str,
    feed: &Feed,
    pairs: &[(usize, &str)],
    now: i64,
    how: &str,
) -> Result<(), String> {
    let mut stmt = db
        .prepare(
            "SELECT start, title FROM programme
             WHERE provider_id = ?1 AND epg_channel_id = ?2 AND start BETWEEN ?3 AND ?4",
        )
        .map_err(|e| e.to_string())?;
    let (mut compared, mut programmes) = (0, 0);
    let mut modes: BTreeMap<i64, usize> = BTreeMap::new();
    for &(i, guide_id) in pairs {
        let Some(sample) = feed.sample.get(&feed.channels[i].id) else {
            continue;
        };
        let theirs: Vec<(i64, String)> = stmt
            .query_map(
                rusqlite::params![
                    provider_id,
                    guide_id,
                    now - SAMPLE - 43_200,
                    now + SAMPLE + 43_200
                ],
                |r| Ok((r.get(0)?, r.get::<_, String>(1)?.to_lowercase())),
            )
            .and_then(|r| r.collect())
            .map_err(|e| e.to_string())?;
        let mut steps: HashMap<i64, usize> = HashMap::new();
        for (start, title) in sample {
            let nearest = theirs
                .iter()
                .filter(|(_, t)| t == title)
                .map(|(s, _)| start - s)
                .filter(|d| d.abs() <= 43_200)
                .min_by_key(|d| d.abs());
            if let Some(d) = nearest {
                *steps
                    .entry((d as f64 / 900.0).round() as i64 * 15)
                    .or_default() += 1;
                programmes += 1;
            }
        }
        if let Some((&mode, _)) = steps.iter().max_by_key(|(m, n)| (**n, -m.abs())) {
            compared += 1;
            *modes.entry(mode).or_default() += 1;
        }
    }
    let mut modes: Vec<(i64, usize)> = modes.into_iter().collect();
    modes.sort_by_key(|&(m, n)| (std::cmp::Reverse(n), m));
    let shown: Vec<String> = modes
        .iter()
        .take(6)
        .map(|(m, n)| format!("{m:+} min {n}"))
        .collect();
    println!(
        "times, {how}, feed minus provider ({} channel pairs tried): {compared} channels, {programmes} programmes paired by title",
        pairs.len()
    );
    if !shown.is_empty() {
        println!("  per channel, most common: {}", shown.join(", "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;

    const XML: &str = r#"<?xml version="1.0"?><tv>
        <channel id="harbour.one"><display-name>Harbour One</display-name><display-name>H1 &amp; Co</display-name></channel>
        <channel id="summit"><display-name>Summit</display-name></channel>
        <programme start="20261012100000 +0200" stop="20261012110000 +0200" channel="harbour.one"><title>Morning</title></programme>
        <programme start="20261012110000 +0200" stop="20261012120000 +0200" channel="harbour.one"><title>Noon</title></programme>
        <programme start="202610121200" stop="202610121300" channel="ghost"><title>Late</title></programme>
        <programme start="20261012120000 +0200" stop="20261012130000 +0200" channel="summit"></programme>
    </tv>"#;

    #[test]
    fn scan_counts_plain_and_gzipped() {
        // 09:00 UTC; the first programme starts at 08:00 UTC.
        let now = xmltv_time("20261012090000").unwrap();
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(XML.as_bytes()).unwrap();
        let gz = gz.finish().unwrap();
        for (body, gzipped) in [(XML.as_bytes().to_vec(), false), (gz, true)] {
            let f = scan(&body[..], now).unwrap();
            assert_eq!(f.gzipped, gzipped);
            assert_eq!(f.xml, XML.len() as u64);
            assert_eq!(f.read, body.len() as u64);
            assert_eq!(f.channels.len(), 2);
            assert_eq!(f.channels[0].names, ["Harbour One", "H1 & Co"]);
            assert_eq!(f.programmes, 3);
            assert_eq!(f.unusable, 1);
            assert_eq!(f.zones.get("+0200"), Some(&2));
            assert_eq!(f.zones.get("none"), Some(&1));
            assert_eq!(f.reach["harbour.one"], (now - 3600, now + 3600, 2));
            assert_eq!(f.sample["harbour.one"][0], (now - 3600, "morning".into()));
        }
        assert!(scan(&XML.as_bytes()[..XML.len() - 6], 0).is_err());
    }
}
