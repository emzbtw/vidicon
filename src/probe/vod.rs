//! `--probe-vod` (`devtools` only, VOD ticket 01): how the provider serves
//! movies and series. Lists, info, ids, streams, mpv, posters and downloads,
//! printed as shapes, counts, sizes and timings. As the archive probe, never
//! URLs, hosts, credentials or titles; hosts are letters.

use super::{GAP, int, mpv_seek};
use crate::config::{self, Provider};
use crate::import::ChunkReader;
use crate::store;
use crate::xtream::{Client, get_small, http, redact};
use reqwest::{Url, header};
use serde::de::{Deserializer, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::BufReader;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Field name to how many elements had it in each shape.
type Fields = BTreeMap<String, BTreeMap<&'static str, usize>>;

/// Category id to (name, shown by the rules).
type Categories = HashMap<String, (String, bool)>;

pub async fn run(provider: Provider, mpv: &str) -> Result<(), String> {
    let provider_id = provider.id.clone();
    let client = Client::new(provider);
    let own = client.host();

    // What the database knows about live, and the category rules.
    let db = store::open_reader(&config::database_path()).map_err(|e| e.to_string())?;
    let live_ids: HashSet<i64> = db
        .prepare("SELECT stream_id FROM channel WHERE provider_id = ?1")
        .and_then(|mut s| s.query_map([&provider_id], |r| r.get(0))?.collect())
        .map_err(|e| e.to_string())?;
    let live_cats: HashMap<String, String> = db
        .prepare("SELECT category_id, name FROM category WHERE provider_id = ?1 AND kind = 'live'")
        .and_then(|mut s| {
            s.query_map([&provider_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect()
        })
        .map_err(|e| e.to_string())?;
    let rules: Vec<String> = store::category_rules(&db, &provider_id, store::Kind::Live)
        .map_err(|e| e.to_string())?
        .iter()
        .map(|r| store::fold(r))
        .collect();
    drop(db);
    println!(
        "live, stored: {} channels, {} categories, {} category rules",
        live_ids.len(),
        live_cats.len(),
        rules.len()
    );

    let account = client.account().await?;
    let cons_before = account.user_info.active_cons;
    println!(
        "account: active_cons {}, max_connections {}, formats {:?}",
        opt(cons_before),
        opt(account.user_info.max_connections),
        account.user_info.allowed_output_formats
    );

    // 1. Categories.
    let movie_cats = categories(&client, "get_vod_categories", &live_cats, &rules).await?;
    let series_cats = categories(&client, "get_series_categories", &live_cats, &rules).await?;
    let both = movie_cats.keys().filter(|k| series_cats.contains_key(*k));
    let same = both
        .clone()
        .filter(|k| movie_cats[*k].0 == series_cats[*k].0)
        .count();
    println!(
        "movie and series category ids shared: {}, same name: {same}",
        both.count()
    );
    // Why the rules show what they do, without names: how each kind's
    // names begin, and what each rule matches on its first word alone.
    let names = |c: &Categories| c.values().map(|v| v.0.clone()).collect::<Vec<_>>();
    let kinds = [
        ("live", live_cats.values().cloned().collect::<Vec<_>>()),
        ("movie", names(&movie_cats)),
        ("series", names(&series_cats)),
    ];
    for (kind, names) in &kinds {
        println!("{kind} category names begin: {}", prefixes(names));
    }
    for (n, rule) in (1..).zip(&rules) {
        let word = first_word(rule);
        let loose: Vec<String> = kinds
            .iter()
            .map(|(kind, names)| {
                let exact = names
                    .iter()
                    .filter(|c| store::matches_rules(std::slice::from_ref(rule), c))
                    .count();
                let first = names
                    .iter()
                    .filter(|c| word.is_some() && first_word(c) == word)
                    .count();
                format!("{kind} {exact} as it is, {first} on its first word")
            })
            .collect();
        println!("rule {n} ({}): {}", pattern(rule), loose.join("; "));
    }

    // 2. Lists.
    let movies = Tally::new("stream_id", "stream_icon", own.clone(), movie_cats);
    let (movies, bytes, ms) = list(&client, "get_vod_streams", movies).await?;
    movies.print("get_vod_streams", bytes, ms, &live_ids, &HashSet::new());
    let movie_ids: HashSet<i64> = movies.ids.iter().copied().collect();
    let series = Tally::new("series_id", "cover", own.clone(), series_cats);
    let (series, bytes, ms) = list(&client, "get_series", series).await?;
    series.print("get_series", bytes, ms, &live_ids, &movie_ids);
    println!(
        "names in memory, as live search keeps them: {} titles, {:.1} MiB",
        movies.count + series.count,
        (movies.name_bytes + series.name_bytes) as f64 / 1_048_576.0
    );

    // 3. Info, for titles spread through the shown categories.
    let mut movie_stream = None;
    for (n, (id, ext)) in (1..).zip(spread(movies.sample_from(), 3)) {
        let mut url = client.api_url(Some("get_vod_info"))?;
        url.query_pairs_mut().append_pair("vod_id", &id.to_string());
        match get_json(&client, url).await {
            Err(e) => println!("get_vod_info {n}: {}", redact(&e)),
            Ok((v, bytes, ms)) => {
                println!("get_vod_info {n}: {} in {ms} ms", size(bytes));
                print_object("  ", &v);
                let secs = v["info"].get("duration_secs").and_then(int);
                println!("  duration_secs {}", opt(secs));
                let ext = v["movie_data"]
                    .get("container_extension")
                    .and_then(text)
                    .unwrap_or(ext);
                movie_stream.get_or_insert((id, ext));
            }
        }
    }
    let mut episode_stream = None;
    let (mut episodes, mut sampled) = (0, 0);
    for (n, (id, _)) in (1..).zip(spread(series.sample_from(), 3)) {
        let mut url = client.api_url(Some("get_series_info"))?;
        url.query_pairs_mut()
            .append_pair("series_id", &id.to_string());
        match get_json(&client, url).await {
            Err(e) => println!("get_series_info {n}: {}", redact(&e)),
            Ok((v, bytes, ms)) => {
                println!("get_series_info {n}: {} in {ms} ms", size(bytes));
                let (count, first) = series_info(&v, &live_ids, &movie_ids, &series.ids);
                episodes += count;
                sampled += 1;
                if episode_stream.is_none() {
                    episode_stream = first;
                }
            }
        }
    }
    if sampled > 0 {
        println!(
            "episodes: {:.0} per series sampled, about {:.0}k over all series",
            episodes as f64 / sampled as f64,
            episodes as f64 / sampled as f64 * series.count as f64 / 1000.0
        );
    }

    // 4. Streams, ranges and a download, outside the client's gate as mpv
    // is (SPEC 6.2), spaced as if under it.
    let plain = reqwest::Client::builder()
        .user_agent(concat!("vidicon/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let follow = http();
    if let Some((id, ext)) = &movie_stream {
        let url = client.vod_url("movie", *id, ext)?;
        println!("movie stream (.{ext}):");
        stream(&plain, &follow, &url, own.as_deref()).await;
        download(&client, &follow, &url, cons_before).await;
        tokio::time::sleep(GAP).await;
        match mpv_seek(mpv, &url, true).await {
            Ok(r) => println!("  mpv: {r}"),
            Err(e) => println!("  mpv: {}", redact(&e)),
        }
        tokio::time::sleep(GAP).await;
    }
    if let Some((id, ext)) = &episode_stream {
        let url = client.vod_url("series", *id, ext)?;
        println!("episode stream (.{ext}):");
        stream(&plain, &follow, &url, own.as_deref()).await;
        match mpv_seek(mpv, &url, false).await {
            Ok(r) => println!("  mpv: {r}"),
            Err(e) => println!("  mpv: {}", redact(&e)),
        }
        tokio::time::sleep(GAP).await;
    }

    // 5. Posters.
    posters(&client, "movie posters", &movies.samples, own.as_deref()).await;
    posters(&client, "series covers", &series.samples, own.as_deref()).await;
    Ok(())
}

/// Fetches a category list and prints its shape, how its ids meet the live
/// ones, and how many the rules show.
async fn categories(
    client: &Client,
    action: &str,
    live: &HashMap<String, String>,
    rules: &[String],
) -> Result<Categories, String> {
    let (v, bytes, ms) = get_json(client, client.api_url(Some(action))?).await?;
    let Some(list) = v.as_array() else {
        println!("{action}: not a list but {}", shape(&v));
        return Ok(HashMap::new());
    };
    let mut fields = Fields::new();
    let mut cats = Categories::new();
    for c in list {
        if let Some(o) = c.as_object() {
            tally(&mut fields, o);
        }
        if let (Some(id), Some(name)) = (
            c.get("category_id").and_then(text),
            c["category_name"].as_str(),
        ) {
            let shown = store::matches_rules(rules, name);
            cats.insert(id, (name.to_owned(), shown));
        }
    }
    let shared: Vec<_> = cats.keys().filter(|k| live.contains_key(*k)).collect();
    let same = shared.iter().filter(|k| live[**k] == cats[**k].0).count();
    println!(
        "{action}: {} categories, {} in {ms} ms; {} shown by the rules; ids shared with live {}, same name {same}",
        list.len(),
        size(bytes),
        cats.values().filter(|c| c.1).count(),
        shared.len(),
    );
    print_fields("  ", &fields);
    Ok(cats)
}

/// What a movie or series list holds, gathered as it streams in.
struct Tally {
    id_field: &'static str,
    icon_field: &'static str,
    own: Option<String>,
    cats: Categories,
    count: usize,
    fields: Fields,
    ids: Vec<i64>,
    per_cat: HashMap<String, usize>,
    unknown_cat: usize,
    /// Ids and extensions of titles in shown categories.
    shown_titles: Vec<(i64, String)>,
    /// The same for every title not marked adult, to sample from when the
    /// rules show none.
    titles: Vec<(i64, String)>,
    exts: BTreeMap<String, usize>,
    hosts: HashMap<String, usize>,
    schemes: BTreeMap<String, usize>,
    no_icon: usize,
    icons: HashSet<u64>,
    /// Every 500th poster URL, for fetching.
    samples: Vec<String>,
    name_bytes: usize,
}

impl Tally {
    fn new(
        id_field: &'static str,
        icon_field: &'static str,
        own: Option<String>,
        cats: Categories,
    ) -> Self {
        Self {
            id_field,
            icon_field,
            own,
            cats,
            count: 0,
            fields: Fields::new(),
            ids: Vec::new(),
            per_cat: HashMap::new(),
            unknown_cat: 0,
            shown_titles: Vec::new(),
            titles: Vec::new(),
            exts: BTreeMap::new(),
            hosts: HashMap::new(),
            schemes: BTreeMap::new(),
            no_icon: 0,
            icons: HashSet::new(),
            samples: Vec::new(),
            name_bytes: 0,
        }
    }

    /// Titles in shown categories, or all but adult ones if none are.
    fn sample_from(&self) -> &[(i64, String)] {
        if self.shown_titles.is_empty() {
            &self.titles
        } else {
            &self.shown_titles
        }
    }

    fn add(&mut self, v: Value) {
        self.count += 1;
        let Some(o) = v.as_object() else {
            return;
        };
        tally(&mut self.fields, o);
        let id = o.get(self.id_field).and_then(int);
        if let Some(id) = id {
            self.ids.push(id);
        }
        let ext = o.get("container_extension").and_then(text);
        if let Some(e) = &ext {
            *self.exts.entry(e.clone()).or_default() += 1;
        }
        if let (Some(id), false) = (id, o.get("is_adult").and_then(int) == Some(1)) {
            self.titles.push((id, ext.clone().unwrap_or_default()));
        }
        match o.get("category_id").and_then(text) {
            Some(c) if self.cats.contains_key(&c) => {
                if let (true, Some(id)) = (self.cats[&c].1, id) {
                    self.shown_titles.push((id, ext.unwrap_or_default()));
                }
                *self.per_cat.entry(c).or_default() += 1;
            }
            _ => self.unknown_cat += 1,
        }
        match o.get(self.icon_field).and_then(text) {
            None => self.no_icon += 1,
            Some(icon) => {
                let host = match Url::parse(&icon) {
                    Ok(u) => {
                        *self.schemes.entry(u.scheme().to_owned()).or_default() += 1;
                        u.host_str().unwrap_or("").to_owned()
                    }
                    Err(_) => "(not a URL)".into(),
                };
                *self.hosts.entry(host).or_default() += 1;
                let mut h = DefaultHasher::new();
                icon.hash(&mut h);
                if self.icons.insert(h.finish()) && self.icons.len() % 500 == 1 {
                    self.samples.push(icon);
                }
            }
        }
        self.name_bytes += o.get("name").and_then(Value::as_str).map_or(0, str::len);
    }

    fn print(
        &self,
        action: &str,
        bytes: usize,
        ms: u128,
        live: &HashSet<i64>,
        movies: &HashSet<i64>,
    ) {
        println!(
            "{action}: {} titles, {} in {ms} ms",
            self.count,
            size(bytes)
        );
        print_fields("  ", &self.fields);
        let distinct: HashSet<i64> = self.ids.iter().copied().collect();
        println!(
            "  ids: {} with {}, {} distinct, {}..{}; same as a live stream_id {}, as a movie's {}",
            self.ids.len(),
            self.id_field,
            distinct.len(),
            opt(distinct.iter().min().copied()),
            opt(distinct.iter().max().copied()),
            distinct.intersection(live).count(),
            distinct.intersection(movies).count(),
        );
        let shown_cats = self.per_cat.keys().filter(|c| self.cats[*c].1).count();
        let mut sizes: Vec<usize> = self.per_cat.values().copied().collect();
        sizes.sort_unstable();
        println!(
            "  categories: titles in {} of {}, {} titles in none listed; shown by the rules: {} titles in {shown_cats} categories; largest category {}, median {}",
            self.per_cat.len(),
            self.cats.len(),
            self.unknown_cat,
            self.shown_titles.len(),
            opt(sizes.last().copied()),
            opt(sizes.get(sizes.len() / 2).copied()),
        );
        if !self.exts.is_empty() {
            println!("  container_extension: {}", counts(&self.exts));
        }
        let mut hosts: Vec<(&String, &usize)> = self.hosts.iter().collect();
        hosts.sort_by(|a, b| b.1.cmp(a.1));
        let named: Vec<String> = hosts
            .iter()
            .zip(b'A'..=b'Z')
            .map(|((h, n), l)| {
                if Some(h.as_str()) == self.own.as_deref() {
                    format!("provider's own {n}")
                } else if h.starts_with('(') || h.is_empty() {
                    format!("{h} {n}")
                } else {
                    format!("{} {n}", l as char)
                }
            })
            .collect();
        println!(
            "  {}: none {}, {} distinct URLs on {} hosts: {}; schemes {}",
            self.icon_field,
            self.no_icon,
            self.icons.len(),
            hosts.len(),
            named.join(", "),
            counts(&self.schemes),
        );
        println!(
            "  names: {:.1} MiB, {:.0} bytes on average",
            self.name_bytes as f64 / 1_048_576.0,
            self.name_bytes as f64 / self.count.max(1) as f64
        );
    }
}

/// Visits a JSON list element by element, so the body is never whole in memory.
struct Each<'a>(&'a mut Tally);

impl<'de> Visitor<'de> for Each<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a list")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(v) = seq.next_element::<Value>()? {
            self.0.add(v);
        }
        Ok(())
    }
}

/// Streams `action`'s list through `t`. Returns it with the body's size and
/// time, the gate's wait left out.
async fn list(client: &Client, action: &str, mut t: Tally) -> Result<(Tally, usize, u128), String> {
    let url = client.api_url(Some(action))?;
    let (tx, rx) = mpsc::channel(16);
    let job = tokio::task::spawn_blocking(move || {
        let mut de = serde_json::Deserializer::from_reader(BufReader::with_capacity(
            64 * 1024,
            ChunkReader::new(rx),
        ));
        // Position only: serde's message can quote values from the body.
        let r = de
            .deserialize_seq(Each(&mut t))
            .and_then(|()| de.end())
            .map_err(|e| format!("not a list, at line {} column {}", e.line(), e.column()));
        (t, r)
    });
    tokio::time::sleep(GAP).await;
    let start = Instant::now();
    let fetched = client.fetch(url, tx).await;
    let ms = start.elapsed().as_millis();
    let (t, parsed) = job.await.map_err(|_| "parse task failed")?;
    let bytes = fetched?;
    parsed.map_err(|e| format!("{action}: {e}"))?;
    Ok((t, bytes, ms))
}

/// A small body through the gate, parsed. With its size and time.
async fn get_json(client: &Client, url: Url) -> Result<(Value, usize, u128), String> {
    let (tx, mut rx) = mpsc::channel::<crate::xtream::Chunk>(16);
    let collect = async {
        let mut body = Vec::new();
        while let Some(Ok(chunk)) = rx.recv().await {
            body.extend_from_slice(&chunk);
        }
        body
    };
    tokio::time::sleep(GAP).await;
    let start = Instant::now();
    let (fetched, body) = tokio::join!(client.fetch(url, tx), collect);
    let ms = start.elapsed().as_millis();
    fetched?;
    let v = serde_json::from_slice(&body)
        .map_err(|e| format!("not JSON, at line {} column {}", e.line(), e.column()))?;
    Ok((v, body.len(), ms))
}

/// A series' info: its shape, its episodes' ids against the other kinds'.
/// Returns the episode count and the first episode's id and extension.
fn series_info(
    v: &Value,
    live: &HashSet<i64>,
    movies: &HashSet<i64>,
    series: &[i64],
) -> (usize, Option<(i64, String)>) {
    let Some(o) = v.as_object() else {
        println!("  not an object but {}", shape(v));
        return (0, None);
    };
    println!("  keys {}", keys(o));
    if let Some(info) = o.get("info").and_then(Value::as_object) {
        println!("  info: {}", details(info));
    }
    if let Some(seasons) = o.get("seasons") {
        let n = seasons.as_array().map_or(0, Vec::len);
        let first = seasons
            .as_array()
            .and_then(|a| a.first())
            .and_then(Value::as_object)
            .map_or("-".into(), details);
        println!("  seasons: {}, {n} long; first: {first}", shape(seasons));
    }
    let lists: Vec<(String, &Vec<Value>)> = match o.get("episodes") {
        Some(Value::Object(m)) => m
            .iter()
            .filter_map(|(k, v)| {
                let k = if k.parse::<i64>().is_ok() { k } else { "text" };
                Some((format!("key {k}"), v.as_array()?))
            })
            .collect(),
        Some(Value::Array(a)) => a
            .iter()
            .enumerate()
            .filter_map(|(i, v)| Some((format!("index {i}"), v.as_array()?)))
            .collect(),
        other => {
            println!("  episodes: {}", other.map_or("missing", shape));
            return (0, None);
        }
    };
    println!(
        "  episodes: {} by season, {} seasons: {}",
        shape(&o["episodes"]),
        lists.len(),
        lists
            .iter()
            .map(|(k, l)| format!("{k} {}", l.len()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let all: Vec<&Map<String, Value>> = lists
        .iter()
        .flat_map(|(_, l)| l.iter().filter_map(Value::as_object))
        .collect();
    if let Some(e) = all.first() {
        println!("  first episode: {}", details(e));
        if let Some(info) = e.get("info").and_then(Value::as_object) {
            println!("  its info: {}", details(info));
        }
    }
    let ids: HashSet<i64> = all
        .iter()
        .filter_map(|e| e.get("id").and_then(int))
        .collect();
    let mut exts = BTreeMap::new();
    for e in &all {
        if let Some(x) = e.get("container_extension").and_then(text) {
            *exts.entry(x).or_insert(0) += 1;
        }
    }
    let series: HashSet<i64> = series.iter().copied().collect();
    println!(
        "  episode ids: {} of {} episodes, same as a live stream_id {}, a movie's {}, a series' {}; container_extension {}",
        ids.len(),
        all.len(),
        ids.intersection(live).count(),
        ids.intersection(movies).count(),
        ids.intersection(&series).count(),
        counts(&exts),
    );
    let first = all.first().and_then(|e| {
        Some((
            e.get("id").and_then(int)?,
            e.get("container_extension").and_then(text)?,
        ))
    });
    (all.len(), first)
}

/// Redirects, then ranges at the start and the middle.
async fn stream(plain: &reqwest::Client, follow: &reqwest::Client, url: &Url, own: Option<&str>) {
    match plain.get(url.clone()).send().await {
        Err(e) => println!("  no redirect followed: {}", e.without_url()),
        Ok(r) => {
            let to = r
                .headers()
                .get(header::LOCATION)
                .and_then(|l| url.join(l.to_str().ok()?).ok());
            let host = to.as_ref().map(|u| {
                if u.host_str() == own {
                    "the same host"
                } else {
                    "another host"
                }
            });
            println!(
                "  first answer: HTTP {}{}",
                r.status(),
                host.map_or(String::new(), |h| format!(", to {h}"))
            );
        }
    }
    tokio::time::sleep(GAP).await;
    let total = range(follow, url, 0).await;
    tokio::time::sleep(GAP).await;
    if let Some(total) = total {
        range(follow, url, total / 2).await;
        tokio::time::sleep(GAP).await;
    }
}

/// Asks for 1 KiB at `from`; prints the answer, returns the full size.
async fn range(follow: &reqwest::Client, url: &Url, from: u64) -> Option<u64> {
    let start = Instant::now();
    let r = follow
        .get(url.clone())
        .header(header::RANGE, format!("bytes={from}-{}", from + 1023))
        .timeout(Duration::from_secs(30))
        .send()
        .await;
    let mut r = match r {
        Ok(r) => r,
        Err(e) => {
            println!("  range at {from}: {}", e.without_url());
            return None;
        }
    };
    let first = start.elapsed().as_millis();
    let h = |k| {
        r.headers()
            .get(k)
            .and_then(|v: &header::HeaderValue| v.to_str().ok())
            .map(str::to_owned)
    };
    let (status, kind, accept, content_range, length) = (
        r.status(),
        h(header::CONTENT_TYPE),
        h(header::ACCEPT_RANGES),
        h(header::CONTENT_RANGE),
        h(header::CONTENT_LENGTH),
    );
    let mut got = 0;
    while let Ok(Some(c)) = r.chunk().await {
        got += c.len();
        if got >= 64 * 1024 {
            break;
        }
    }
    let total = content_range
        .as_deref()
        .and_then(|c| c.rsplit('/').next()?.parse().ok());
    println!(
        "  range at {}: HTTP {status} in {first} ms, {got} bytes read, type {}, accept-ranges {}, content-range total {}, content-length {}",
        size(from as usize),
        kind.as_deref().unwrap_or("-"),
        accept.as_deref().unwrap_or("-"),
        total.map_or("-".into(), |t: u64| size(t as usize)),
        length.as_deref().unwrap_or("-"),
    );
    total
}

/// Downloads for up to 20 s; 5 s in, asks the account how many connections
/// are open.
async fn download(client: &Client, follow: &reqwest::Client, url: &Url, before: Option<i64>) {
    let start = Instant::now();
    let get = async {
        let mut r = follow.get(url.clone()).send().await?;
        let status = r.status();
        let mut got = 0usize;
        while start.elapsed() < Duration::from_secs(20) && got < 1 << 30 {
            match r.chunk().await? {
                Some(c) => got += c.len(),
                None => break,
            }
        }
        Ok::<_, reqwest::Error>((status, got))
    };
    let during = async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        client.account().await.map(|a| a.user_info.active_cons)
    };
    let (got, during) = tokio::join!(get, during);
    let secs = start.elapsed().as_secs_f64();
    match got {
        Err(e) => println!("  download: {}", e.without_url()),
        Ok((status, got)) => println!(
            "  download: HTTP {status}, {} in {secs:.1} s, {:.1} MiB/s",
            size(got),
            got as f64 / 1_048_576.0 / secs
        ),
    }
    println!(
        "  active_cons before {}, during the download {}",
        opt(before),
        during.map_or_else(|e| redact(&e), opt)
    );
    tokio::time::sleep(GAP).await;
}

/// Fetches up to 8 of `urls`, spread through them, as logos are fetched:
/// the provider's own through its gate, the rest spaced by the same gap.
async fn posters(client: &Client, what: &str, urls: &[String], own: Option<&str>) {
    let http = http();
    let picks: Vec<&String> = (0..8)
        .filter_map(|i| urls.get(i * urls.len() / 8))
        .collect::<Vec<_>>();
    println!("{what}: fetching {} of {} sampled", picks.len(), urls.len());
    for (n, u) in (1..).zip(picks) {
        let Ok(url) = Url::parse(u) else {
            println!("  {n}: not a URL");
            continue;
        };
        let own_host = url.host_str() == own;
        let start = Instant::now();
        let r = if own_host {
            client.logo(url, 8 << 20).await
        } else {
            get_small(&http, url, 8 << 20).await
        };
        let ms = start.elapsed().as_millis();
        if !own_host {
            tokio::time::sleep(GAP).await;
        }
        let whose = if own_host { "own host" } else { "third party" };
        match r {
            Err(e) => println!("  {n} ({whose}): {}", redact(&e)),
            Ok((status, body)) => {
                let reader = image::ImageReader::new(std::io::Cursor::new(&body))
                    .with_guessed_format()
                    .ok();
                let format = reader.as_ref().and_then(|r| r.format());
                let dims = reader.and_then(|r| r.into_dimensions().ok());
                println!(
                    "  {n} ({whose}): HTTP {status}, {} in {ms} ms, {}{}",
                    size(body.len()),
                    format.map_or("unknown format".into(), |f| format!("{f:?}")),
                    dims.map_or(String::new(), |(w, h)| format!(" {w}x{h}")),
                );
            }
        }
    }
}

/// The commonest shapes of the names' first 6 characters, letters as `A`
/// and digits as `9`.
fn prefixes(names: &[String]) -> String {
    let mut m: HashMap<String, usize> = HashMap::new();
    for n in names {
        *m.entry(pattern(&n.chars().take(6).collect::<String>()))
            .or_default() += 1;
    }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    v.iter()
        .take(8)
        .map(|(p, n)| format!("\"{p}\" {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Letters as `A`, digits as `9`, the rest as it is.
fn pattern(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            c if c.is_alphabetic() => 'A',
            c if c.is_numeric() => '9',
            c => c,
        })
        .collect()
}

/// The first run of letters and digits, folded.
fn first_word(s: &str) -> Option<String> {
    store::fold(s)
        .split(|c: char| !c.is_alphanumeric())
        .find(|w| !w.is_empty())
        .map(str::to_owned)
}

/// Up to `n` items spread through `v`.
fn spread(v: &[(i64, String)], n: usize) -> Vec<(i64, String)> {
    (1..=n)
        .filter_map(|i| v.get(i * v.len() / (n + 1)).cloned())
        .collect()
}

fn tally(fields: &mut Fields, o: &Map<String, Value>) {
    for (k, v) in o {
        *fields
            .entry(k.clone())
            .or_default()
            .entry(shape(v))
            .or_default() += 1;
    }
}

fn print_fields(indent: &str, fields: &Fields) {
    for (k, shapes) in fields {
        println!("{indent}{k}: {}", counts(shapes));
    }
}

/// An info response: its keys, then each object's fields in detail.
fn print_object(indent: &str, v: &Value) {
    let Some(o) = v.as_object() else {
        return println!("{indent}not an object but {}", shape(v));
    };
    println!("{indent}keys {}", keys(o));
    for (k, v) in o {
        if let Some(inner) = v.as_object() {
            println!("{indent}{k}: {}", details(inner));
        }
    }
}

fn keys(o: &Map<String, Value>) -> String {
    o.keys().cloned().collect::<Vec<_>>().join(", ")
}

/// Each field's shape; short strings of digits and punctuation (dates,
/// lengths, ratings) as their pattern, other strings by length only.
fn details(o: &Map<String, Value>) -> String {
    o.iter()
        .map(|(k, v)| {
            let d = match v {
                Value::String(s) if !s.is_empty() && s.len() <= 25 && digits(s) != "text" => {
                    format!("\"{}\"", digits(s))
                }
                Value::String(s) if !s.is_empty() => format!("string({})", s.chars().count()),
                Value::Number(n) => format!("number {}", digits(&n.to_string())),
                Value::Array(a) => format!("array({})", a.len()),
                other => shape(other).to_owned(),
            };
            format!("{k} {d}")
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// `s` with every digit as 9, or `text` if it has letters.
fn digits(s: &str) -> String {
    if s.chars().any(char::is_alphabetic) {
        return "text".into();
    }
    s.chars()
        .map(|c| if c.is_ascii_digit() { '9' } else { c })
        .collect()
}

fn shape(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(s) if s.is_empty() => "empty string",
        Value::String(s) if s.trim().parse::<f64>().is_ok() => "numeric string",
        Value::String(_) => "string",
        Value::Array(a) if a.is_empty() => "empty array",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// A string or number as text; empty is none.
fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn counts<K: fmt::Display>(m: &BTreeMap<K, usize>) -> String {
    m.iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn opt(v: Option<impl fmt::Display>) -> String {
    v.map_or("-".into(), |v| v.to_string())
}

fn size(bytes: usize) -> String {
    match bytes {
        ..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.0} KiB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MiB", bytes as f64 / 1_048_576.0),
    }
}
