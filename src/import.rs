//! Streams provider responses into the store (SPEC 7.1; ADR 0003).
//!
//! No list (`get_live_streams`, `get_vod_streams`, `get_series`) nor
//! `xmltv.php` is ever held whole: the HTTP
//! body flows through a bounded channel into a blocking task that parses one
//! element at a time and inserts it, all inside one transaction. Any failure
//! (network, truncated or invalid body, database) rolls back and leaves the
//! previous data untouched.

use crate::config::Provider;
use crate::m3u;
use crate::store::{self, Kind, NewChannel, NewMovie, NewProgramme, NewSeries};
use crate::xtream::{Chunk, Client, Movie, Series, Stream};
use bytes::{Buf, Bytes};
use flate2::bufread::MultiGzDecoder;
use lzma_rust2::XzReader;
use quick_xml::events::Event;
use rusqlite::Connection;
use serde::de::{DeserializeOwned, DeserializeSeed, Deserializer, SeqAccess, Visitor};
use std::fmt;
use std::io::{self, BufRead, BufReader, Read};
use std::marker::PhantomData;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

/// Chunks buffered between the download and the parser. Bounds memory.
const CHUNKS_IN_FLIGHT: usize = 8;

/// The worker runtime's single writer connection.
pub type Writer = Arc<Mutex<Connection>>;

/// Imports categories and channels. `ext` is the stream extension from the
/// account check, which the caller runs once per refresh (SPEC 6.2).
pub async fn channels(
    client: &Client,
    writer: &Writer,
    provider: &Provider,
    ext: &'static str,
) -> Result<usize, String> {
    let categories: Vec<(String, String)> = client
        .live_categories()
        .await?
        .into_iter()
        .filter_map(|c| Some((c.category_id?, c.category_name)))
        .collect();

    let (tx, rx) = mpsc::channel(CHUNKS_IN_FLIGHT);
    let job = {
        let (writer, id, name) = (writer.clone(), provider.id.clone(), provider.name.clone());
        tokio::task::spawn_blocking(move || {
            let mut conn = writer.lock().unwrap_or_else(|e| e.into_inner());
            let n = write_channels(
                &mut conn,
                (&id, &name),
                &categories,
                ChunkReader::new(rx),
                ext,
                unix_now(),
            )?;
            log::info!("{} categories, {n} channels imported", categories.len());
            Ok::<_, String>(n)
        })
    };
    let fetched = client.live_streams(tx).await;
    let stored = job.await.map_err(|e| format!("import task failed: {e}"))?;
    // A network error explains a parse error, so it wins.
    fetched?;
    stored
}

/// Imports an M3U provider's playlist (M3U ticket 02), fetched through its
/// client, or read from its path when the file changed since the last
/// read (`force` reads it anyway). Returns the channels imported and the
/// entries skipped for having no URL, or `None` for a path unchanged.
pub async fn playlist(
    client: &Client,
    writer: &Writer,
    provider: &Provider,
    force: bool,
) -> Result<Option<(usize, usize)>, String> {
    let t = std::time::Instant::now();
    let label = provider.playlist_label();
    let path = provider.playlist_path();
    let mtime = if let Some(path) = path {
        let mtime = file_mtime(path).map_err(|e| format!("{}: {label}: {e}", provider.name))?;
        let stored = {
            let conn = writer.lock().unwrap_or_else(|e| e.into_inner());
            store::playlist_mtime(&conn, &provider.id)
                .map_err(|e| format!("reading provider: {e}"))?
        };
        if !force && stored == Some(mtime) {
            return Ok(None);
        }
        Some(mtime)
    } else {
        None
    };
    let (tx, rx) = mpsc::channel(CHUNKS_IN_FLIGHT);
    let job = {
        let (writer, id, name) = (writer.clone(), provider.id.clone(), provider.name.clone());
        let (path, label) = (path.map(Path::to_owned), label.clone());
        tokio::task::spawn_blocking(move || {
            let mut conn = writer.lock().unwrap_or_else(|e| e.into_inner());
            match path {
                Some(path) => {
                    let file =
                        std::fs::File::open(&path).map_err(|e| format!("{name}: {label}: {e}"))?;
                    write_playlist(&mut conn, (&id, &name), file, mtime, unix_now())
                }
                None => write_playlist(
                    &mut conn,
                    (&id, &name),
                    ChunkReader::new(rx),
                    None,
                    unix_now(),
                ),
            }
        })
    };
    let fetched = match mtime {
        Some(_) => Ok(0),
        None => client.playlist(tx).await,
    };
    let stored = job.await.map_err(|e| format!("import task failed: {e}"))?;
    fetched?;
    let (n, skipped) = stored?;
    log::info!(
        "{label}: {n} channels imported, {skipped} entries skipped, {} ms from the request",
        t.elapsed().as_millis()
    );
    Ok(Some((n, skipped)))
}

/// Whether an M3U provider's playlist path changed since it was last
/// read: `None` for a URL, which goes by age. A file that cannot be read
/// counts as changed, so the import says why.
pub fn playlist_changed(writer: &Writer, provider: &Provider) -> Option<bool> {
    let path = provider.playlist_path()?;
    let conn = writer.lock().unwrap_or_else(|e| e.into_inner());
    let stored = store::playlist_mtime(&conn, &provider.id).ok().flatten();
    Some(file_mtime(path).ok().is_none_or(|m| stored != Some(m)))
}

/// A file's modification time in ns.
fn file_mtime(path: &Path) -> io::Result<i64> {
    let modified = std::fs::metadata(path)?.modified()?;
    Ok(modified
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64))
}

/// Imports the provider's movies and their categories (VOD ticket 02).
pub async fn movies(
    client: &Client,
    writer: &Writer,
    provider: &Provider,
) -> Result<usize, String> {
    vod(client, writer, provider, Kind::Movie).await
}

/// Imports the provider's series and their categories (VOD ticket 02).
pub async fn series(
    client: &Client,
    writer: &Writer,
    provider: &Provider,
) -> Result<usize, String> {
    vod(client, writer, provider, Kind::Series).await
}

async fn vod(
    client: &Client,
    writer: &Writer,
    provider: &Provider,
    kind: Kind,
) -> Result<usize, String> {
    let (categories, list) = match kind {
        Kind::Movie => ("get_vod_categories", "get_vod_streams"),
        _ => ("get_series_categories", "get_series"),
    };
    let categories: Vec<(String, String)> = client
        .categories(categories)
        .await?
        .into_iter()
        .filter_map(|c| Some((c.category_id?, c.category_name)))
        .collect();

    let (tx, rx) = mpsc::channel(CHUNKS_IN_FLIGHT);
    let job = {
        let (writer, id, name) = (writer.clone(), provider.id.clone(), provider.name.clone());
        tokio::task::spawn_blocking(move || {
            let mut conn = writer.lock().unwrap_or_else(|e| e.into_inner());
            let body = ChunkReader::new(rx);
            let n = match kind {
                Kind::Movie => write_movies(&mut conn, (&id, &name), &categories, body, unix_now()),
                _ => write_series(&mut conn, (&id, &name), &categories, body, unix_now()),
            }?;
            log::info!(
                "{} {} categories, {n} titles imported",
                categories.len(),
                kind.as_str()
            );
            Ok::<_, String>(n)
        })
    };
    let fetched = client.list(list, tx).await;
    let stored = job.await.map_err(|e| format!("import task failed: {e}"))?;
    fetched?;
    stored
}

/// Imports the provider's XMLTV, its times moved by `shift` seconds. Run
/// after [`channels`] (SPEC 7.2), which records the provider row this
/// stamps.
pub async fn epg(
    client: &Client,
    writer: &Writer,
    provider: &Provider,
    shift: i64,
) -> Result<usize, String> {
    let (tx, rx) = mpsc::channel(CHUNKS_IN_FLIGHT);
    let job = {
        let (writer, id) = (writer.clone(), provider.id.clone());
        tokio::task::spawn_blocking(move || {
            let mut conn = writer.lock().unwrap_or_else(|e| e.into_inner());
            write_epg(&mut conn, &id, ChunkReader::new(rx), shift, unix_now())
        })
    };
    let fetched = client.xmltv(tx).await;
    let stored = job.await.map_err(|e| format!("import task failed: {e}"))?;
    fetched?;
    stored
}

/// Programmes that ended more than this long ago are not kept, on a
/// channel without an archive. One with an archive keeps its days.
pub(crate) const EPG_KEEP_PAST: i64 = 3600;
/// Programmes starting more than this far ahead are not kept.
pub(crate) const EPG_KEEP_AHEAD: i64 = 3 * 86_400;

/// Replaces a provider's programmes with those in `body`, an XMLTV document,
/// gzipped, xz or plain, in one transaction, their times moved by `shift`
/// seconds. Only programmes overlapping `now - 1 h .. now + 3 days` are
/// kept, or from further back on a guide id with an archive: as many days
/// back as its longest archive. Returns how many were.
pub fn write_epg(
    conn: &mut Connection,
    provider_id: &str,
    body: impl Read,
    shift: i64,
    now: i64,
) -> Result<usize, String> {
    let db = |e: rusqlite::Error| format!("saving EPG: {e}");
    let tx = conn.transaction().map_err(db)?;
    store::begin_epg_import(&tx, provider_id, now).map_err(db)?;
    // Channels refresh first (SPEC 7.2), so their archives are current.
    let archives = store::archive_days_by_guide(&tx, provider_id).map_err(db)?;

    let xml = unpacked(body).map_err(|e| format!("EPG download interrupted: {e}"))?;
    let until = now + EPG_KEEP_AHEAD;
    let (mut kept, mut outside) = (0, 0);
    let skipped = parse_xmltv(xml, |p| {
        let p = &p.shifted(shift);
        let past = archives
            .get(p.epg_channel_id)
            .map_or(EPG_KEEP_PAST, |days| days * 86_400);
        if p.stop <= now - past || p.start >= until {
            outside += 1;
            return Ok(());
        }
        kept += 1;
        store::insert_programme(&tx, provider_id, p).map_err(db)
    })?;
    log::info!("{kept} programmes imported, {outside} outside the window, {skipped} unusable");
    store::finish_epg_import(&tx, provider_id, now).map_err(db)?;
    tx.commit().map_err(db)?;
    Ok(kept)
}

/// `body`, gunzipped or unxzed if it starts as gzip or xz does (M3U
/// ticket 05). xz's checks fail a cut-off body; `</tv>` is still what
/// says a feed is whole.
pub(crate) fn unpacked<'a>(mut body: impl Read + 'a) -> io::Result<Box<dyn BufRead + 'a>> {
    // xz's magic is 6 bytes, which a first chunk may not hold.
    let mut head = [0; 6];
    let n = Filled(&mut body).read(&mut head)?;
    let head = head[..n].to_vec();
    let gzip = head.starts_with(&[0x1f, 0x8b]);
    let xz = head.starts_with(b"\xfd7zXZ\0");
    let body = BufReader::with_capacity(64 * 1024, Read::chain(io::Cursor::new(head), body));
    Ok(if gzip {
        Box::new(BufReader::with_capacity(
            64 * 1024,
            MultiGzDecoder::new(body),
        ))
    } else if xz {
        Box::new(BufReader::with_capacity(
            64 * 1024,
            XzReader::new(Filled(body), true),
        ))
    } else {
        Box::new(body)
    })
}

