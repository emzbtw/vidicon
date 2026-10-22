//! Fuzzy search (SPEC 8.5; ADR 0003 amendment): channel, movie and series
//! names held in memory, one index per kind, matched with nucleo against
//! the current view's row keys.

use crate::store::{self, Kind};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str, chars};
use rusqlite::Connection;
use unicode_normalization::UnicodeNormalization;

/// One kind's names by `rowid`, for one provider. Rebuilt after each import
/// of that kind, since an import replaces the rows.
///
/// Laid out to be read in order (VOD ticket 09): over 180k movie names a
/// map of separately allocated names, cold between keystrokes, took two
/// to three times as long to match.
pub struct Index {
    kind: Kind,
    /// The lowest row key: key `k`'s name is in slot `k - first`. An import
    /// numbers a provider's rows of a kind in one run, so slots are rarely
    /// empty.
    first: i64,
    slots: Vec<Slot>,
    /// The ASCII names end to end, and the rest as nucleo matches them.
    ascii: Vec<u8>,
    unicode: Vec<char>,
    len: usize,
}

/// Where a name is in [`Index`]'s text.
#[derive(Clone, Copy, Default)]
struct Slot {
    /// Its [`letters`].
    letters: u64,
    start: u32,
    len: u16,
    unicode: bool,
    present: bool,
}

/// Views smaller than this are scored on the calling thread alone.
const PARALLEL_FROM: usize = 4096;

impl Index {
    pub fn build(conn: &Connection, provider_id: &str, kind: Kind) -> rusqlite::Result<Self> {
        let mut index = Self::new(kind);
        store::names(conn, provider_id, kind, |key, name| index.insert(key, name))?;
        Ok(index)
    }

    fn new(kind: Kind) -> Self {
        Self {
            kind,
            first: 0,
            slots: Vec::new(),
            ascii: Vec::new(),
            unicode: Vec::new(),
            len: 0,
        }
    }

    /// Names are NFKC-folded, so `uhd` finds `ᵁᴴᴰ` (SPEC 5 `name_norm`).
    fn insert(&mut self, key: i64, name: &str) {
        let folded: String = name.nfkc().collect();
        // As nucleo folds the name's chars when it matches.
        let letters = folded
            .chars()
            .flat_map(|c| {
                let lower = chars::to_lower_case(c);
                [c, lower, chars::normalize(c), chars::normalize(lower)]
            })
            .fold(0, |set, c| set | letter(c));
        // As `Utf32String::from` holds it: a grapheme's first char.
        let (start, len, unicode) = if folded.is_ascii() {
            let start = self.ascii.len();
            self.ascii.extend(folded.bytes().take(u16::MAX.into()));
            (start, self.ascii.len() - start, false)
        } else {
            let start = self.unicode.len();
            self.unicode
                .extend(chars::graphemes(&folded).take(u16::MAX.into()));
            (start, self.unicode.len() - start, true)
        };
        if self.slots.is_empty() {
            self.first = key;
        } else if key < self.first {
            let more = (self.first - key) as usize;
            self.slots
                .splice(0..0, std::iter::repeat_n(Slot::default(), more));
            self.first = key;
        }
        let at = (key - self.first) as usize;
        if at >= self.slots.len() {
            self.slots.resize(at + 1, Slot::default());
        }
        if !self.slots[at].present {
            self.len += 1;
        }
        self.slots[at] = Slot {
            letters,
            start: start as u32,
            len: len as u16,
            unicode,
            present: true,
        };
    }

