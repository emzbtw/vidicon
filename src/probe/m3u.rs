//! `--probe-m3u <url-or-path>` (`devtools` only, M3U ticket 01): what an
//! M3U playlist holds, before the parser is built around guesses. Streams
//! the playlist line by line and prints counts only: size, entries, line
//! endings, quoting, which attributes appear, groups, duplicates, option
//! lines, URL shapes and the header's guide feeds. A URL is fetched twice,
//! [`REFETCH`] apart, to count which identity keeps more channels. Never
//! URLs, hosts, ids or names, so the output can be pasted anywhere.

use crate::import::ChunkReader;
use crate::xtream::http;
use reqwest::Url;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufRead, BufReader, Read};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const REFETCH: Duration = Duration::from_secs(5 * 60);
/// IPTVnator's old URL limit, which collapsed long token lists (research 7).
const LONG_URL: usize = 2084;

pub async fn run(source: String) -> Result<(), String> {
    let t = Instant::now();
    match Url::parse(&source) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => {
            let first = fetch(url.clone()).await?;
            print_playlist(&first, t.elapsed().as_secs_f64());
            println!(
                "waiting {} min for the second fetch",
                REFETCH.as_secs() / 60
            );
            tokio::time::sleep(REFETCH).await;
            let t = Instant::now();
            let second = fetch(url).await?;
            println!(
                "second fetch: {:.1} MB, {} entries, read in {:.1} s",
                second.bytes as f64 / 1e6,
                second.keys.len(),
                t.elapsed().as_secs_f64()
            );
            compare(&first, &second);
        }
        _ => {
            let file = std::fs::File::open(&source).map_err(|e| format!("playlist: {e}"))?;
            let p = tokio::task::spawn_blocking(move || scan(file))
                .await
                .map_err(|e| format!("scan task failed: {e}"))??;
            print_playlist(&p, t.elapsed().as_secs_f64());
        }
    }
    Ok(())
}

/// Streams the playlist through [`scan`]. Errors carry no URL.
async fn fetch(url: Url) -> Result<Playlist, String> {
    let (tx, rx) = mpsc::channel(8);
    let job = tokio::task::spawn_blocking(move || scan(ChunkReader::new(rx)));
    let fetched = async {
        let mut resp = http().get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Ok(Err(format!("playlist: HTTP {status}")));
        }
        let length = resp.content_length();
        while let Some(chunk) = resp.chunk().await? {
            if tx.send(Ok(chunk)).await.is_err() {
                break;
            }
        }
        Ok::<_, reqwest::Error>(Ok(length))
    }
    .await
    .map_err(|e| format!("playlist: {}", e.without_url()))
    .and_then(|r| r);
    if let Err(e) = &fetched {
        let _ = tx.send(Err(e.clone())).await;
    }
    drop(tx);
    let scanned = job.await.map_err(|e| format!("scan task failed: {e}"))?;
    let length = fetched?;
    let mut p = scanned?;
    p.content_length = length;
    Ok(p)
}

/// Picks the key an id scheme starts from.
type Key = fn(&Keys) -> u64;

/// How duplicates of one key are told apart.
#[derive(Clone, Copy)]
enum Scheme {
    Occurrence,
    UrlHash,
}

/// One entry's identity inputs, hashed: what ticket 02 might key on.
#[derive(Clone, Copy)]
struct Keys {
    /// `tvg-id`, else `tvg-name`, else the title.
    with_name: u64,
    /// `tvg-id`, else the title.
    without_name: u64,
    /// The URL, without a pipe suffix.
    url: u64,
    /// The URL without its query or pipe suffix: survives a rotated token.
    path: u64,
    tvg_id: Option<u64>,
    tvg_name: Option<u64>,
}