/// A reader whose reads fill the buffer unless the body ends. lzma-rust2
/// 0.23's `XzReader` reads a block's padding with one `read`, so a chunk
/// ending inside the padding failed a whole feed.
struct Filled<R>(R);

impl<R: Read> Read for Filled<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut n = 0;
        while n < buf.len() {
            match self.0.read(&mut buf[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(n)
    }
}

/// `data` xz-compressed, as a guide feed may be.
#[cfg(test)]
pub(crate) fn xz(data: &[u8]) -> Vec<u8> {
    use lzma_rust2::{XzOptions, XzWriter};
    use std::io::Write;
    let mut w = XzWriter::new(Vec::new(), XzOptions::with_preset(1)).unwrap();
    w.write_all(data).unwrap();
    w.finish().unwrap()
}

impl NewProgramme<'_> {
    /// The same programme `shift` seconds later.
    fn shifted(&self, shift: i64) -> Self {
        Self {
            start: self.start + shift,
            stop: self.stop + shift,
            ..*self
        }
    }
}

/// Which child of `<programme>` or `<channel>` text currently belongs to.
#[derive(PartialEq)]
enum Field {
    Other,
    Title,
    Desc,
    Name,
}

/// A `<channel>` of an XMLTV document: its id, display names and icon.
#[derive(Clone, Debug, Default)]
pub struct FeedChannel {
    pub id: String,
    pub names: Vec<String>,
    pub icon: Option<String>,
}

/// What [`parse_xmltv_items`] hands over.
pub enum Item<'a> {
    Channel(&'a FeedChannel),
    Programme(&'a NewProgramme<'a>),
}

/// Pull-parses XMLTV, calling `each` per `<programme>` with a start, stop,
/// channel and title. Returns how many programmes lacked one of those.
/// Only the current programme is held in memory.
pub(crate) fn parse_xmltv(
    src: impl BufRead,
    mut each: impl FnMut(&NewProgramme) -> Result<(), String>,
) -> Result<usize, String> {
    parse_xmltv_items(src, |item| match item {
        Item::Programme(p) => each(p),
        Item::Channel(_) => Ok(()),
    })
}