    /// Key `key`'s letters and name, if it has one.
    fn get(&self, key: i64) -> Option<(u64, Utf32Str<'_>)> {
        let at = usize::try_from(key.checked_sub(self.first)?).ok()?;
        let slot = self.slots.get(at).filter(|s| s.present)?;
        let range = slot.start as usize..slot.start as usize + slot.len as usize;
        let name = match slot.unicode {
            false => Utf32Str::Ascii(&self.ascii[range]),
            true => Utf32Str::Unicode(&self.unicode[range]),
        };
        Some((slot.letters, name))
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

/// A provider's indexes, those built so far, and the matchers that score
/// them.
pub struct Search {
    indexes: [Option<Index>; 3],
    /// One per thread that scores a share of the names.
    matchers: Vec<Matcher>,
}

/// One kind's matches when search is widened to all kinds (VOD ticket 09).
#[derive(Debug, PartialEq)]
pub struct Group {
    pub kind: Kind,
    /// The best, as [`store::list_key`]s, at most the cap.
    pub keys: Vec<i64>,
    /// How many matched.
    pub total: usize,
}

/// A key's score, its position in the view, and the key.
type Scored = (u32, usize, i64);

impl Search {
    pub fn new() -> Self {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
        Self {
            indexes: [None, None, None],
            matchers: (0..threads)
                .map(|_| Matcher::new(Config::DEFAULT))
                .collect(),
        }
    }

    pub fn set(&mut self, index: Index) {
        let kind = index.kind;
        self.indexes[kind as usize] = Some(index);
    }

    /// Drops `kind`'s index, e.g. as an import replaces its rows.
    pub fn drop_kind(&mut self, kind: Kind) {
        self.indexes[kind as usize] = None;
    }

    /// The keys among `keys`, row keys of `kind`, whose names match
    /// `query`, best match first; equal scores keep `keys`' order. `keys` is
    /// the current view, so the search stays inside its category. `None`
    /// until `kind`'s index is built.
    pub fn filter(&mut self, kind: Kind, keys: &[i64], query: &str) -> Option<Vec<i64>> {
        let index = self.indexes[kind as usize].as_ref()?;
        let pattern = pattern(query);
        Some(ranked(score(&mut self.matchers, &pattern, keys, |k| {
            index.get(k)
        })))
    }

    /// As [`Search::filter`], for a list of several kinds: `keys` are
    /// [`store::list_key`]s, each matched against its own kind's names,
    /// best match first across kinds. `None` until each kind the list holds
    /// is indexed.
    pub fn filter_mixed(&mut self, keys: &[i64], query: &str) -> Option<Vec<i64>> {
        let mut held = [false; 3];
        for &key in keys {
            held[store::from_list_key(key).0 as usize] = true;
        }
        let indexes = &self.indexes;
        if (0..3).any(|k| held[k] && indexes[k].is_none()) {
            return None;
        }
        let pattern = pattern(query);
        Some(ranked(score(&mut self.matchers, &pattern, keys, |key| {
            let (kind, row) = store::from_list_key(key);
            indexes[kind as usize].as_ref()?.get(row)
        })))
    }

    /// Search widened to all kinds: `keys`, [`store::list_key`]s of each
    /// kind's All, filtered as [`Search::filter_mixed`] does and grouped by
    /// kind, live first, each group cut to its best `cap`. One pass over
    /// all of them, not one per kind.
    pub fn widen(&mut self, keys: &[i64], query: &str, cap: usize) -> Option<Vec<Group>> {
        let mut groups: Vec<Group> = Kind::ALL
            .into_iter()
            .map(|kind| Group {
                kind,
                keys: Vec::new(),
                total: 0,
            })
            .collect();
        for key in self.filter_mixed(keys, query)? {
            let group = &mut groups[store::from_list_key(key).0 as usize];
            group.total += 1;
            if group.keys.len() < cap {
                group.keys.push(key);
            }
        }
        Some(groups)
    }
}

fn pattern(query: &str) -> Pattern {
    Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart)
}

/// The ASCII letters and digits a name or query holds, as a bit each;
/// other chars hold none. A name lacking a letter the query needs cannot
/// match, and this rules it out far faster than nucleo can (VOD ticket 09,
/// SPEC 8.5).
fn letters(pattern: &Pattern) -> u64 {
    pattern
        .atoms
        .iter()
        .filter(|a| !a.negative)
        .flat_map(|a| a.needle_text().chars().collect::<Vec<_>>())
        .fold(0, |set, c| set | letter(c))
}

fn letter(c: char) -> u64 {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => 1 << (c as u32 - 'a' as u32),
        c @ '0'..='9' => 1 << (26 + c as u32 - '0' as u32),
        _ => 0,
    }
}

/// Best first; equal scores keep their positions' order. [`score`] hands
/// over sorted runs, which a stable sort merges.
fn ranked(mut scored: Vec<Scored>) -> Vec<i64> {
    scored.sort_by(best_first);
    scored.into_iter().map(|(_, _, key)| key).collect()
}

fn best_first(a: &Scored, b: &Scored) -> std::cmp::Ordering {
    b.0.cmp(&a.0).then(a.1.cmp(&b.1))
}