#[derive(Default)]
struct Playlist {
    bytes: u64,
    content_length: Option<u64>,
    lines: usize,
    bom: bool,
    crlf: usize,
    not_utf8: usize,
    blank: usize,
    /// The first non-empty line is `#EXTM3U`.
    header_first: bool,
    /// `#EXTM3U` lines after the first.
    more_headers: usize,
    /// Header attribute key to its count.
    header_attrs: BTreeMap<String, usize>,
    /// Guide feeds named by `url-tvg` and `x-tvg-url`, by compression.
    feeds: BTreeMap<&'static str, usize>,
    extinf: usize,
    /// `#EXTINF` lines followed by another `#EXTINF`, or the end, before a URL.
    no_url: usize,
    /// URL lines with no `#EXTINF` above.
    no_extinf: usize,
    /// Entries with a blank or other `#` line between `#EXTINF` and URL.
    gaps: usize,
    empty_titles: usize,
    /// Entries with more than one comma outside quotes: a comma in the title,
    /// which the first comma rule keeps and the last comma rule cuts.
    commas: usize,
    /// Attribute values double-quoted, single-quoted, unquoted.
    quotes: [usize; 3],
    unterminated: usize,
    /// Attribute key to the entries that have it.
    attrs: BTreeMap<String, usize>,
    logo: usize,
    /// Hashes of the first group of each entry.
    groups: HashSet<u64>,
    no_group: usize,
    multi_group: usize,
    chno: [usize; 2],
    /// `catchup` value to its count.
    catchup: BTreeMap<String, usize>,
    templates: usize,
    shifted: usize,
    radio: usize,
    /// `#EXTVLCOPT` and `#KODIPROP` keys to their counts.
    vlcopt: BTreeMap<String, usize>,
    kodiprop: BTreeMap<String, usize>,
    /// Other `#` lines by directive, `#` alone for comments.
    directives: BTreeMap<String, usize>,
    hls: usize,
    schemes: BTreeMap<String, usize>,
    longest_url: usize,
    long_urls: usize,
    piped: usize,
    pipe_keys: BTreeMap<String, usize>,
    keys: Vec<Keys>,
}

/// An `#EXTINF` waiting for its URL.
#[derive(Default)]
struct Pending {
    title: String,
    tvg_id: Option<String>,
    tvg_name: Option<String>,
    gap: bool,
}