/// As [`parse_xmltv`], also calling `each` per `<channel>` with an id.
pub(crate) fn parse_xmltv_items(
    src: impl BufRead,
    mut each: impl FnMut(Item) -> Result<(), String>,
) -> Result<usize, String> {
    let mut reader = quick_xml::Reader::from_reader(src);
    let mut buf = Vec::new();
    let (mut channel, mut title, mut desc) = (String::new(), String::new(), String::new());
    let (mut start, mut stop) = (None, None);
    let mut in_programme = false;
    let mut feed_channel: Option<FeedChannel> = None;
    let mut name = String::new();
    // Only the first `<title>` and `<desc>` count; others are translations.
    let (mut has_title, mut has_desc) = (false, false);
    let mut field = Field::Other;
    let mut complete = false;
    let mut skipped = 0;
    loop {
        let event = reader.read_event_into(&mut buf).map_err(|e| match e {
            quick_xml::Error::Io(e) => format!("EPG download interrupted: {e}"),
            // Position only: the message can quote the body.
            _ => format!("invalid XMLTV at byte {}", reader.error_position()),
        })?;
        match event {
            Event::Start(e) => match e.name().as_ref() {
                "programme" => {
                    in_programme = true;
                    (has_title, has_desc) = (false, false);
                    title.clear();
                    desc.clear();
                    channel.clear();
                    (start, stop) = (None, None);
                    for a in e.attributes().flatten() {
                        let Ok(v) = a.normalized_value(quick_xml::XmlVersion::Implicit1_0) else {
                            continue;
                        };
                        match a.key.as_ref() {
                            "channel" => channel.push_str(&v),
                            "start" => start = xmltv_time(&v),
                            "stop" => stop = xmltv_time(&v),
                            _ => {}
                        }
                    }
                }
                "title" if in_programme && !has_title => field = Field::Title,
                "desc" if in_programme && !has_desc => field = Field::Desc,
                "channel" => {
                    feed_channel = Some(FeedChannel {
                        id: attribute(&e, "id").unwrap_or_default(),
                        ..FeedChannel::default()
                    });
                }
                "display-name" if feed_channel.is_some() => {
                    field = Field::Name;
                    name.clear();
                }
                "icon" => icon(&mut feed_channel, &e),
                _ => {}
            },
            // A `<programme/>` without children has no title.
            Event::Empty(e) if e.name().as_ref() == "programme" => skipped += 1,
            Event::Empty(e) if e.name().as_ref() == "icon" => icon(&mut feed_channel, &e),
            Event::Text(t) => push_text(
                &mut field,
                [&mut title, &mut desc, &mut name],
                &t.xml10_content(),
            ),
            Event::CData(t) => push_text(
                &mut field,
                [&mut title, &mut desc, &mut name],
                &t.xml10_content(),
            ),
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref() {
                    Ok(Some(c)) => Some(c.encode_utf8(&mut [0; 4]).to_string()),
                    _ => quick_xml::escape::resolve_predefined_entity(&r).map(str::to_string),
                };
                if let Some(s) = resolved {
                    push_text(&mut field, [&mut title, &mut desc, &mut name], &s);
                }
            }
            Event::End(e) => match e.name().as_ref() {
                "title" if field == Field::Title => {
                    has_title = true;
                    field = Field::Other;
                }
                "desc" if field == Field::Desc => {
                    has_desc = true;
                    field = Field::Other;
                }
                "display-name" if field == Field::Name => {
                    field = Field::Other;
                    let name = name.trim();
                    if let Some(c) = &mut feed_channel
                        && !name.is_empty()
                    {
                        c.names.push(name.to_owned());
                    }
                }
                "channel" => {
                    if let Some(c) = feed_channel.take().filter(|c| !c.id.is_empty()) {
                        each(Item::Channel(&c))?;
                    }
                }
                "programme" => {
                    in_programme = false;
                    let title = title.trim();
                    match (start, stop) {
                        (Some(start), Some(stop))
                            if stop > start && !channel.is_empty() && !title.is_empty() =>
                        {
                            let desc = desc.trim();
                            each(Item::Programme(&NewProgramme {
                                epg_channel_id: &channel,
                                start,
                                stop,
                                title,
                                description: (!desc.is_empty()).then_some(desc),
                            }))?;
                        }
                        _ => skipped += 1,
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
    // A cut download can still end on a tag boundary.
    if !complete {
        return Err("EPG download incomplete: no closing </tv>".into());
    }
    Ok(skipped)
}

fn push_text(field: &mut Field, [title, desc, name]: [&mut String; 3], text: &str) {
    match field {
        Field::Title => title.push_str(text),
        Field::Desc => desc.push_str(text),
        Field::Name => name.push_str(text),
        Field::Other => {}
    }
}

/// An attribute's value, if the element has it.
fn attribute(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| {
            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
}

/// A channel's first `<icon src>`.
fn icon(channel: &mut Option<FeedChannel>, e: &quick_xml::events::BytesStart) {
    if let Some(c) = channel.as_mut().filter(|c| c.icon.is_none()) {
        c.icon = attribute(e, "src");
    }
}

/// XMLTV time: `YYYYMMDDhhmm[ss] [+-hhmm]`, UTC when the offset is missing.
/// Returns Unix seconds.
pub(crate) fn xmltv_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (digits, zone) = (&s[..split], s[split..].trim());
    if digits.len() != 12 && digits.len() != 14 {
        return None;
    }
    let n = |r: std::ops::Range<usize>| digits[r].parse::<i64>().ok();
    let (year, month, day) = (n(0..4)?, n(4..6)?, n(6..8)?);
    let (hour, minute) = (n(8..10)?, n(10..12)?);
    let second = if digits.len() == 14 { n(12..14)? } else { 0 };
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let offset = match zone.as_bytes() {
        [] | b"Z" | b"UTC" | b"GMT" => 0,
        [sign @ (b'+' | b'-'), rest @ ..]
            if rest.len() == 4 && rest.iter().all(u8::is_ascii_digit) =>
        {
            let hhmm: i64 = zone[1..].parse().ok()?;
            let secs = (hhmm / 100) * 3600 + (hhmm % 100) * 60;
            if *sign == b'-' { -secs } else { secs }
        }
        _ => return None,
    };
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Replaces a provider's categories and channels with `streams`, a
/// `get_live_streams` JSON body, in one transaction.
pub fn write_channels(
    conn: &mut Connection,
    (provider_id, provider_name): (&str, &str),
    categories: &[(String, String)],
    streams: impl Read,
    extension: &str,
    now: i64,
) -> Result<usize, String> {
    let db = |e: rusqlite::Error| format!("saving channels: {e}");
    let tx = conn.transaction().map_err(db)?;
    store::begin_channel_import(
        &tx,
        provider_id,
        provider_name,
        categories
            .iter()
            .map(|(id, name)| (id.as_str(), name.as_str())),
    )
    .map_err(db)?;

    let count = each_element(streams, ("get_live_streams", "channel"), |s: Stream| {
        let Some(stream_id) = s.stream_id else {
            return Ok(false);
        };
        let row = NewChannel {
            stream_id,
            num: s.num,
            name: &s.name,
            category_id: s.category_id.as_deref(),
            epg_channel_id: s.epg_channel_id.as_deref(),
            icon_url: s.stream_icon.as_deref(),
            extension,
            archive_days: s.archive_days(),
            url: None,
            tvg_name: None,
            headers: None,
        };
        store::insert_channel(&tx, provider_id, &row).map(|()| true)
    })
    .map_err(|e| e.into_message(db))?;
    store::finish_channel_import(&tx, provider_id, now).map_err(db)?;
    tx.commit().map_err(db)?;
    Ok(count)
}

/// Replaces a provider's categories and channels with `body`, an M3U
/// playlist (M3U ticket 02), in one transaction: each entry a live
/// channel, its group its category ("Uncategorised" for none), its number
/// `tvg-chno` else its place in the file. `mtime` is stored for a path,
/// and the header's guide feeds (M3U ticket 05).
/// A body that is not a channel list, or has no entries, keeps the old
/// channels. Returns the channels imported and the entries skipped.
pub fn write_playlist(
    conn: &mut Connection,
    (provider_id, provider_name): (&str, &str),
    body: impl Read,
    mtime: Option<i64>,
    now: i64,
) -> Result<(usize, usize), String> {
    let db = |e: rusqlite::Error| format!("saving channels: {e}");
    let tx = conn.transaction().map_err(db)?;
    store::begin_playlist_import(&tx, provider_id, provider_name).map_err(db)?;
    let mut ord = 0;
    let summary = m3u::read(body, |e: m3u::Entry| {
        let url = e.url.as_str();
        let key = e
            .tvg_id
            .as_deref()
            .or(e.tvg_name.as_deref())
            .unwrap_or(&e.title);
        let name = [Some(e.title.as_str()), e.tvg_name.as_deref()]
            .into_iter()
            .flatten()
            .find(|n| !n.is_empty())
            .unwrap_or_else(|| m3u::last_segment(url));
        let row = NewChannel {
            stream_id: 0,
            num: Some(e.chno.unwrap_or(ord + 1)),
            name,
            category_id: Some(e.group.as_deref().unwrap_or(UNCATEGORISED)),
            epg_channel_id: e.tvg_id.as_deref(),
            icon_url: e.logo.as_deref(),
            extension: "",
            archive_days: 0,
            url: Some(url),
            tvg_name: e.tvg_name.as_deref(),
            headers: Some(&e.headers),
        };
        let hashes = (
            stable_hash(&[key.as_bytes()]),
            stable_hash(&[url.as_bytes()]),
        );
        store::insert_playlist_channel(&tx, provider_id, ord, hashes, &row)?;
        ord += 1;
        Ok(())
    })
    .map_err(|e| match e {
        m3u::ReadError::Each(e) => db(e),
        m3u::ReadError::Body(e) => e,
    })?;
    if ord == 0 {
        return Err("the playlist has no channels; keeping the stored ones".into());
    }
    let lost = store::finish_playlist_ids(&tx, provider_id, playlist_id).map_err(db)?;
    store::finish_channel_import(&tx, provider_id, now).map_err(db)?;
    store::set_playlist_mtime(&tx, provider_id, mtime).map_err(db)?;
    store::set_playlist_guides(&tx, provider_id, &summary.guides).map_err(db)?;
    tx.commit().map_err(db)?;
    Ok((ord as usize - lost, summary.skipped))
}

/// The category of a playlist entry with no group.
const UNCATEGORISED: &str = "Uncategorised";

/// An M3U channel's `stream_id` (SPEC 6.4) from the hashes of its key
/// (`tvg-id`, else `tvg-name`, else the title) and URL: the key's alone
/// for the first entry with that key, so a channel keeps its id when a
/// backup is added after it; for a later one, with the URL's and `occ`,
/// its place among the entries that share both, from 0.
fn playlist_id(key: i64, url: i64, first: bool, occ: i64) -> i64 {
    let id = if first {
        key
    } else {
        stable_hash(&[&key.to_le_bytes(), &url.to_le_bytes(), &occ.to_le_bytes()])
    };
    id & i64::MAX
}

/// 64-bit FNV-1a over `parts`, each ended by a 0xff byte (never in UTF-8):
/// the same in every build, unlike std's hasher, as ids are stored.
fn stable_hash(parts: &[&[u8]]) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in parts.iter().flat_map(|p| p.iter().chain(&[0xff])) {
        h ^= u64::from(*byte);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h as i64
}

/// Replaces a provider's movie categories and movies with `body`, a
/// `get_vod_streams` JSON body, in one transaction.
pub fn write_movies(
    conn: &mut Connection,
    provider: (&str, &str),
    categories: &[(String, String)],
    body: impl Read,
    now: i64,
) -> Result<usize, String> {
    write_vod(conn, provider, Kind::Movie, categories, now, |tx, id| {
        each_element(body, ("get_vod_streams", "movie"), |m: Movie| {
            let Some(stream_id) = m.stream_id else {
                return Ok(false);
            };
            let row = NewMovie {
                stream_id,
                num: m.num,
                name: &m.name,
                category_id: m.category_id.as_deref(),
                rating: m.rating,
                poster_url: m.stream_icon.as_deref(),
                extension: m.container_extension.as_deref(),
                added: m.added,
            };
            store::insert_movie(tx, id, &row).map(|()| true)
        })
    })
}

/// Replaces a provider's series categories and series with `body`, a
/// `get_series` JSON body, in one transaction.
pub fn write_series(
    conn: &mut Connection,
    provider: (&str, &str),
    categories: &[(String, String)],
    body: impl Read,
    now: i64,
) -> Result<usize, String> {
    write_vod(conn, provider, Kind::Series, categories, now, |tx, id| {
        each_element(body, ("get_series", "series"), |s: Series| {
            let Some(series_id) = s.series_id else {
                return Ok(false);
            };
            let row = NewSeries {
                series_id,
                num: s.num,
                name: &s.name,
                category_id: s.category_id.as_deref(),
                year: s.year(),
                rating: s.rating,
                cover_url: s.cover.as_deref(),
                genre: s.genre.as_deref(),
                plot: s.plot.as_deref(),
                actors: s.cast.as_deref(),
                director: s.director.as_deref(),
                episode_run_time: s.episode_run_time,
                last_modified: s.last_modified,
            };
            store::insert_series(tx, id, &row).map(|()| true)
        })
    })
}

/// One movie or series import: `insert` streams the titles in between
/// replacing the kind's categories and finishing, all in one transaction.
fn write_vod(
    conn: &mut Connection,
    (provider_id, provider_name): (&str, &str),
    kind: Kind,
    categories: &[(String, String)],
    now: i64,
    insert: impl FnOnce(&Connection, &str) -> Result<usize, ListError>,
) -> Result<usize, String> {
    let what = if kind == Kind::Movie {
        "movies"
    } else {
        "series"
    };
    let db = |e: rusqlite::Error| format!("saving {what}: {e}");
    let tx = conn.transaction().map_err(db)?;
    let stored: bool = tx
        .query_row(
            &format!(
                "SELECT EXISTS (SELECT 1 FROM {} WHERE provider_id = ?1)",
                kind.as_str()
            ),
            [provider_id],
            |r| r.get(0),
        )
        .map_err(db)?;
    store::begin_import(
        &tx,
        provider_id,
        provider_name,
        kind,
        categories
            .iter()
            .map(|(id, name)| (id.as_str(), name.as_str())),
    )
    .map_err(db)?;
    let count = insert(&tx, provider_id).map_err(|e| e.into_message(db))?;
    // Providers send `[]` while their VOD is down: keep what we have
    // (the transaction rolls back on return).
    if count == 0 && stored {
        let noun = if kind == Kind::Movie {
            "movie"
        } else {
            "series"
        };
        return Err(format!(
            "provider sent an empty {noun} list; keeping the stored {what}"
        ));
    }
    store::finish_vod_import(&tx, provider_id, kind, now).map_err(db)?;
    tx.commit().map_err(db)?;
    Ok(count)
}

/// Why a list import stopped.
enum ListError {
    Db(rusqlite::Error),
    /// Already a message: no position quotes the body.
    Body(String),
}

impl ListError {
    fn into_message(self, db: impl Fn(rusqlite::Error) -> String) -> String {
        match self {
            ListError::Db(e) => db(e),
            ListError::Body(e) => e,
        }
    }
}

/// Parses `body`, a JSON list (`action`'s response, of `noun`s), calling
/// `each` on each element as it is parsed, so only the current one is in
/// memory. Returns how many `each` took.
fn each_element<T: DeserializeOwned>(
    body: impl Read,
    (action, noun): (&str, &str),
    each: impl FnMut(T) -> rusqlite::Result<bool>,
) -> Result<usize, ListError> {
    let mut inserter = Inserter {
        each,
        count: 0,
        db_error: None,
        item: PhantomData,
    };
    let mut de = serde_json::Deserializer::from_reader(BufReader::with_capacity(64 * 1024, body));
    let parsed = (&mut inserter).deserialize(&mut de).and_then(|()| de.end());
    if let Some(e) = inserter.db_error {
        return Err(ListError::Db(e));
    }
    if let Err(e) = parsed {
        // Position only: serde's message can quote values from the body.
        return Err(ListError::Body(if e.is_io() {
            format!("{noun} download interrupted: {e}")
        } else {
            format!(
                "invalid {action} response at line {} column {}",
                e.line(),
                e.column()
            )
        }));
    }
    Ok(inserter.count)
}

/// Sequence visitor: hands each element to `each` as it is parsed.
struct Inserter<T, F> {
    each: F,
    count: usize,
    db_error: Option<rusqlite::Error>,
    item: PhantomData<T>,
}

impl<'de, T: DeserializeOwned, F: FnMut(T) -> rusqlite::Result<bool>> DeserializeSeed<'de>
    for &mut Inserter<T, F>
{
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de, T: DeserializeOwned, F: FnMut(T) -> rusqlite::Result<bool>> Visitor<'de>
    for &mut Inserter<T, F>
{
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a list")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(item) = seq.next_element::<T>()? {
            match (self.each)(item) {
                Ok(taken) => self.count += usize::from(taken),
                Err(e) => {
                    self.db_error = Some(e);
                    return Err(serde::de::Error::custom("database error"));
                }
            }
        }
        Ok(())
    }
}

/// Blocking `Read` over the download's chunk channel. The sender closing is
/// end of body; an `Err` chunk is a failed download.
pub struct ChunkReader {
    rx: mpsc::Receiver<Chunk>,
    current: Bytes,
}

impl ChunkReader {
    pub fn new(rx: mpsc::Receiver<Chunk>) -> Self {
        Self {
            rx,
            current: Bytes::new(),
        }
    }
}

impl Read for ChunkReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.current.is_empty() {
            match self.rx.blocking_recv() {
                Some(Ok(chunk)) => self.current = chunk,
                Some(Err(e)) => return Err(io::Error::other(e)),
                None => return Ok(0),
            }
        }
        let n = buf.len().min(self.current.len());
        buf[..n].copy_from_slice(&self.current[..n]);
        self.current.advance(n);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_xtream::xmltv_stamp;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    /// Tracks live heap bytes per thread, so a test can measure its own peak
    /// while other tests run in parallel.
    struct Counting;

    thread_local! {
        static LIVE: Cell<isize> = const { Cell::new(0) };
        static PEAK: Cell<isize> = const { Cell::new(0) };
    }

    fn track(delta: isize) {
        let _ = LIVE.try_with(|l| {
            let v = l.get() + delta;
            l.set(v);
            let _ = PEAK.try_with(|p| p.set(p.get().max(v)));
        });
    }

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            track(l.size() as isize);
            unsafe { System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            track(-(l.size() as isize));
            unsafe { System.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
            track(new as isize - l.size() as isize);
            unsafe { System.realloc(p, l, new) }
        }
    }

    #[global_allocator]
    static ALLOC: Counting = Counting;

    /// Peak heap bytes this thread allocated while running `f`, above where it started.
    fn peak_during<T>(f: impl FnOnce() -> T) -> (T, usize) {
        let base = LIVE.with(Cell::get);
        PEAK.with(|p| p.set(base));
        let out = f();
        (out, (PEAK.with(Cell::get) - base) as usize)
    }

    /// Generates a `get_live_streams` body on the fly, so the fixture itself
    /// never sits in memory. Mixes the field types real providers send.
    struct Fixture {
        next: usize,
        total: usize,
        buf: Vec<u8>,
        pos: usize,
    }

    impl Fixture {
        fn new(total: usize) -> Self {
            Self {
                next: 0,
                total,
                buf: b"[".to_vec(),
                pos: 0,
            }
        }
    }

    impl Read for Fixture {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.pos == self.buf.len() {
                self.buf.clear();
                self.pos = 0;
                let i = self.next;
                if i > self.total {
                    return Ok(0);
                }
                self.next += 1;
                if i == self.total {
                    self.buf.extend_from_slice(b"]");
                } else {
                    let sep = if i == 0 { "" } else { "," };
                    let (num, epg) = if i.is_multiple_of(2) {
                        (format!("{}", i + 1), format!("\"ch{i}.uk\""))
                    } else {
                        (format!("\"{}\"", i + 1), "null".into())
                    };
                    self.buf.extend_from_slice(
                        format!(
                            r#"{sep}{{"num":{num},"name":"UK: Channel {i} ᴴᴰ","stream_type":"live","stream_id":{},"stream_icon":"http://img/{i}.png","epg_channel_id":{epg},"added":"1700000000","category_id":"{}","custom_sid":"","tv_archive":0,"direct_source":"","tv_archive_duration":0}}"#,
                            100_000 + i,
                            i % 40
                        )
                        .as_bytes(),
                    );
                }
            }
            let n = out.len().min(self.buf.len() - self.pos);
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    fn cats() -> Vec<(String, String)> {
        (0..40)
            .map(|i| (i.to_string(), format!("Category {i}")))
            .collect()
    }

    fn db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = store::open(&dir.path().join("v.db")).unwrap();
        (dir, conn)
    }

    fn count(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT count(*) FROM channel WHERE provider_id = 'p'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn last_refresh(conn: &Connection) -> Option<i64> {
        conn.query_row(
            "SELECT last_channels_refresh FROM provider WHERE id = 'p'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn import(conn: &mut Connection, body: impl Read) -> Result<usize, String> {
        write_channels(conn, ("p", "Provider"), &cats(), body, "ts", 1_700_000_000)
    }

    /// A `get_live_streams` body of `(stream_id, name)`s.
    fn streams(list: &[(i64, &str)]) -> String {
        let items: Vec<String> = list
            .iter()
            .map(|(id, name)| {
                format!(r#"{{"stream_id":{id},"num":{id},"name":"{name}","category_id":"1"}}"#)
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    /// Favourites in order: `(stream_id, available)`, via the Favourites view.
    fn favourites(conn: &Connection) -> Vec<(i64, bool)> {
        store::channel_keys(conn, "p", store::View::Favourites)
            .unwrap()
            .into_iter()
            .map(|k| {
                let row = store::channel(conn, k, NOW).unwrap().unwrap();
                assert!(row.favourite);
                (row.stream_id, row.available)
            })
            .collect()
    }

    #[test]
    fn favourites_follow_a_changed_id_by_name() {
        let (_dir, mut conn) = db();
        let before = streams(&[
            (1, "UK: Harbour One HD"),
            (2, "UK: Kestrel"),
            (3, "UK: Juniper"),
            (4, "UK: Cobalt Films"),
        ]);
        import(&mut conn, before.as_bytes()).unwrap();
        for id in [1, 2, 3, 4] {
            assert!(store::toggle_favourite(&conn, "p", store::Kind::Live, id).unwrap());
        }

        // New ids for all four. Harbour One: one match (quality tag differs,
        // same name_norm). Kestrel: two matches. Juniper: none. Cobalt Films:
        // one match, but
        // under an id that is also changing.
        let after = streams(&[
            (11, "UK: Harbour One ᴴᴰ"),
            (12, "UK: Kestrel"),
            (13, "UK: Kestrel HD"),
            (14, "UK: Cobalt Films"),
        ]);
        import(&mut conn, after.as_bytes()).unwrap();
        assert_eq!(
            favourites(&conn),
            [(11, true), (2, false), (3, false), (14, true)],
            "order kept; ambiguous and missing ones kept but unavailable"
        );
        // Unavailable favourites keep their last display name.
        let keys = store::channel_keys(&conn, "p", store::View::Favourites).unwrap();
        assert!(keys[1] < 0);
        assert_eq!(
            store::channel(&conn, keys[1], NOW).unwrap().unwrap().name,
            "UK: Kestrel"
        );

        // Juniper comes back under a new id: re-pointed on the next import.
        let back = streams(&[
            (11, "UK: Harbour One ᴴᴰ"),
            (15, "UK: Juniper"),
            (14, "UK: Cobalt Films"),
        ]);
        import(&mut conn, back.as_bytes()).unwrap();
        assert_eq!(
            favourites(&conn),
            [(11, true), (2, false), (15, true), (14, true)]
        );
    }

    #[test]
    fn repointing_onto_an_existing_favourite_drops_the_duplicate() {
        let (_dir, mut conn) = db();
        import(
            &mut conn,
            streams(&[(1, "UK: Kestrel"), (2, "UK: Kestrel HD")]).as_bytes(),
        )
        .unwrap();
        store::toggle_favourite(&conn, "p", store::Kind::Live, 1).unwrap();
        store::toggle_favourite(&conn, "p", store::Kind::Live, 2).unwrap();
        // Both names now fold to one channel.
        import(&mut conn, streams(&[(2, "UK: Kestrel HD")]).as_bytes()).unwrap();
        assert_eq!(favourites(&conn), [(2, true)]);
    }

    #[test]
    fn favourite_order_persists_and_moves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.db");
        {
            let mut conn = store::open(&path).unwrap();
            let body = streams(&[(1, "A"), (2, "B"), (3, "C")]);
            import(&mut conn, body.as_bytes()).unwrap();
            for id in [3, 1, 2] {
                store::toggle_favourite(&conn, "p", store::Kind::Live, id).unwrap();
            }
            assert!(store::move_favourite(&mut conn, "p", store::Kind::Live, 2, -1).unwrap());
            assert!(
                !store::move_favourite(&mut conn, "p", store::Kind::Live, 3, -1).unwrap(),
                "already first"
            );
            assert!(store::move_favourite(&mut conn, "p", store::Kind::Live, 3, 1).unwrap());
            assert!(
                !store::move_favourite(&mut conn, "p", store::Kind::Live, 1, 1).unwrap(),
                "already last"
            );
        }
        let mut conn = store::open(&path).unwrap();
        assert_eq!(favourites(&conn), [(2, true), (3, true), (1, true)]);
        // Toggling off and on again puts it at the end.
        assert!(!store::toggle_favourite(&conn, "p", store::Kind::Live, 2).unwrap());
        assert!(store::toggle_favourite(&conn, "p", store::Kind::Live, 2).unwrap());
        assert_eq!(favourites(&conn), [(3, true), (1, true), (2, true)]);
        // A re-import keeps the order.
        import(
            &mut conn,
            streams(&[(1, "A"), (2, "B"), (3, "C")]).as_bytes(),
        )
        .unwrap();
        assert_eq!(favourites(&conn), [(3, true), (1, true), (2, true)]);
        // Unknown channels cannot be added.
        assert!(!store::toggle_favourite(&conn, "p", store::Kind::Live, 99).unwrap());
    }

    #[test]
    fn imports_all_fields() {
        let (_dir, mut conn) = db();
        assert_eq!(import(&mut conn, Fixture::new(3)), Ok(3));
        let row: (i64, i64, String, String, Option<String>, String, String) = conn
            .query_row(
                "SELECT stream_id, num, name, name_norm, epg_channel_id, icon_url, extension FROM channel WHERE num = 2",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (
                100_001,
                2,
                "UK: Channel 1 ᴴᴰ".into(),
                "uk: channel 1".into(),
                None,
                "http://img/1.png".into(),
                "ts".into()
            )
        );
        let cats: i64 = conn
            .query_row("SELECT count(*) FROM category", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cats, 40);
        assert_eq!(last_refresh(&conn), Some(1_700_000_000));
        let fts: i64 = conn
            .query_row(
                "SELECT count(*) FROM channel_fts WHERE channel_fts MATCH 'channel'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fts, 3);
    }

    #[test]
    fn reimport_replaces_previous_channels() {
        let (_dir, mut conn) = db();
        import(&mut conn, Fixture::new(10)).unwrap();
        import(&mut conn, Fixture::new(4)).unwrap();
        assert_eq!(count(&conn), 4);
        let fts: i64 = conn
            .query_row("SELECT count(*) FROM channel_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fts, 4);
    }

    #[test]
    fn bad_response_rolls_back() {
        let (_dir, mut conn) = db();
        import(&mut conn, Fixture::new(5)).unwrap();

        let mut full = Vec::new();
        Fixture::new(5_000).read_to_end(&mut full).unwrap();
        let truncated = &full[..full.len() / 2];
        let bodies: [&[u8]; 4] = [
            truncated,
            b"{\"user_info\":{\"auth\":0}}",
            b"[{\"stream_id\":1,}]",
            b"",
        ];
        for body in bodies {
            let err = import(&mut conn, body).unwrap_err();
            assert!(err.contains("invalid get_live_streams"), "{err}");
            assert_eq!(count(&conn), 5);
            assert_eq!(last_refresh(&conn), Some(1_700_000_000));
        }
    }

    #[test]
    fn failed_download_rolls_back() {
        let (_dir, mut conn) = db();
        import(&mut conn, Fixture::new(5)).unwrap();
        let (tx, rx) = mpsc::channel(4);
        tx.blocking_send(Ok(Bytes::from_static(
            b"[{\"stream_id\":1,\"name\":\"a\"},",
        )))
        .unwrap();
        tx.blocking_send(Err("connection reset".into())).unwrap();
        drop(tx);
        let err = import(&mut conn, ChunkReader::new(rx)).unwrap_err();
        assert!(
            err.contains("interrupted") && err.contains("connection reset"),
            "{err}"
        );
        assert_eq!(count(&conn), 5);
    }

    /// The UI's read connection keeps seeing the old channels mid-import, and
    /// the new ones after commit.
    #[test]
    fn reader_sees_snapshot_during_import() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.db");
        let mut writer = store::open(&path).unwrap();
        import(&mut writer, Fixture::new(5)).unwrap();
        let reader = store::open(&path).unwrap();

        let mut body = Vec::new();
        Fixture::new(2_000).read_to_end(&mut body).unwrap();
        let (tx, rx) = mpsc::channel(1);
        let job = std::thread::spawn(move || import(&mut writer, ChunkReader::new(rx)));
        let (first, rest) = body.split_at(body.len() / 2);
        tx.blocking_send(Ok(Bytes::copy_from_slice(first))).unwrap();
        // With capacity 1, this returns once the writer has taken the first
        // half, i.e. it is inside its transaction and inserting.
        tx.blocking_send(Ok(Bytes::from_static(b" "))).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(count(&reader), 5);

        tx.blocking_send(Ok(Bytes::copy_from_slice(rest))).unwrap();
        drop(tx);
        assert_eq!(job.join().unwrap(), Ok(2_000));
        assert_eq!(count(&reader), 2_000);
    }

    /// Peak heap during an import must not grow with the list. SQLite's own
    /// page cache is outside this count and capped by `cache_size`.
    #[test]
    fn peak_memory_does_not_scale_with_list_size() {
        let (_dir, mut conn) = db();
        // Warm up statement caches so both runs start equal.
        import(&mut conn, Fixture::new(10)).unwrap();
        let (n, small) = peak_during(|| import(&mut conn, Fixture::new(5_000)).unwrap());
        assert_eq!(n, 5_000);
        let (n, large) = peak_during(|| import(&mut conn, Fixture::new(50_000)).unwrap());
        assert_eq!(n, 50_000);
        eprintln!("peak heap during import: 5k channels {small} B, 50k channels {large} B");
        assert!(large < small + 64 * 1024, "5k: {small} B, 50k: {large} B");
    }

    /// A `get_vod_streams` body made on the fly, as [`Fixture`] is, with
    /// the fields the user's provider sends (SPEC 6.1): numbers as strings
    /// on odd ones, and empty or missing ones on every third.
    struct Movies {
        next: usize,
        total: usize,
        buf: Vec<u8>,
        pos: usize,
    }

    impl Movies {
        fn new(total: usize) -> Self {
            Self {
                next: 0,
                total,
                buf: b"[".to_vec(),
                pos: 0,
            }
        }
    }

    impl Read for Movies {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.pos == self.buf.len() {
                self.buf.clear();
                self.pos = 0;
                let i = self.next;
                if i > self.total {
                    return Ok(0);
                }
                self.next += 1;
                if i == self.total {
                    self.buf.extend_from_slice(b"]");
                } else {
                    let sep = if i == 0 { "" } else { "," };
                    let id = 20_000 + i;
                    let (id, num) = if i.is_multiple_of(2) {
                        (format!("{id}"), format!("{}", i + 1))
                    } else {
                        (format!("\"{id}\""), format!("\"{}\"", i + 1))
                    };
                    let extra = if i.is_multiple_of(3) {
                        r#""rating":"","stream_icon":"""#.to_string()
                    } else {
                        format!(
                            r#""rating":"7.{}","rating_5based":3.5,"stream_icon":"http://img/m{i}.jpg","added":"1700000000","container_extension":"mkv","tmdb":"","trailer":"","is_adult":0,"custom_sid":"","direct_source":"""#,
                            i % 10
                        )
                    };
                    self.buf.extend_from_slice(
                        format!(
                            r#"{sep}{{"num":{num},"name":"The Quiet Year {i}","stream_type":"movie","stream_id":{id},"category_id":"{}","category_ids":[{}],{extra}}}"#,
                            i % 40,
                            i % 40
                        )
                        .as_bytes(),
                    );
                }
            }
            let n = out.len().min(self.buf.len() - self.pos);
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    fn import_movies(conn: &mut Connection, body: impl Read) -> Result<usize, String> {
        write_movies(conn, ("p", "Provider"), &cats(), body, NOW)
    }

    fn titles(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn imports_movie_fields_as_numbers_strings_or_missing() {
        let (_dir, mut conn) = db();
        assert_eq!(import_movies(&mut conn, Movies::new(3)), Ok(3));
        type Row = (
            i64,
            Option<i64>,
            String,
            Option<String>,
            Option<f64>,
            Option<String>,
            Option<String>,
            Option<i64>,
        );
        let rows: Vec<Row> = conn
            .prepare(
                "SELECT stream_id, num, name, category_id, rating, poster_url, extension, added
                 FROM movie ORDER BY stream_id",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            [
                // Empty rating and poster, the rest missing.
                (
                    20_000,
                    Some(1),
                    "The Quiet Year 0".into(),
                    Some("0".into()),
                    None,
                    None,
                    None,
                    None
                ),
                // Ids as strings.
                (
                    20_001,
                    Some(2),
                    "The Quiet Year 1".into(),
                    Some("1".into()),
                    Some(7.1),
                    Some("http://img/m1.jpg".into()),
                    Some("mkv".into()),
                    Some(1_700_000_000)
                ),
                (
                    20_002,
                    Some(3),
                    "The Quiet Year 2".into(),
                    Some("2".into()),
                    Some(7.2),
                    Some("http://img/m2.jpg".into()),
                    Some("mkv".into()),
                    Some(1_700_000_000)
                ),
            ]
        );
        let (cats, refreshed): (i64, Option<i64>) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM category WHERE kind = 'movie'), last_movies_refresh
                 FROM provider",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((cats, refreshed), (40, Some(NOW)));
        // Nothing of live's is touched.
        assert_eq!(titles(&conn, "category WHERE kind = 'live'"), 0);
        assert_eq!(last_refresh(&conn), None);
    }

    #[test]
    fn imports_series_fields_as_numbers_strings_or_missing() {
        let (_dir, mut conn) = db();
        let body = r#"[
            {"num":1,"name":"Harbour Watch","series_id":10,"cover":"http://img/s10.jpg","plot":"A port town.","cast":"A, B","director":"C","genre":"Crime","releaseDate":"2021-03-04","release_date":"2021-03-04","last_modified":"1700000000","rating":"8","rating_5based":"4","backdrop_path":[],"youtube_trailer":"","episode_run_time":"45","tmdb":"12","category_id":"7","category_ids":[7]},
            {"num":"2","name":"Coastal Lives","series_id":"11","release_date":"","rating":8.5,"episode_run_time":30,"last_modified":1700000001,"backdrop_path":null,"category_id":7},
            {"name":"Untitled","series_id":12,"releaseDate":"1999"},
            {"name":"No id"}
        ]"#;
        let n = write_series(&mut conn, ("p", "Provider"), &cats(), body.as_bytes(), NOW);
        assert_eq!(n, Ok(3));
        type Row = (
            i64,
            Option<i64>,
            String,
            Option<String>,
            Option<i64>,
            Option<f64>,
            Option<String>,
            Option<String>,
        );
        let rows: Vec<Row> = conn
            .prepare(
                "SELECT series_id, num, name, category_id, year, rating, cover_url, genre
                 FROM series ORDER BY series_id",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            [
                (
                    10,
                    Some(1),
                    "Harbour Watch".into(),
                    Some("7".into()),
                    Some(2021),
                    Some(8.0),
                    Some("http://img/s10.jpg".into()),
                    Some("Crime".into())
                ),
                (
                    11,
                    Some(2),
                    "Coastal Lives".into(),
                    Some("7".into()),
                    None,
                    Some(8.5),
                    None,
                    None
                ),
                (
                    12,
                    None,
                    "Untitled".into(),
                    None,
                    Some(1999),
                    None,
                    None,
                    None
                ),
            ]
        );
        type Rest = (String, String, String, i64, i64);
        let rest: Rest = conn
            .query_row(
                "SELECT plot, actors, director, episode_run_time, last_modified FROM series
                 WHERE series_id = 10",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            rest,
            (
                "A port town.".into(),
                "A, B".into(),
                "C".into(),
                45,
                1_700_000_000
            )
        );
        let run_time: Option<i64> = conn
            .query_row(
                "SELECT episode_run_time FROM series WHERE series_id = 11",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(run_time, Some(30));
    }

    #[test]
    fn cut_movie_or_series_body_rolls_back() {
        let (_dir, mut conn) = db();
        import_movies(&mut conn, Movies::new(5)).unwrap();
        let series = r#"[{"series_id":1,"name":"Harbour Watch","category_id":"1"}]"#;
        write_series(
            &mut conn,
            ("p", "Provider"),
            &cats(),
            series.as_bytes(),
            NOW,
        )
        .unwrap();

        let mut full = Vec::new();
        Movies::new(5_000).read_to_end(&mut full).unwrap();
        let err = import_movies(&mut conn, &full[..full.len() / 2]).unwrap_err();
        assert!(err.contains("invalid get_vod_streams"), "{err}");
        let err = write_series(
            &mut conn,
            ("p", "Provider"),
            &[],
            &series.as_bytes()[..30],
            NOW + 1,
        )
        .unwrap_err();
        assert!(err.contains("invalid get_series"), "{err}");

        let (tx, rx) = mpsc::channel(4);
        tx.blocking_send(Ok(Bytes::copy_from_slice(&full[..4096])))
            .unwrap();
        tx.blocking_send(Err("connection reset".into())).unwrap();
        drop(tx);
        let err = import_movies(&mut conn, ChunkReader::new(rx)).unwrap_err();
        assert!(
            err.contains("movie download interrupted") && err.contains("connection reset"),
            "{err}"
        );

        assert_eq!(titles(&conn, "movie"), 5);
        assert_eq!(titles(&conn, "series"), 1);
        assert_eq!(titles(&conn, "category WHERE kind = 'series'"), 40);
        let last: (Option<i64>, Option<i64>) = store::last_vod_refreshes(&conn, "p").unwrap();
        assert_eq!(last, (Some(NOW), Some(NOW)));
    }

    #[test]
    fn empty_movie_or_series_list_keeps_a_stored_library() {
        let (_dir, mut conn) = db();
        let p = ("p", "Provider");
        assert_eq!(write_series(&mut conn, p, &[], &b"[]"[..], NOW), Ok(0));
        import_movies(&mut conn, Movies::new(5)).unwrap();
        let series = r#"[{"series_id":1,"name":"Harbour Watch","category_id":"1"}]"#;
        write_series(&mut conn, p, &cats(), series.as_bytes(), NOW).unwrap();

        let err = write_movies(&mut conn, p, &[], &b"[]"[..], NOW + 1).unwrap_err();
        assert!(err.contains("empty movie list"), "{err}");
        let err = write_series(&mut conn, p, &[], &b"[]"[..], NOW + 1).unwrap_err();
        assert!(err.contains("empty series list"), "{err}");

        assert_eq!(titles(&conn, "movie"), 5);
        assert_eq!(titles(&conn, "series"), 1);
        assert_eq!(titles(&conn, "category WHERE kind = 'series'"), 40);
        let last: (Option<i64>, Option<i64>) = store::last_vod_refreshes(&conn, "p").unwrap();
        assert_eq!(last, (Some(NOW), Some(NOW)));
    }

    /// Peak heap during a movie import must not grow with the list (7.1).
    #[test]
    fn movie_import_peak_memory_does_not_scale_with_list_size() {
        let (_dir, mut conn) = db();
        import_movies(&mut conn, Movies::new(10)).unwrap();
        let (n, small) = peak_during(|| import_movies(&mut conn, Movies::new(5_000)).unwrap());
        assert_eq!(n, 5_000);
        let (n, large) = peak_during(|| import_movies(&mut conn, Movies::new(50_000)).unwrap());
        assert_eq!(n, 50_000);
        eprintln!("peak heap during import: 5k movies {small} B, 50k movies {large} B");
        assert!(large < small + 64 * 1024, "5k: {small} B, 50k: {large} B");
    }

    const NOW: i64 = 1_700_000_000;

    /// Generates an XMLTV document on the fly: `channels` channels, each with
    /// hour-long programmes from two days before [`NOW`] to five days after,
    /// with times in +01:00.
    struct Xmltv {
        channels: usize,
        next: usize,
        buf: Vec<u8>,
        pos: usize,
    }

    const PER_CHANNEL: usize = 7 * 24;

    fn slot_start(k: usize) -> i64 {
        NOW - NOW % 3600 - 2 * 86_400 + k as i64 * 3600
    }

    impl Xmltv {
        fn new(channels: usize) -> Self {
            Self {
                channels,
                next: 0,
                buf: br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE tv SYSTEM "xmltv.dtd">
<tv generator-info-name="fixture">
"#
                .to_vec(),
                pos: 0,
            }
        }

        /// Programmes a correct import keeps.
        fn expected(&self) -> usize {
            let kept = (0..PER_CHANNEL)
                .filter(|&k| slot_start(k) + 3600 > NOW - 3600 && slot_start(k) < NOW + 3 * 86_400)
                .count();
            kept * self.channels
        }
    }

    impl Read for Xmltv {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.pos == self.buf.len() {
                self.buf.clear();
                self.pos = 0;
                let i = self.next;
                let (channels, programmes) = (self.channels, self.channels * PER_CHANNEL);
                if i > channels + programmes {
                    return Ok(0);
                }
                self.next += 1;
                let text = if i < channels {
                    format!(
                        "<channel id=\"ch{i}.uk\"><display-name>Channel {i}</display-name></channel>\n"
                    )
                } else if i < channels + programmes {
                    let (c, k) = ((i - channels) % channels, (i - channels) / channels);
                    let start = slot_start(k);
                    format!(
                        "<programme start=\"{}\" stop=\"{}\" channel=\"ch{c}.uk\"><title lang=\"en\">Show {c}/{k} &amp; friends</title><desc lang=\"en\">Episode {k}.</desc><category>News</category></programme>\n",
                        xmltv_stamp(start, 60),
                        xmltv_stamp(start + 3600, 60),
                    )
                } else {
                    "</tv>\n".into()
                };
                self.buf.extend_from_slice(text.as_bytes());
            }
            let n = out.len().min(self.buf.len() - self.pos);
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    fn epg_import(conn: &mut Connection, body: impl Read) -> Result<usize, String> {
        write_epg(conn, "p", body, 0, NOW)
    }

    fn programmes(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT count(*) FROM programme WHERE provider_id = 'p'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn last_epg(conn: &Connection) -> Option<i64> {
        store::last_refreshes(conn, "p").unwrap().1
    }

    /// A database with channels ch0.uk .. ch4.uk (stream ids 100000 up),
    /// even ones only, as `Fixture` gives odd ones no EPG id.
    fn db_with_channels() -> (tempfile::TempDir, Connection) {
        let (dir, mut conn) = db();
        import(&mut conn, Fixture::new(5)).unwrap();
        (dir, conn)
    }

    #[test]
    fn xmltv_times_convert_to_utc() {
        assert_eq!(xmltv_time("20231114221320 +0000"), Some(NOW));
        assert_eq!(xmltv_time("20231114231320 +0100"), Some(NOW));
        assert_eq!(xmltv_time("20231114171320 -0500"), Some(NOW));
        assert_eq!(xmltv_time("20231114231320+0100"), Some(NOW));
        assert_eq!(xmltv_time("20231114221320"), Some(NOW));
        assert_eq!(xmltv_time("202311142213 Z"), Some(NOW - 20));
        assert_eq!(xmltv_time("20240229000000 +0000"), Some(1_709_164_800));
        for bad in [
            "",
            "2023111422",
            "20231314221320 +0000",
            "20231114221320 +01",
            "2023111422132x",
        ] {
            assert_eq!(xmltv_time(bad), None, "{bad}");
        }
        for t in [NOW, 0, 951_782_400, 4_102_444_800] {
            assert_eq!(xmltv_time(&xmltv_stamp(t, -330)), Some(t));
        }
    }

    #[test]
    fn imports_programme_fields() {
        let (_dir, mut conn) = db_with_channels();
        let doc = format!(
            r#"<?xml version="1.0"?><tv>
<programme start="{s}" stop="{e}" channel="ch0.uk">
  <title lang="en">Fish &amp; Chips &#x2764;</title><title lang="fr">Poisson</title>
  <desc><![CDATA[<b>Raw</b> text]]></desc>
</programme>
<programme start="{s}" channel="ch2.uk"><title>No stop</title></programme>
<programme start="{s}" stop="{e}" channel="ch2.uk"><desc>No title</desc></programme>
<programme start="{s}" stop="{e}" channel="ch2.uk"/>
<programme start="garbage" stop="{e}" channel="ch2.uk"><title>Bad time</title></programme>
</tv>"#,
            s = xmltv_stamp(NOW - 600, 0),
            e = xmltv_stamp(NOW + 1200, 0),
        );
        assert_eq!(epg_import(&mut conn, doc.as_bytes()), Ok(1));
        let row: (String, i64, i64, String, Option<String>) = conn
            .query_row(
                "SELECT epg_channel_id, start, stop, title, description FROM programme",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (
                "ch0.uk".into(),
                NOW - 600,
                NOW + 1200,
                "Fish & Chips ❤".into(),
                Some("<b>Raw</b> text".into())
            )
        );
        assert_eq!(last_epg(&conn), Some(NOW));
    }

    #[test]
    fn large_epg_import_keeps_only_the_window() {
        let (_dir, mut conn) = db();
        let fixture = Xmltv::new(200);
        let expected = fixture.expected();
        assert!(expected > 0 && expected < 200 * PER_CHANNEL);
        assert_eq!(epg_import(&mut conn, fixture), Ok(expected));
        assert_eq!(programmes(&conn), expected as i64);
        let (min_stop, max_start): (i64, i64) = conn
            .query_row("SELECT min(stop), max(start) FROM programme", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert!(min_stop > NOW - 3600, "{min_stop}");
        assert!(max_start < NOW + 3 * 86_400, "{max_start}");
    }

    /// Channels `(stream_id, guide id, archive days)`, as `get_live_streams`
    /// sends them.
    fn archive_channels(list: &[(i64, &str, i64)]) -> String {
        let items: Vec<String> = list
            .iter()
            .map(|(id, epg, days)| {
                format!(
                    r#"{{"stream_id":{id},"name":"Channel {id}","category_id":"1","epg_channel_id":"{epg}","tv_archive":{},"tv_archive_duration":"{days}"}}"#,
                    i64::from(*days > 0)
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    #[test]
    fn imported_channels_keep_their_archive_days() {
        let (_dir, mut conn) = db();
        let body = archive_channels(&[(1, "a", 3), (2, "b", 0)]);
        import(&mut conn, body.as_bytes()).unwrap();
        let days: Vec<i64> = conn
            .prepare("SELECT archive_days FROM channel ORDER BY stream_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(days, [3, 0]);
    }

    /// Each guide id keeps past programmes as far back as its longest
    /// archive, or 1 h without one.
    #[test]
    fn past_programmes_are_kept_per_archive() {
        let (_dir, mut conn) = db();
        // ch0: 3 days. ch1: none. ch2: shared by a 1-day and a 7-day
        // channel. ch3: 2 days. ch4 has no channel at all.
        let body = archive_channels(&[
            (1, "ch0.uk", 3),
            (2, "ch1.uk", 0),
            (3, "ch2.uk", 1),
            (4, "ch2.uk", 7),
            (5, "ch3.uk", 2),
        ]);
        import(&mut conn, body.as_bytes()).unwrap();
        epg_import(&mut conn, Xmltv::new(5)).unwrap();

        // The fixture reaches 2 days back.
        let oldest = |conn: &Connection, guide: &str| -> i64 {
            conn.query_row(
                "SELECT min(stop) FROM programme WHERE epg_channel_id = ?1",
                [guide],
                |r| r.get(0),
            )
            .unwrap()
        };
        let first_stop = slot_start(0) + 3600;
        assert_eq!(oldest(&conn, "ch0.uk"), first_stop, "3 days: all of it");
        assert_eq!(
            oldest(&conn, "ch2.uk"),
            first_stop,
            "the longer of 1 and 7 days"
        );
        assert_eq!(oldest(&conn, "ch3.uk"), first_stop, "2 days, to the hour");
        for guide in ["ch1.uk", "ch4.uk"] {
            let stop = oldest(&conn, guide);
            assert!(stop > NOW - 3600 && stop <= NOW, "{guide}: {stop}");
        }

        // A 1-day archive alone: from a day back.
        import(&mut conn, archive_channels(&[(3, "ch2.uk", 1)]).as_bytes()).unwrap();
        epg_import(&mut conn, Xmltv::new(5)).unwrap();
        let stop = oldest(&conn, "ch2.uk");
        assert!(stop > NOW - 86_400 && stop <= NOW - 86_400 + 3600, "{stop}");
        // Nothing reaches further ahead for having an archive.
        let latest: i64 = conn
            .query_row("SELECT max(start) FROM programme", [], |r| r.get(0))
            .unwrap();
        assert!(latest < NOW + 3 * 86_400, "{latest}");
    }

    /// An XMLTV document of `(guide id, start, stop)`s.
    fn guide_doc(progs: &[(&str, i64, i64)]) -> String {
        let items: String = progs
            .iter()
            .map(|(id, a, b)| {
                format!(
                    r#"<programme start="{}" stop="{}" channel="{id}"><title>Guide</title></programme>"#,
                    xmltv_stamp(*a, 0),
                    xmltv_stamp(*b, 0)
                )
            })
            .collect();
        format!("<tv>{items}</tv>")
    }

    /// Older days from a channel's table (catch-up ticket 06) go before
    /// the guide's first programme, and a later import keeps them until
    /// they leave the archive.
    #[test]
    fn older_programmes_from_a_table_outlive_imports_within_the_archive() {
        let (_dir, mut conn) = db();
        import(
            &mut conn,
            archive_channels(&[(1, "a", 3), (2, "b", 0)]).as_bytes(),
        )
        .unwrap();
        let (h, day) = (3600, 86_400);
        let t0 = NOW - NOW % h;
        let guide = |now: i64| guide_doc(&[("a", now - day, now + day), ("b", now - h, now + day)]);
        write_epg(&mut conn, "p", guide(t0).as_bytes(), 0, t0).unwrap();

        // Hourly from 4 days back to now, flagged within 3 days.
        let table: Vec<store::TableProgramme> = (0..4 * 24)
            .map(|k| {
                let start = t0 - 4 * day + k * h;
                store::TableProgramme {
                    start,
                    stop: start + h,
                    title: format!("Table {k}"),
                    description: None,
                    archive: Some(start >= t0 - 3 * day),
                }
            })
            .collect();
        let added = store::add_table(&mut conn, "p", "a", 3, &table, t0).unwrap();
        // Before the guide's start, a day back, and within 3 days.
        assert_eq!(added, 48);
        let rows = |conn: &Connection, id: &str| -> Vec<(i64, i64, Option<bool>)> {
            conn.prepare(
                "SELECT start, stop, archive FROM programme WHERE epg_channel_id = ?1 ORDER BY start",
            )
            .unwrap()
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
        };
        let a = rows(&conn, "a");
        assert_eq!(a.len(), 49);
        assert_eq!(a[0].1, t0 - 3 * day + h, "the first ends within 3 days");
        assert_eq!(a[47].1, t0 - day, "the last ends where the guide starts");
        // The guide's own programme takes only the table's flag.
        assert_eq!(a[48], (t0 - day, t0 + day, Some(true)));

        // Half a day on, the guide reaches half a day less far back: the
        // table's programmes stay where it no longer reaches, and those
        // that left the archive go. Without an archive nothing stays.
        let later = t0 + 12 * h;
        write_epg(&mut conn, "p", guide(later).as_bytes(), 0, later).unwrap();
        let a = rows(&conn, "a");
        assert_eq!(a.len(), 36 + 1);
        assert_eq!(a[0].1, later - 3 * day + h);
        assert_eq!(a[35].1, t0 - day);
        assert_eq!(a[35].2, Some(true), "flags are kept");
        assert_eq!(a[36].0, later - day);
        assert_eq!(rows(&conn, "b").len(), 1);

        // A guide reaching back over them replaces them.
        let over = guide_doc(&[("a", later - 2 * day, later + day)]);
        write_epg(&mut conn, "p", over.as_bytes(), 0, later).unwrap();
        let a = rows(&conn, "a");
        assert_eq!(a.len(), 24 + 1, "half a day left in the archive before it");
        assert_eq!(a[24].0, later - 2 * day);

        // The channel loses its archive: its older days go.
        import(&mut conn, archive_channels(&[(1, "a", 0)]).as_bytes()).unwrap();
        write_epg(&mut conn, "p", guide(later).as_bytes(), 0, later).unwrap();
        assert_eq!(rows(&conn, "a").len(), 1);
    }

    #[test]
    fn gzipped_epg_is_detected() {
        use flate2::{Compression, read::GzEncoder};
        let (_dir, mut conn) = db();
        let fixture = Xmltv::new(3);
        let expected = fixture.expected();
        assert_eq!(
            epg_import(&mut conn, GzEncoder::new(fixture, Compression::fast())),
            Ok(expected)
        );
    }

    #[test]
    fn xz_epg_is_detected() {
        let (_dir, mut conn) = db();
        let mut fixture = Xmltv::new(3);
        let expected = fixture.expected();
        let mut full = Vec::new();
        fixture.read_to_end(&mut full).unwrap();
        assert_eq!(epg_import(&mut conn, &xz(&full)[..]), Ok(expected));
        // Its magic split over the first chunks, as a download may be.
        let (tx, rx) = mpsc::channel(1024);
        for chunk in xz(&full).chunks(3) {
            tx.blocking_send(Ok(Bytes::copy_from_slice(chunk))).unwrap();
        }
        drop(tx);
        assert_eq!(epg_import(&mut conn, ChunkReader::new(rx)), Ok(expected));
    }

    #[test]
    fn epg_reimport_replaces_previous_programmes() {
        let (_dir, mut conn) = db();
        epg_import(&mut conn, Xmltv::new(10)).unwrap();
        let fixture = Xmltv::new(2);
        let expected = fixture.expected();
        epg_import(&mut conn, fixture).unwrap();
        assert_eq!(programmes(&conn), expected as i64);
    }

    #[test]
    fn failed_epg_import_rolls_back() {
        let (_dir, mut conn) = db_with_channels();
        let first = Xmltv::new(3);
        let kept = first.expected() as i64;
        epg_import(&mut conn, first).unwrap();

        let mut full = Vec::new();
        Xmltv::new(50).read_to_end(&mut full).unwrap();
        let mut gz = Vec::new();
        flate2::read::GzEncoder::new(&full[..], flate2::Compression::fast())
            .read_to_end(&mut gz)
            .unwrap();
        let xz = xz(&full);
        let cases: [(&[u8], &str); 7] = [
            // Cut mid-tag or mid-text, depending on where half falls.
            (&full[..full.len() / 2], "invalid XMLTV|incomplete"),
            // Cut exactly after a programme: well-formed so far, but no `</tv>`.
            (&full[..full.len() - "</tv>\n".len()], "incomplete"),
            (&gz[..gz.len() / 2], "interrupted"),
            // xz cut short, or cut just before its last bytes, the index
            // and footer, where the XML may be whole.
            (&xz[..xz.len() / 2], "interrupted"),
            (&xz[..xz.len() - 12], "interrupted|incomplete"),
            (b"<tv><programme></title></tv>", "invalid XMLTV"),
            (b"", "incomplete"),
        ];
        for (body, why) in cases {
            let err = epg_import(&mut conn, body).unwrap_err();
            assert!(why.split('|').any(|w| err.contains(w)), "{why}: {err}");
            assert_eq!(programmes(&conn), kept);
            assert_eq!(last_epg(&conn), Some(NOW));
        }

        let (tx, rx) = mpsc::channel(4);
        tx.blocking_send(Ok(Bytes::copy_from_slice(&full[..4096])))
            .unwrap();
        tx.blocking_send(Err("connection reset".into())).unwrap();
        drop(tx);
        let err = epg_import(&mut conn, ChunkReader::new(rx)).unwrap_err();
        assert!(
            err.contains("interrupted") && err.contains("connection reset"),
            "{err}"
        );
        assert_eq!(programmes(&conn), kept);
    }

    /// Peak heap during an EPG import must not grow with the guide.
    #[test]
    fn epg_peak_memory_does_not_scale_with_guide_size() {
        let (_dir, mut conn) = db();
        // Every guide id has an archive, so both runs read the same map.
        let ids: Vec<String> = (0..500).map(|i| format!("ch{i}.uk")).collect();
        let list: Vec<(i64, &str, i64)> =
            (0..).zip(&ids).map(|(i, id)| (i, id.as_str(), 3)).collect();
        import(&mut conn, archive_channels(&list).as_bytes()).unwrap();
        epg_import(&mut conn, Xmltv::new(2)).unwrap();
        let (small_n, small) = peak_during(|| epg_import(&mut conn, Xmltv::new(50)).unwrap());
        let (large_n, large) = peak_during(|| epg_import(&mut conn, Xmltv::new(500)).unwrap());
        assert_eq!(large_n, small_n * 10);
        eprintln!(
            "peak heap during EPG import: {small_n} programmes {small} B, {large_n} programmes {large} B"
        );
        assert!(
            large < small + 64 * 1024,
            "small: {small} B, large: {large} B"
        );
    }

    #[test]
    fn channel_rows_show_what_is_on_now() {
        let (_dir, mut conn) = db_with_channels();
        epg_import(&mut conn, Xmltv::new(3)).unwrap();
        let rowid = |stream_id: i64| -> i64 {
            conn.query_row(
                "SELECT rowid FROM channel WHERE stream_id = ?1",
                [stream_id],
                |r| r.get(0),
            )
            .unwrap()
        };
        let current = (NOW - slot_start(0)) as usize / 3600;
        let row = store::channel(&conn, rowid(100_002), NOW).unwrap().unwrap();
        let on_now = row.now.unwrap();
        assert_eq!(on_now.title, format!("Show 2/{current} & friends"));
        assert!(on_now.start <= NOW && NOW < on_now.stop);
        // A programme boundary: the new one is on.
        let row = store::channel(&conn, rowid(100_002), on_now.stop)
            .unwrap()
            .unwrap();
        assert_eq!(row.now.unwrap().start, on_now.stop);
        // No EPG id, or an id the guide doesn't cover, or past the guide's end.
        assert!(
            store::channel(&conn, rowid(100_001), NOW)
                .unwrap()
                .unwrap()
                .now
                .is_none()
        );
        assert!(
            store::channel(&conn, rowid(100_004), NOW)
                .unwrap()
                .unwrap()
                .now
                .is_none()
        );
        let past_end = slot_start(PER_CHANNEL) + 10 * 86_400;
        assert!(
            store::channel(&conn, rowid(100_002), past_end)
                .unwrap()
                .unwrap()
                .now
                .is_none()
        );
    }

    #[test]
    fn later_is_rest_of_day_with_at_least_a_next() {
        let (_dir, mut conn) = db();
        epg_import(&mut conn, Xmltv::new(1)).unwrap();
        let hour = NOW - NOW % 3600;
        let starts = |until| -> Vec<i64> {
            store::later(&conn, "p", "ch0.uk", NOW, until)
                .unwrap()
                .iter()
                .map(|p| p.start)
                .collect()
        };
        assert_eq!(starts(hour + 3 * 3600), [hour + 3600, hour + 7200]);
        // Nothing else before `until`: the next one still shows.
        assert_eq!(starts(hour + 60), [hour + 3600]);
        assert!(
            store::later(&conn, "p", "nope", NOW, hour + 60)
                .unwrap()
                .is_empty()
        );
    }

    fn import_playlist(conn: &mut Connection, body: &str) -> Result<(usize, usize), String> {
        write_playlist(conn, ("p", "Provider"), body.as_bytes(), None, NOW)
    }

    /// `(stream_id, name, category, url)` in number order.
    fn playlist_rows(conn: &Connection) -> Vec<(i64, String, String, String)> {
        conn.prepare("SELECT stream_id, name, category_id, url FROM channel WHERE provider_id = 'p' ORDER BY num")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// Each tolerated form, as SPEC 6.4 lists them.
    const PLAYLIST: &str = concat!(
        "\u{feff}#EXTM3U url-tvg=\"http://g.example/guide.xml\"\r\n",
        "#EXTINF:-1 tvg-id=\"harbour.uk\" tvg-name=\"Harbour One\" tvg-logo=\"http://l/1.png\" group-title=\"News;UK\",Harbour One, London\r\n",
        "\r\n",
        "# a comment\r\n",
        "#EXTVLCOPT:http-user-agent=Example/1.0\r\n",
        "http://s.example/1.ts?token=a\r\n",
        "#EXTINF:0 tvg-id=kestrel.uk group-title='Drama' tvg-chno=\"12\",Kestrel\n",
        "http://s.example/2.ts|User-Agent=Example\n",
        "#EXT-X-CREDENTIALS:none\n",
        "http://s.example/live/juniper.m3u8?token=t\n",
        "#EXTINF:-1,Lost\n",
        "#EXTINF:-1 tvg-name=\"Cobalt\",\n",
        "rtmp://s.example/cobalt\n",
    );

    #[test]
    fn playlist_imports_all_fields() {
        let (_dir, mut conn) = db();
        assert_eq!(import_playlist(&mut conn, PLAYLIST), Ok((4, 1)));
        type Row = (
            Option<i64>,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
        );
        let row: Row = conn
            .query_row(
                "SELECT num, name, category_id, epg_channel_id, icon_url, tvg_name, url FROM channel WHERE name_norm LIKE 'harbour%'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (
                Some(1),
                "Harbour One, London".into(),
                "News".into(),
                Some("harbour.uk".into()),
                Some("http://l/1.png".into()),
                Some("Harbour One".into()),
                "http://s.example/1.ts?token=a".into()
            )
        );
        let rows = playlist_rows(&conn);
        let named: Vec<(&str, &str)> = rows
            .iter()
            .map(|(_, n, c, _)| (n.as_str(), c.as_str()))
            .collect();
        // File order, `tvg-chno` 12 last; a URL with no `#EXTINF` named
        // after its file, an empty title after its `tvg-name`.
        assert_eq!(
            named,
            [
                ("Harbour One, London", "News"),
                ("juniper.m3u8", "Uncategorised"),
                ("Cobalt", "Uncategorised"),
                ("Kestrel", "Drama")
            ]
        );
        assert_eq!(rows[3].3, "http://s.example/2.ts");
        // Headers from an option and from a `|` suffix (M3U ticket 03).
        let agents: Vec<Option<String>> = rows
            .iter()
            .map(|(id, ..)| {
                let stream = store::channel_stream(&conn, "p", *id).unwrap().unwrap();
                stream.headers.user_agent
            })
            .collect();
        assert_eq!(
            agents,
            [
                Some("Example/1.0".into()),
                None,
                None,
                Some("Example".into())
            ]
        );
        let cats: Vec<(String, i64)> = conn
            .prepare("SELECT name, sort FROM category WHERE provider_id = 'p' ORDER BY sort")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            cats,
            [
                ("News".into(), 0),
                ("Drama".into(), 1),
                ("Uncategorised".into(), 2)
            ]
        );
        assert_eq!(last_refresh(&conn), Some(NOW));
        // The header's feed is kept for the guide (M3U ticket 05), apart.
        assert_eq!(
            store::playlist_guides(&conn, "p").unwrap(),
            ["http://g.example/guide.xml"]
        );
        // No playlist URL, guide feed or token in the name or stamps.
        let dump: String = conn
            .query_row(
                "SELECT group_concat(coalesce(name, '') || coalesce(last_channels_refresh, '')) FROM provider",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!dump.contains("g.example"), "{dump}");
    }

    #[test]
    fn each_playlist_reject_keeps_the_old_channels() {
        let (_dir, mut conn) = db();
        import_playlist(&mut conn, PLAYLIST).unwrap();
        let before = playlist_rows(&conn);
        for (body, why) in [
            ("<html>sign in</html>\n", "#EXTM3U"),
            ("", "#EXTM3U"),
            ("#EXTM3U\n", "no channels"),
            ("#EXTM3U\n#EXTINF:-1,One\n", "no channels"),
            (
                "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6.0,\nseg1.ts\n",
                "a stream",
            ),
            (
                "#EXTM3U\n#EXTINF:-1,One\nhttp://s/1\n#EXT-X-STREAM-INF:BANDWIDTH=1\nlow.m3u8\n",
                "a stream",
            ),
        ] {
            let err = import_playlist(&mut conn, body).unwrap_err();
            assert!(err.contains(why), "{body:?}: {err}");
            assert_eq!(playlist_rows(&conn), before);
            assert_eq!(last_refresh(&conn), Some(NOW));
            assert_eq!(store::playlist_guides(&conn, "p").unwrap().len(), 1);
        }
        // A download cut short.
        let (tx, rx) = mpsc::channel(4);
        tx.blocking_send(Ok(Bytes::from_static(
            b"#EXTM3U\n#EXTINF:-1,One\nhttp://s/1\n",
        )))
        .unwrap();
        tx.blocking_send(Err("Fake: download cut short, 30 of 900 bytes".into()))
            .unwrap();
        drop(tx);
        let err = write_playlist(
            &mut conn,
            ("p", "Provider"),
            ChunkReader::new(rx),
            None,
            NOW,
        )
        .unwrap_err();
        assert!(err.contains("cut short"), "{err}");
        assert_eq!(playlist_rows(&conn), before);
    }

    #[test]
    fn playlist_ids_are_stable() {
        let (_dir, mut conn) = db();
        let list = |entries: &[(&str, &str, &str)]| {
            let mut s = String::from("#EXTM3U\n");
            for (attrs, title, url) in entries {
                s += &format!("#EXTINF:-1 {attrs},{title}\n{url}\n");
            }
            s
        };
        let ids = |conn: &Connection| -> Vec<(String, String, i64)> {
            let mut v: Vec<_> = playlist_rows(conn)
                .into_iter()
                .map(|(id, name, cat, _)| (name, cat, id))
                .collect();
            v.sort();
            v
        };
        let entries = [
            (
                "tvg-id=\"harbour.uk\" group-title=\"News\"",
                "Harbour One",
                "http://s/h?token=a",
            ),
            // One stream in two groups: the same key and URL.
            (
                "tvg-id=\"kestrel.uk\" group-title=\"Drama\"",
                "Kestrel",
                "http://s/k?token=a",
            ),
            (
                "tvg-id=\"kestrel.uk\" group-title=\"Films\"",
                "Kestrel",
                "http://s/k?token=a",
            ),
            // Two streams under one key.
            (
                "tvg-id=\"juniper.uk\" group-title=\"News\"",
                "Juniper",
                "http://s/j1",
            ),
            (
                "tvg-id=\"juniper.uk\" group-title=\"News\"",
                "Juniper HD",
                "http://s/j2",
            ),
            ("tvg-name=\"Cobalt\"", "Cobalt Films", "http://s/c"),
        ];
        import_playlist(&mut conn, &list(&entries)).unwrap();
        let first = ids(&conn);
        assert_eq!(first.len(), 6);
        let mut distinct: Vec<i64> = first.iter().map(|r| r.2).collect();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), 6, "{first:?}");
        assert!(distinct.iter().all(|&id| id >= 0));

        // Again, as it was.
        import_playlist(&mut conn, &list(&entries)).unwrap();
        assert_eq!(ids(&conn), first);

        // Reordered, with a channel added before them; entries that share
        // a key keep their order among themselves.
        let mut moved = entries.to_vec();
        moved.rotate_left(1);
        moved.insert(0, ("tvg-id=\"solstice.uk\"", "Solstice", "http://s/s"));
        import_playlist(&mut conn, &list(&moved)).unwrap();
        let after: Vec<_> = ids(&conn)
            .into_iter()
            .filter(|(name, ..)| name != "Solstice")
            .collect();
        assert_eq!(after, first);

        // A backup added after a channel leaves the channel's id alone.
        let mut backed = entries.to_vec();
        backed.insert(
            1,
            (
                "tvg-id=\"harbour.uk\" group-title=\"News\"",
                "Harbour One",
                "http://s/h2",
            ),
        );
        import_playlist(&mut conn, &list(&backed)).unwrap();
        let harbour = |v: &[(String, String, i64)]| -> Vec<i64> {
            v.iter()
                .filter(|r| r.0 == "Harbour One")
                .map(|r| r.2)
                .collect()
        };
        let now = ids(&conn);
        assert_eq!(now.len(), 7);
        assert!(harbour(&now).contains(&harbour(&first)[0]), "{now:?}");

        // A rotated token keeps a channel whose key is its own.
        let rotated: Vec<_> = entries
            .iter()
            .map(|&(a, t, u)| {
                (
                    a,
                    t,
                    if t == "Harbour One" {
                        "http://s/h?token=b"
                    } else {
                        u
                    },
                )
            })
            .collect();
        import_playlist(&mut conn, &list(&rotated)).unwrap();
        assert_eq!(ids(&conn), first);
    }

    #[test]
    fn favourites_survive_a_playlist_refresh() {
        let (_dir, mut conn) = db();
        let body = |token: &str| {
            format!(
                "#EXTM3U\n#EXTINF:-1 tvg-id=\"harbour.uk\",Harbour One\nhttp://s/h\n\
                 #EXTINF:-1 tvg-id=\"juniper.uk\",Juniper\nhttp://s/j1?token={token}\n\
                 #EXTINF:-1 tvg-id=\"juniper.uk\",Juniper Plus\nhttp://s/j2?token={token}\n"
            )
        };
        import_playlist(&mut conn, &body("a")).unwrap();
        let rows = playlist_rows(&conn);
        for (id, ..) in &rows {
            assert!(store::toggle_favourite(&conn, "p", store::Kind::Live, *id).unwrap());
        }
        // A plain refresh keeps them all.
        import_playlist(&mut conn, &body("a")).unwrap();
        assert!(favourites(&conn).iter().all(|&(_, available)| available));
        // A rotated token gives the later duplicate a new id; it follows by
        // name.
        import_playlist(&mut conn, &body("b")).unwrap();
        let now: Vec<i64> = playlist_rows(&conn).iter().map(|r| r.0).collect();
        assert_ne!(now, rows.iter().map(|r| r.0).collect::<Vec<_>>());
        let favs = favourites(&conn);
        assert_eq!(favs.len(), 3);
        assert!(
            favs.iter()
                .all(|&(id, available)| available && now.contains(&id)),
            "{favs:?}"
        );
    }

    /// An M3U playlist made on the fly, as [`Fixture`] is: double-quoted
    /// attributes, a group in 40, a duplicate `tvg-id` on every tenth.
    struct Playlist {
        next: usize,
        total: usize,
        buf: Vec<u8>,
        pos: usize,
    }

    impl Playlist {
        fn new(total: usize) -> Self {
            Self {
                next: 0,
                total,
                buf: b"#EXTM3U\n".to_vec(),
                pos: 0,
            }
        }
    }

    impl Read for Playlist {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.pos == self.buf.len() {
                self.buf.clear();
                self.pos = 0;
                let i = self.next;
                if i == self.total {
                    return Ok(0);
                }
                self.next += 1;
                let id = if i % 10 == 9 { i - 1 } else { i };
                self.buf.extend_from_slice(
                    format!(
                        "#EXTINF:-1 tvg-id=\"ch{id}.uk\" tvg-name=\"Channel {i}\" tvg-logo=\"http://img/{i}.png\" group-title=\"Group {}\",UK: Channel {i} ᴴᴰ\nhttp://s.example/live/{i}.ts?token=abcdef\n",
                        i % 40
                    )
                    .as_bytes(),
                );
            }
            let n = out.len().min(self.buf.len() - self.pos);
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    /// Peak heap during an M3U import must not grow with the list (7.1).
    #[test]
    fn m3u_import_peak_memory_does_not_scale_with_list_size() {
        let (_dir, mut conn) = db();
        let mut import =
            |n| write_playlist(&mut conn, ("p", "Provider"), Playlist::new(n), None, NOW);
        import(10).unwrap();
        let (n, small) = peak_during(|| import(5_000).unwrap());
        assert_eq!(n, (5_000, 0));
        let (n, large) = peak_during(|| import(50_000).unwrap());
        assert_eq!(n, (50_000, 0));
        eprintln!("peak heap during import: 5k entries {small} B, 50k entries {large} B");
        assert!(large < small + 64 * 1024, "5k: {small} B, 50k: {large} B");
    }

    /// 50k entries from the body to the commit, for `docs/perf/v1.md`:
    /// `cargo test --release m3u_import_time -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn m3u_import_time() {
        let (_dir, mut conn) = db();
        write_playlist(
            &mut conn,
            ("p", "Provider"),
            Playlist::new(50_000),
            None,
            NOW,
        )
        .unwrap();
        let t = std::time::Instant::now();
        let n = write_playlist(
            &mut conn,
            ("p", "Provider"),
            Playlist::new(50_000),
            None,
            NOW,
        )
        .unwrap();
        eprintln!("{n:?} in {} ms", t.elapsed().as_millis());
    }
}