/// The keys among `keys` that match, split across `matchers`' threads for
/// long views, each thread's share sorted [`best_first`]. `name` reads a
/// key's letters and name; keys it does not know (e.g. a view read after
/// an import the index has not caught up with) are skipped.
fn score<'a>(
    matchers: &mut [Matcher],
    pattern: &Pattern,
    keys: &[i64],
    name: impl Fn(i64) -> Option<(u64, Utf32Str<'a>)> + Sync,
) -> Vec<Scored> {
    let needs = letters(pattern);
    // Scores `keys[from..]` chunk; `from` keeps positions view-wide.
    let score = |from: usize, chunk: &[i64], matcher: &mut Matcher| {
        let mut scored = chunk
            .iter()
            .enumerate()
            .filter_map(|(i, key)| {
                let (has, name) = name(*key)?;
                if needs & !has != 0 {
                    return None;
                }
                let score = pattern.score(name, matcher)?;
                Some((score, from + i, *key))
            })
            .collect::<Vec<_>>();
        scored.sort_unstable_by(best_first);
        scored
    };
    if keys.len() < PARALLEL_FROM || matchers.len() == 1 {
        return score(0, keys, &mut matchers[0]);
    }
    let size = keys.len().div_ceil(matchers.len());
    std::thread::scope(|scope| {
        let score = &score;
        let parts: Vec<_> = keys
            .chunks(size)
            .zip(matchers)
            .enumerate()
            .map(|(n, (chunk, matcher))| scope.spawn(move || score(n * size, chunk, matcher)))
            .collect();
        parts
            .into_iter()
            .flat_map(|p| p.join().expect("search thread"))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Channels named `names`, keyed by position.
    fn index(names: &[&str]) -> Search {
        let mut search = Search::new();
        search.set(names_of(Kind::Live, names));
        search
    }

    fn names_of(kind: Kind, names: &[&str]) -> Index {
        let mut index = Index::new(kind);
        for (i, name) in names.iter().enumerate() {
            index.insert(i as i64, name);
        }
        index
    }

    fn all(search: &Search) -> Vec<i64> {
        (0..search.indexes[0].as_ref().unwrap().len() as i64).collect()
    }

    impl Search {
        fn live(&mut self, keys: &[i64], query: &str) -> Vec<i64> {
            self.filter(Kind::Live, keys, query).unwrap()
        }
    }

    #[test]
    fn best_match_first() {
        let mut ix = index(&[
            "UK: Kestrel Xtra Network",
            "US: AQN News",
            "UK: KXN One HD",
            "UK: KXN News",
        ]);
        let keys = all(&ix);
        let found = ix.live(&keys, "kxn");
        // The contiguous matches beat the scattered one, which still matches.
        assert_eq!(found[..2], [2, 3]);
        assert_eq!(found.last(), Some(&0));
        assert!(!found.contains(&1));
        assert_eq!(ix.live(&keys, "kxn news"), [3]);
    }

    #[test]
    fn ties_keep_list_order() {
        let mut ix = index(&[
            "Northwind Sports 1",
            "Northwind Sports 2",
            "Northwind Sports 3",
        ]);
        assert_eq!(ix.live(&[2, 0, 1], "northwind sports"), [2, 0, 1]);
    }

    #[test]
    fn scoped_to_the_given_keys() {
        let mut ix = index(&["KXN One", "KXN Two", "Juniper", "KXN Four"]);
        // A category holding KXN Two and Juniper only.
        assert_eq!(ix.live(&[1, 2], "kxn"), [1]);
        assert!(ix.live(&[2], "kxn").is_empty());
        // Keys the index does not know (e.g. a view read after an import the
        // index has not caught up with) are skipped, not a panic.
        assert_eq!(ix.live(&[99, 0], "kxn"), [0]);
    }

    #[test]
    fn case_and_superscripts_fold() {
        let mut ix = index(&["4K: NORTHWIND SPORTS ᴾᴸ ᵁᴴᴰ ³⁸⁴⁰ᵖ", "Café Olé"]);
        let keys = all(&ix);
        assert_eq!(ix.live(&keys, "northwind uhd 3840p"), [0]);
        assert_eq!(ix.live(&keys, "cafe"), [1]);
    }

    /// VOD ticket 09: a Movies or Series category searches its kind's
    /// names, folded as channels' are.
    #[test]
    fn each_kind_searches_its_own_names() {
        let mut ix = Search::new();
        ix.set(names_of(Kind::Live, &["Kestrel News"]));
        ix.set(names_of(Kind::Movie, &["The Kestrel Affair", "Juniper ᴴᴰ"]));
        // Row keys collide across kinds; the kind says whose names.
        assert_eq!(ix.filter(Kind::Movie, &[0, 1], "kestrel"), Some(vec![0]));
        assert_eq!(ix.filter(Kind::Movie, &[0, 1], "juniper hd"), Some(vec![1]));
        assert_eq!(ix.filter(Kind::Live, &[0], "kestrel"), Some(vec![0]));
        // Series are not indexed yet.
        assert_eq!(ix.filter(Kind::Series, &[0], "kestrel"), None);
        ix.drop_kind(Kind::Movie);
        assert_eq!(ix.filter(Kind::Movie, &[0], "kestrel"), None);
    }

    /// Favourites, Recently played and Continue watching hold list keys
    /// of several kinds: each matched against its kind, best first across
    /// kinds, ties in list order.
    #[test]
    fn mixed_lists_match_each_key_by_its_kind() {
        let mut ix = Search::new();
        ix.set(names_of(Kind::Live, &["Kestrel Xtra News", "Juniper"]));
        ix.set(names_of(Kind::Movie, &["Kestrel", "KXN"]));
        let movie = |k| store::list_key(Kind::Movie, k);
        let keys = [0, movie(0), 1, movie(1)];
        // The movie KXN beats the channel, a scattered match listed first.
        assert_eq!(ix.filter_mixed(&keys, "kxn"), Some(vec![movie(1), 0]));
        assert_eq!(ix.filter_mixed(&keys, "juniper"), Some(vec![1]));
        // A series favourite needs the series index.
        let series = store::list_key(Kind::Series, 0);
        assert_eq!(ix.filter_mixed(&[0, series], "kxn"), None);
        // An unavailable favourite's negative key matches nothing.
        assert_eq!(ix.filter_mixed(&[-5, 0], "kxn"), Some(vec![0]));
    }

    /// Tab widens to all kinds: live first, each group best first and cut
    /// to the cap, with how many matched.
    #[test]
    fn widening_groups_by_kind_and_caps_each() {
        let mut ix = Search::new();
        ix.set(names_of(
            Kind::Live,
            &["Harbour One", "Juniper", "Harbour Two"],
        ));
        ix.set(names_of(
            Kind::Movie,
            &["Harbour", "Harbour Lights", "The Harbour", "Dune"],
        ));
        ix.set(names_of(Kind::Series, &["Juniper Bay"]));
        let movie = |k| store::list_key(Kind::Movie, k);
        let views = [
            0,
            1,
            2,
            movie(0),
            movie(1),
            movie(2),
            movie(3),
            store::list_key(Kind::Series, 0),
        ];
        let groups = ix.widen(&views, "harbour", 2).unwrap();
        assert_eq!(
            groups,
            [
                Group {
                    kind: Kind::Live,
                    keys: vec![0, 2],
                    total: 2
                },
                Group {
                    kind: Kind::Movie,
                    keys: vec![movie(0), movie(1)],
                    total: 3
                },
                Group {
                    kind: Kind::Series,
                    keys: vec![],
                    total: 0
                },
            ]
        );
        ix.drop_kind(Kind::Series);
        assert!(ix.widen(&views, "harbour", 2).is_none());
    }

    /// Movies and series are indexed from their own tables.
    #[test]
    fn builds_movie_and_series_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = store::open(&dir.path().join("v.db")).unwrap();
        let tx = conn.transaction().unwrap();
        store::begin_import(&tx, "p", "P", Kind::Movie, []).unwrap();
        for (id, name) in [(1, "Kestrel"), (2, "Juniper")] {
            let m = store::NewMovie {
                stream_id: id,
                num: Some(id),
                name,
                category_id: None,
                rating: None,
                poster_url: None,
                extension: Some("mkv"),
                added: None,
            };
            store::insert_movie(&tx, "p", &m).unwrap();
        }
        store::finish_vod_import(&tx, "p", Kind::Movie, 0).unwrap();
        tx.commit().unwrap();
        let movies = Index::build(&conn, "p", Kind::Movie).unwrap();
        assert_eq!((movies.kind, movies.len()), (Kind::Movie, 2));
        assert_eq!(Index::build(&conn, "p", Kind::Series).unwrap().len(), 0);
        let mut ix = Search::new();
        ix.set(movies);
        let keys = store::title_keys(
            &conn,
            "p",
            Kind::Movie,
            store::TitleView::All(store::Order::Provider),
        )
        .unwrap();
        let found = ix.filter(Kind::Movie, &keys, "junip").unwrap();
        let row = store::title(&conn, Kind::Movie, found[0]).unwrap().unwrap();
        assert_eq!((found.len(), row.name.as_str()), (1, "Juniper"));
    }

    /// The guide grid's filter (SPEC 8.2): over its category's guide
    /// channels only, so a channel without a guide never shows however
    /// well it matches.
    #[test]
    fn the_grid_filters_guide_channels_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = store::open(&dir.path().join("v.db")).unwrap();
        let tx = conn.transaction().unwrap();
        store::begin_channel_import(&tx, "p", "P", [("a", "UK| Sport")]).unwrap();
        let chans = [
            (1, "Kestrel Sport", Some("k1")),
            // The best match, but no guide.
            (2, "KXN", None),
            (3, "KXN Sport", Some("k3")),
            // A guide id the guide does not have.
            (4, "KXN News", Some("k4")),
            (5, "Juniper", Some("k5")),
        ];
        for (id, name, epg) in chans {
            let c = store::NewChannel {
                stream_id: id,
                num: Some(id),
                name,
                category_id: Some("a"),
                epg_channel_id: epg,
                icon_url: None,
                extension: "ts",
                archive_days: 0,
                url: None,
                tvg_name: None,
                headers: None,
            };
            store::insert_channel(&tx, "p", &c).unwrap();
        }
        store::finish_channel_import(&tx, "p", 0).unwrap();
        store::begin_epg_import(&tx, "p", 0).unwrap();
        for epg in ["k1", "k3", "k5"] {
            let p = store::NewProgramme {
                epg_channel_id: epg,
                start: 0,
                stop: 100,
                title: "Programme",
                description: None,
            };
            store::insert_programme(&tx, "p", &p).unwrap();
        }
        store::resolve_guides(&tx, "p").unwrap();
        tx.commit().unwrap();
        let mut ix = Search::new();
        ix.set(Index::build(&conn, "p", Kind::Live).unwrap());
        let view = store::View::Category("a", store::Order::Provider);
        let guide = store::guide_keys(&conn, "p", view).unwrap();
        let ids = |keys: Vec<i64>| -> Vec<i64> {
            keys.iter()
                .map(|&k| store::channel(&conn, k, 0).unwrap().unwrap().stream_id)
                .collect()
        };
        // Best match first, among guide channels only.
        assert_eq!(ids(ix.live(&guide, "kxn")), [3]);
        assert_eq!(ids(ix.live(&guide, "sport")), [1, 3]);
        assert_eq!(ids(ix.live(&guide, "ksp")), [3, 1]);
        assert!(ix.live(&guide, "news").is_empty());
    }

    /// Ticket 23: the index is rebuilt right after a channel import, as the
    /// EPG import starts writing. The build must not wait for the writer.
    #[test]
    fn builds_while_an_import_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.db");
        let mut writer = store::open(&path).unwrap();
        let tx = writer.transaction().unwrap();
        store::begin_channel_import(&tx, "p", "P", []).unwrap();
        for id in 1..=3 {
            let name = format!("Channel {id}");
            let c = store::NewChannel {
                stream_id: id,
                num: Some(id),
                name: &name,
                category_id: None,
                epg_channel_id: None,
                icon_url: None,
                extension: "ts",
                archive_days: 0,
                url: None,
                tvg_name: None,
                headers: None,
            };
            store::insert_channel(&tx, "p", &c).unwrap();
        }
        store::finish_channel_import(&tx, "p", 0).unwrap();
        tx.commit().unwrap();

        // An EPG import holding the write lock.
        let epg = writer
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        store::begin_epg_import(&epg, "p", 0).unwrap();
        let t = Instant::now();
        let index = store::open_reader(&path)
            .and_then(|c| Index::build(&c, "p", Kind::Live))
            .unwrap();
        assert_eq!(index.len(), 3);
        assert!(t.elapsed().as_secs() < 1, "waited {:?}", t.elapsed());
        epg.commit().unwrap();
    }

    /// SPEC 10: under 16 ms per keystroke at 50k channels. Debug builds of
    /// this crate are unoptimised, so this only guards against gross
    /// regressions; the budget is checked in the app's debug log.
    #[test]
    fn fifty_thousand_names() {
        let names: Vec<String> = (0..50_000)
            .map(|i| format!("UK: Channel {i} ᴴᴰ Sport News"))
            .collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut ix = index(&refs);
        let keys = all(&ix);
        let t = Instant::now();
        let found = ix.live(&keys, "chan 4999");
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        eprintln!("50k names: {} matches in {ms:.1} ms", found.len());
        assert_eq!(found[0], 4999);
        assert!(ms < 200.0, "{ms} ms");
    }
}