/// Reads the playlist line by line into counts and per-entry hashes.
fn scan(body: impl Read) -> Result<Playlist, String> {
    let mut body = BufReader::with_capacity(64 * 1024, body);
    let mut p = Playlist::default();
    let (mut line, mut first) = (Vec::new(), true);
    let mut pending: Option<Pending> = None;
    loop {
        line.clear();
        let n = body
            .read_until(b'\n', &mut line)
            .map_err(|e| format!("playlist interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        p.bytes += n as u64;
        p.lines += 1;
        let mut raw = &line[..];
        if p.lines == 1 && raw.starts_with(b"\xef\xbb\xbf") {
            p.bom = true;
            raw = &raw[3..];
        }
        if raw.ends_with(b"\r\n") {
            p.crlf += 1;
        }
        let text = match std::str::from_utf8(raw) {
            Ok(s) => std::borrow::Cow::Borrowed(s),
            Err(_) => {
                p.not_utf8 += 1;
                String::from_utf8_lossy(raw)
            }
        };
        let text = text.trim();
        if text.is_empty() {
            p.blank += 1;
            if let Some(e) = &mut pending {
                e.gap = true;
            }
            continue;
        }
        let was_first = std::mem::replace(&mut first, false);
        if let Some(rest) = strip_ci(text, "#EXTM3U") {
            if was_first {
                p.header_first = true;
            } else {
                p.more_headers += 1;
            }
            header(&mut p, rest);
        } else if let Some(rest) = strip_ci(text, "#EXTINF:") {
            if pending.is_some() {
                p.no_url += 1;
            }
            pending = Some(extinf(&mut p, rest));
        } else if let Some(rest) = strip_ci(text, "#EXTVLCOPT:") {
            *p.vlcopt.entry(key_of(rest)).or_default() += 1;
        } else if let Some(rest) = strip_ci(text, "#KODIPROP:") {
            *p.kodiprop.entry(key_of(rest)).or_default() += 1;
        } else if text.starts_with('#') {
            if strip_ci(text, "#EXT-X-").is_some() {
                p.hls += 1;
            }
            // Only `#EXT` directives are named; a comment could hold a name.
            let d = strip_ci(text, "#EXT").map_or("#".into(), |rest| {
                format!("#EXT{}", label(rest.split(':').next().unwrap_or(""))).to_ascii_uppercase()
            });
            *p.directives.entry(d).or_default() += 1;
            if let Some(e) = &mut pending {
                e.gap = true;
            }
        } else {
            let e = pending.take().unwrap_or_else(|| {
                p.no_extinf += 1;
                Pending {
                    title: last_segment(text).to_owned(),
                    ..Pending::default()
                }
            });
            entry(&mut p, e, text);
        }
    }
    if pending.is_some() {
        p.no_url += 1;
    }
    Ok(p)
}

/// `prefix` at the start of `s`, ignoring ASCII case; the rest.
fn strip_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// A printable key: lowercased, or `other` when it could be data.
fn label(k: &str) -> String {
    let k = k.trim().to_ascii_lowercase();
    let plain = !k.is_empty()
        && k.len() <= 40
        && k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    if plain { k } else { "other".into() }
}

/// The key of a `key=value` option line.
fn key_of(rest: &str) -> String {
    label(rest.split('=').next().unwrap_or(""))
}

/// The last path segment of a URL or path, without the query.
fn last_segment(url: &str) -> &str {
    let path = url.split(['?', '#', '|']).next().unwrap_or(url);
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

fn header(p: &mut Playlist, rest: &str) {
    let (attrs, _) = attributes(rest.trim_start());
    for a in attrs {
        *p.header_attrs.entry(label(&a.key)).or_default() += 1;
        if !matches!(a.key.as_str(), "url-tvg" | "x-tvg-url") {
            continue;
        }
        for feed in a.value.split(',').map(str::trim).filter(|f| !f.is_empty()) {
            let path = Url::parse(feed)
                .map(|u| u.path().to_ascii_lowercase())
                .unwrap_or_else(|_| last_segment(feed).to_ascii_lowercase());
            let kind = if path.ends_with(".gz") {
                "gz"
            } else if path.ends_with(".xz") {
                "xz"
            } else if path.ends_with(".xml") {
                "xml"
            } else {
                "other"
            };
            *p.feeds.entry(kind).or_default() += 1;
        }
    }
}

/// How an attribute value was written.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Quote {
    Double,
    Single,
    None,
}

#[derive(Debug)]
struct Attr {
    key: String,
    value: String,
    quote: Quote,
    terminated: bool,
}

/// `key=value` pairs, values double-quoted, single-quoted or unquoted, up to
/// the first comma outside quotes; and that comma's byte offset with the
/// count of commas outside quotes.
fn attributes(s: &str) -> (Vec<Attr>, Option<(usize, usize)>) {
    // The title starts after the first comma outside quotes (ticket 01: 329
    // titles on a provider's list hold commas). Quotes open only right after
    // `=`, so an apostrophe in a title opens nothing.
    let (mut quote, mut first, mut commas, mut prev) = (None::<u8>, None, 0, 0u8);
    for (i, b) in s.bytes().enumerate() {
        match (quote, b) {
            (Some(q), _) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') if prev == b'=' => quote = Some(b),
            (None, b',') => (first, commas) = (first.or(Some(i)), commas + 1),
            _ => {}
        }
        prev = b;
    }
    let head = first.map_or(s, |i| &s[..i]);
    let mut attrs = Vec::new();
    let mut rest = head;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let end = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(rest.len());
        let key = rest[..end].to_ascii_lowercase();
        rest = &rest[end..];
        let Some(after) = rest.strip_prefix('=') else {
            continue;
        };
        let (value, quote, terminated, left) = match after.as_bytes().first() {
            Some(&q @ (b'"' | b'\'')) => match after[1..].find(q as char) {
                Some(j) => (&after[1..1 + j], q, true, &after[2 + j..]),
                None => (&after[1..], q, false, ""),
            },
            _ => {
                let j = after.find(char::is_whitespace).unwrap_or(after.len());
                (&after[..j], 0, true, &after[j..])
            }
        };
        attrs.push(Attr {
            key,
            value: value.to_owned(),
            quote: match quote {
                b'"' => Quote::Double,
                b'\'' => Quote::Single,
                _ => Quote::None,
            },
            terminated,
        });
        rest = left;
    }
    (attrs, first.map(|i| (i, commas)))
}

/// Counts an `#EXTINF` line's attributes and title.
fn extinf(p: &mut Playlist, rest: &str) -> Pending {
    p.extinf += 1;
    // The duration comes first, up to whitespace or the title's comma.
    let start = rest
        .find(|c: char| c.is_whitespace() || c == ',')
        .unwrap_or(rest.len());
    let (attrs, comma) = attributes(&rest[start..]);
    let title = comma.map_or("", |(i, _)| rest[start + i + 1..].trim());
    if comma.is_some_and(|(_, n)| n > 1) {
        p.commas += 1;
    }
    if title.is_empty() {
        p.empty_titles += 1;
    }
    let mut e = Pending {
        title: title.to_owned(),
        ..Pending::default()
    };
    let (mut group, mut logo) = (None, false);
    let mut seen = HashSet::new();
    for a in &attrs {
        p.quotes[a.quote as usize] += 1;
        if !a.terminated {
            p.unterminated += 1;
        }
        if seen.insert(a.key.clone()) {
            *p.attrs.entry(label(&a.key)).or_default() += 1;
        }
        let v = a.value.trim();
        match a.key.as_str() {
            "tvg-id" if !v.is_empty() => e.tvg_id = Some(v.to_owned()),
            "tvg-name" if !v.is_empty() => e.tvg_name = Some(v.to_owned()),
            "tvg-logo" | "tvg-icon" | "logo" if !v.is_empty() => logo = true,
            "group-title" | "group" if !v.is_empty() && group.is_none() => group = Some(v),
            "tvg-chno" | "tvg-ch" if !v.is_empty() => {
                p.chno[usize::from(v.parse::<u32>().is_err())] += 1;
            }
            "catchup" | "catchup-type" => {
                let v = v.to_ascii_lowercase();
                let known = [
                    "default",
                    "append",
                    "shift",
                    "flussonic",
                    "flussonic-hls",
                    "flussonic-ts",
                    "fs",
                    "xc",
                    "vod",
                ];
                let k = if known.contains(&v.as_str()) {
                    v
                } else {
                    "other".into()
                };
                *p.catchup.entry(k).or_default() += 1;
            }
            "catchup-source" if v.contains('{') => p.templates += 1,
            "tvg-shift" if v.parse::<f64>().is_ok_and(|s| s != 0.0) => p.shifted += 1,
            "radio" if v.eq_ignore_ascii_case("true") => p.radio += 1,
            _ => {}
        }
    }
    p.logo += usize::from(logo);
    match group {
        None => p.no_group += 1,
        Some(g) => {
            if g.contains(';') {
                p.multi_group += 1;
            }
            let first = g.split(';').next().unwrap_or(g).trim();
            p.groups.insert(hash(first));
        }
    }
    e
}

fn hash(v: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// Counts a URL line and keeps the entry's hashes.
fn entry(p: &mut Playlist, e: Pending, url: &str) {
    if e.gap {
        p.gaps += 1;
    }
    let scheme = url
        .split_once("://")
        .map(|(s, _)| s)
        .filter(|s| {
            s.len() <= 10
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
        })
        .map_or("none".into(), str::to_ascii_lowercase);
    *p.schemes.entry(scheme).or_default() += 1;
    p.longest_url = p.longest_url.max(url.len());
    if url.len() > LONG_URL {
        p.long_urls += 1;
    }
    if let Some((_, suffix)) = url.split_once('|') {
        p.piped += 1;
        for kv in suffix.split('&') {
            *p.pipe_keys.entry(key_of(kv)).or_default() += 1;
        }
    }
    let title = e.title.as_str();
    p.keys.push(Keys {
        with_name: hash(
            e.tvg_id
                .as_deref()
                .or(e.tvg_name.as_deref())
                .unwrap_or(title),
        ),
        without_name: hash(e.tvg_id.as_deref().unwrap_or(title)),
        url: hash(url.split('|').next().unwrap_or(url)),
        path: hash(url.split(['?', '|']).next().unwrap_or(url)),
        tvg_id: e.tvg_id.map(hash),
        tvg_name: e.tvg_name.map(hash),
    });
}

/// Keys seen more than once, and the entries holding them.
fn duplicates(keys: impl Iterator<Item = u64>) -> (usize, usize, usize) {
    let mut n: HashMap<u64, usize> = HashMap::new();
    for k in keys {
        *n.entry(k).or_default() += 1;
    }
    let dup = n.values().filter(|&&c| c > 1);
    (n.len(), dup.clone().count(), dup.sum())
}

fn counts<K: std::fmt::Display>(m: &BTreeMap<K, usize>) -> String {
    if m.is_empty() {
        return "none".into();
    }
    let mut v: Vec<_> = m.iter().collect();
    v.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    v.iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_playlist(p: &Playlist, secs: f64) {
    let entries = p.keys.len();
    println!(
        "playlist: {:.1} MB, {} lines, read in {secs:.1} s",
        p.bytes as f64 / 1e6,
        p.lines
    );
    if let Some(len) = p.content_length.filter(|&l| l != p.bytes) {
        println!("  Content-Length {len}, read {}", p.bytes);
    }
    println!(
        "  #EXTM3U first: {}, more #EXTM3U lines {}; BOM {}; CRLF lines {}; lines not UTF-8 {}; blank lines {}",
        p.header_first, p.more_headers, p.bom, p.crlf, p.not_utf8, p.blank
    );
    println!("  header attributes: {}", counts(&p.header_attrs));
    println!("  header guide feeds: {}", counts(&p.feeds));
    println!(
        "  entries {entries}: #EXTINF {}, #EXTINF without a URL {}, URLs without #EXTINF {}, blank or # lines between #EXTINF and URL {}",
        p.extinf, p.no_url, p.no_extinf, p.gaps
    );
    println!(
        "  titles: empty {}, more than one comma outside quotes {}",
        p.empty_titles, p.commas
    );
    println!(
        "  attribute values: double-quoted {}, single-quoted {}, unquoted {}, unterminated {}",
        p.quotes[0], p.quotes[1], p.quotes[2], p.unterminated
    );
    println!("  entries per attribute: {}", counts(&p.attrs));
    println!(
        "  logo (any alias) {}; tvg-chno or tvg-ch numeric {}, not numeric {}",
        p.logo, p.chno[0], p.chno[1]
    );
    println!(
        "  groups {} distinct (first of ;-separated), entries with ;-separated groups {}, without a group {}",
        p.groups.len(),
        p.multi_group,
        p.no_group
    );
    println!(
        "  catchup: {}; catchup-source templates {}; non-zero tvg-shift {}; radio=true {}",
        counts(&p.catchup),
        p.templates,
        p.shifted,
        p.radio
    );
    let (ids, dup_ids, in_dup_ids) = duplicates(p.keys.iter().filter_map(|k| k.tvg_id));
    let (names, dup_names, _) = duplicates(p.keys.iter().filter_map(|k| k.tvg_name));
    let (keys, dup_keys, in_dup_keys) = duplicates(p.keys.iter().map(|k| k.with_name));
    let (_, dup_urls, _) = duplicates(p.keys.iter().map(|k| k.url));
    let named_only = p
        .keys
        .iter()
        .filter(|k| k.tvg_id.is_none() && k.tvg_name.is_some())
        .count();
    println!(
        "  tvg-id: {ids} distinct, {dup_ids} on more than one entry ({in_dup_ids} entries); tvg-name: {names} distinct, {dup_names} repeated; tvg-name without tvg-id {named_only}"
    );
    println!(
        "  identity (tvg-id, tvg-name, title): {keys} distinct, {dup_keys} repeated ({in_dup_keys} entries); URLs repeated {dup_urls}"
    );
    println!("  #EXTVLCOPT: {}", counts(&p.vlcopt));
    println!("  #KODIPROP: {}", counts(&p.kodiprop));
    println!(
        "  other # lines: {}; #EXT-X- lines {}",
        counts(&p.directives),
        p.hls
    );
    println!(
        "  URL schemes: {}; longest URL {} chars, over {LONG_URL} {}",
        counts(&p.schemes),
        p.longest_url,
        p.long_urls
    );
    println!("  pipe suffixes {}: {}", p.piped, counts(&p.pipe_keys));
}

/// Each entry's id under one scheme, to its URL path: the occurrence
/// index among entries with the same key, or the key alone when unique and
/// else the URL hash (with an occurrence index when the URL repeats too).
fn ids(keys: &[Keys], key: Key, scheme: Scheme) -> HashMap<u64, u64> {
    let mut seen: HashMap<u64, usize> = HashMap::new();
    for k in keys {
        *seen.entry(key(k)).or_default() += 1;
    }
    let mut occurrence: HashMap<(u64, u64), usize> = HashMap::new();
    keys.iter()
        .map(|k| {
            let id = match (scheme, seen[&key(k)]) {
                (Scheme::UrlHash, 1) => hash(key(k)),
                (Scheme::UrlHash, _) => {
                    let i = occurrence.entry((key(k), k.url)).or_default();
                    *i += 1;
                    hash((key(k), k.url, *i))
                }
                (Scheme::Occurrence, _) => {
                    let i = occurrence.entry((key(k), 0)).or_default();
                    *i += 1;
                    hash((key(k), *i))
                }
            };
            (id, k.path)
        })
        .collect()
}

/// What a fetch lost of the first fetch's ids under one scheme.
#[derive(Debug, PartialEq)]
struct Lost {
    gone: usize,
    /// Ids now on a URL path another entry had: reordered duplicates.
    moved: usize,
}

fn lost(a: &Playlist, b: &Playlist, key: Key, scheme: Scheme) -> Lost {
    let (x, y) = (ids(&a.keys, key, scheme), ids(&b.keys, key, scheme));
    let paths: HashSet<u64> = a.keys.iter().map(|k| k.path).collect();
    Lost {
        gone: x.keys().filter(|id| !y.contains_key(id)).count(),
        moved: x
            .iter()
            .filter(|(id, path)| y.get(id).is_some_and(|p| p != *path && paths.contains(p)))
            .count(),
    }
}

/// A `tvg-id` on one entry, with its `tvg-name`, or on several.
enum Named {
    Once(Option<u64>),
    Repeated,
}

/// Per scheme, the ids the second fetch lost or moved, and how the
/// `tvg-name`s under unique `tvg-id`s changed.
fn compare(a: &Playlist, b: &Playlist) {
    let gone = |f: fn(&Keys) -> u64| {
        let later: HashSet<u64> = b.keys.iter().map(f).collect();
        a.keys
            .iter()
            .map(f)
            .collect::<HashSet<_>>()
            .difference(&later)
            .count()
    };
    println!(
        "between fetches: entries {} then {}; URLs gone {}, URL paths (no query) gone {}",
        a.keys.len(),
        b.keys.len(),
        gone(|k| k.url),
        gone(|k| k.path)
    );
    let schemes: [(&str, Key); 2] = [
        ("tvg-id, tvg-name, title", |k| k.with_name),
        ("tvg-id, title", |k| k.without_name),
    ];
    for (name, key) in schemes {
        let occ = lost(a, b, key, Scheme::Occurrence);
        let url = lost(a, b, key, Scheme::UrlHash);
        println!(
            "  key {name}: with occurrence index {} ids gone, {} moved to another entry's URL; with URL hash {} gone",
            occ.gone, occ.moved, url.gone
        );
    }
    let unique = |p: &Playlist| {
        let mut m: HashMap<u64, Named> = HashMap::new();
        for k in &p.keys {
            if let Some(id) = k.tvg_id {
                m.entry(id)
                    .and_modify(|v| *v = Named::Repeated)
                    .or_insert(Named::Once(k.tvg_name));
            }
        }
        m
    };
    let later = unique(b);
    let (mut both, mut changed, mut added, mut dropped) = (0, 0, 0, 0);
    for (id, first) in unique(a) {
        let (Named::Once(first), Some(Named::Once(second))) = (first, later.get(&id)) else {
            continue;
        };
        both += 1;
        match (first, second) {
            (Some(x), Some(y)) if x != *y => changed += 1,
            (None, Some(_)) => added += 1,
            (Some(_), None) => dropped += 1,
            _ => {}
        }
    }
    println!(
        "  tvg-name under the same unique tvg-id ({both} ids): changed {changed}, added {added}, dropped {dropped}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "\u{feff}#EXTM3U url-tvg=\"https://guide.example.net/a.xml.gz,https://guide.example.net/b.xml.xz\"\r\n\
        #EXTINF:-1 tvg-id=\"harbour.one\" tvg-name='Harbour One' group-title=\"News;Local\",Harbour One\r\n\
        #EXTVLCOPT:http-user-agent=Example/1.0\r\n\
        http://streams.example.net/1.ts\r\n\
        \r\n\
        #EXTINF:-1 tvg-id=\"harbour.one\" tvg-logo=\"http://img.example.net/h.png\" group-title=\"News, Weather\",Harbour One, HD\r\n\
        # a comment\r\n\
        http://streams.example.net/2.ts|User-Agent=Example&Referer=http://example.net/\r\n\
        #EXTINF:-1 tvg-chno=7 catchup=\"append\" catchup-source=\"?utc={utc}\",Summit\r\n\
        #EXTINF:0,Lost\r\n\
        #KODIPROP:inputstream.adaptive.license_type=clearkey\r\n\
        rtmp://streams.example.net/live/summit\r\n\
        https://streams.example.net/vod/loose.mp4?token=1\r\n";

    #[test]
    fn attributes_read_every_quoting() {
        let (a, comma) = attributes(r#" tvg-id="a,b" tvg-name='Its' group=News x,Bob's, Show"#);
        assert_eq!(a.len(), 3);
        assert_eq!((a[0].key.as_str(), a[0].value.as_str()), ("tvg-id", "a,b"));
        assert_eq!(a[1].quote, Quote::Single);
        assert_eq!((a[2].value.as_str(), a[2].quote), ("News", Quote::None));
        // Two commas outside quotes: the title is after the first one.
        let (i, n) = comma.unwrap();
        assert_eq!(n, 2);
        assert!(
            r#" tvg-id="a,b" tvg-name='Its' group=News x,Bob's, Show"#[i..]
                .starts_with(",Bob's, Show")
        );
        let (a, _) = attributes(r#" tvg-id="open"#);
        assert!(!a[0].terminated);
    }

    #[test]
    fn scan_counts_a_playlist() {
        let p = scan(LIST.as_bytes()).unwrap();
        assert!(p.bom && p.header_first);
        assert_eq!(p.crlf, p.lines);
        assert_eq!(p.feeds.get("gz"), Some(&1));
        assert_eq!(p.feeds.get("xz"), Some(&1));
        assert_eq!(p.extinf, 4);
        assert_eq!(p.keys.len(), 4);
        assert_eq!((p.no_url, p.no_extinf, p.gaps), (1, 1, 1));
        assert_eq!(p.commas, 1);
        assert_eq!(p.multi_group, 1);
        assert_eq!(p.groups.len(), 2);
        assert_eq!(p.no_group, 2);
        assert_eq!(p.chno, [1, 0]);
        assert_eq!(p.catchup.get("append"), Some(&1));
        assert_eq!(p.templates, 1);
        assert_eq!(p.vlcopt.get("http-user-agent"), Some(&1));
        assert_eq!(
            p.kodiprop.get("inputstream.adaptive.license_type"),
            Some(&1)
        );
        assert_eq!(p.directives.get("#"), Some(&1));
        assert_eq!((p.piped, p.pipe_keys.len()), (1, 2));
        assert_eq!(p.schemes.get("rtmp"), Some(&1));
        assert_eq!(p.attrs.get("tvg-id"), Some(&2));
        assert_eq!(
            duplicates(p.keys.iter().filter_map(|k| k.tvg_id)),
            (1, 1, 2)
        );
        // The loose URL is named after its last path segment.
        assert_eq!(
            last_segment("https://h.example.net/vod/loose.mp4?t=1"),
            "loose.mp4"
        );
        // Comments are not named: they could hold a channel name.
        let p = scan("#EXTM3U\n#EXTGRP:News\n#Harbour\n".as_bytes()).unwrap();
        assert_eq!(p.directives.get("#EXTGRP"), Some(&1));
        assert_eq!(p.directives.get("#"), Some(&1));
    }

    #[test]
    fn ids_follow_order_or_urls() {
        let a = scan(LIST.as_bytes()).unwrap();
        let k = |k: &Keys| k.with_name;
        // The two harbour.one entries swap places: occurrence ids point at
        // each other's URL, URL ids hold.
        let swapped = LIST
            .replace("/1.ts", "/x.ts")
            .replace("/2.ts", "/1.ts")
            .replace("/x.ts", "/2.ts");
        let b = scan(swapped.as_bytes()).unwrap();
        let l = |gone, moved| Lost { gone, moved };
        assert_eq!(lost(&a, &b, k, Scheme::Occurrence), l(0, 2));
        assert_eq!(lost(&a, &b, k, Scheme::UrlHash), l(0, 0));
        // A rotated token loses a duplicate's URL id only. Swapped as well,
        // the paths still show the move.
        let rotated = |list: &str| scan(list.replace("/2.ts", "/2.ts?token=9").as_bytes()).unwrap();
        assert_eq!(lost(&a, &rotated(LIST), k, Scheme::UrlHash), l(1, 0));
        assert_eq!(lost(&a, &rotated(LIST), k, Scheme::Occurrence), l(0, 0));
        assert_eq!(lost(&a, &rotated(&swapped), k, Scheme::Occurrence), l(0, 2));
        // Two entries with one key and one URL stay two ids.
        let twice = LIST.replace("/2.ts", "/1.ts");
        assert_eq!(
            ids(&scan(twice.as_bytes()).unwrap().keys, k, Scheme::UrlHash).len(),
            4
        );
    }
}
