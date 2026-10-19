//! SQLite store (SPEC 5; ADR 0003): schema, migrations, queries.
//!
//! WAL mode: the UI reads on its own connection while the worker's single
//! writer connection imports, and sees the previous snapshot until commit.

use crate::epg_sources::epg_norm;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use unicode_normalization::UnicodeNormalization;

/// Migration `i` takes the database from `user_version` `i` to `i + 1`.
const MIGRATIONS: &[&str] = &[
    // 1: SPEC 5 schema. Drops the ticket 01 skeleton tables, which held only
    // re-fetchable provider data.
    "DROP TABLE IF EXISTS channel;
     DROP TABLE IF EXISTS category;
     CREATE TABLE provider (
         id TEXT PRIMARY KEY,
         name TEXT NOT NULL,
         last_channels_refresh INTEGER,
         last_epg_refresh INTEGER);
     CREATE TABLE category (
         provider_id TEXT NOT NULL,
         category_id TEXT NOT NULL,
         name TEXT NOT NULL,
         sort INTEGER NOT NULL,
         PRIMARY KEY (provider_id, category_id));
     CREATE TABLE channel (
         provider_id TEXT NOT NULL,
         stream_id INTEGER NOT NULL,
         num INTEGER,
         name TEXT NOT NULL,
         name_norm TEXT NOT NULL,
         category_id TEXT,
         epg_channel_id TEXT,
         icon_url TEXT,
         extension TEXT NOT NULL,
         PRIMARY KEY (provider_id, stream_id));
     CREATE TABLE favourite (
         provider_id TEXT NOT NULL,
         stream_id INTEGER NOT NULL,
         name_norm TEXT NOT NULL,
         group_id INTEGER,
         position INTEGER NOT NULL,
         PRIMARY KEY (provider_id, stream_id));
     -- Rebuilt in bulk by finish_channel_import: per-row triggers made a
     -- 60k-channel import 4x slower.
     CREATE VIRTUAL TABLE channel_fts USING fts5(name, content='channel', content_rowid='rowid');",
    // 2: list order per provider and per category, served straight from the index.
    "CREATE INDEX channel_by_num ON channel (provider_id, num);
     CREATE INDEX channel_by_category ON channel (provider_id, category_id, num);",
    // 3: EPG. The key serves "on now" (latest start at or before now) directly.
    "CREATE TABLE programme (
         provider_id TEXT NOT NULL,
         epg_channel_id TEXT NOT NULL,
         start INTEGER NOT NULL,
         stop INTEGER NOT NULL,
         title TEXT NOT NULL,
         description TEXT,
         PRIMARY KEY (provider_id, epg_channel_id, start)) WITHOUT ROWID;",
    // 4: the favourite's display name, so an unavailable one (5) still shows
    // what it was. Kept current by each channel import.
    "ALTER TABLE favourite ADD COLUMN name TEXT NOT NULL DEFAULT '';",
    // 5: small app state that is not provider data, e.g. the last provider
    // switched to (ticket 10).
    "CREATE TABLE app_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    // 6: recently played (ticket 12). `rowid` breaks ties within a second:
    // each play replaces its row, so the latest has the highest.
    "CREATE TABLE history (
         provider_id TEXT NOT NULL,
         stream_id INTEGER NOT NULL,
         played_at INTEGER NOT NULL,
         PRIMARY KEY (provider_id, stream_id));",
    // 7: A–Z order (ticket 19), served from the index like provider order.
    "CREATE INDEX channel_by_name ON channel (provider_id, name_norm, num);
     CREATE INDEX channel_by_category_name ON channel (provider_id, category_id, name_norm, num);",
    // 8: the category allowlist (ticket 20). `category.shown` is derived from
    // the rules and overrides by `apply_category_rules`, on each import and
    // each edit, so the library and All read it directly. All's indexes carry
    // `category_id` so they still cover its query.
    "ALTER TABLE category ADD COLUMN shown INTEGER NOT NULL DEFAULT 1;
     DROP INDEX channel_by_num;
     CREATE INDEX channel_by_num ON channel (provider_id, num, category_id);
     DROP INDEX channel_by_name;
     CREATE INDEX channel_by_name ON channel (provider_id, name_norm, num, category_id);
     CREATE TABLE category_rule (
         provider_id TEXT NOT NULL,
         pattern TEXT NOT NULL,
         PRIMARY KEY (provider_id, pattern));
     CREATE TABLE category_override (
         provider_id TEXT NOT NULL,
         category_id TEXT NOT NULL,
         name TEXT NOT NULL,
         shown INTEGER NOT NULL,
         PRIMARY KEY (provider_id, category_id));",
    // 9: channel name rules (ticket 25). `channel.shown` is derived from them
    // by `apply_channel_rules`, on each import and each edit. The index holds
    // only hidden channels, so the lists' filter on it costs nothing without
    // rules and the indexes that cover them stay as they are.
    "ALTER TABLE channel ADD COLUMN shown INTEGER NOT NULL DEFAULT 1;
     CREATE INDEX channel_hidden ON channel (provider_id, category_id) WHERE NOT shown;
     CREATE TABLE channel_rule (
         provider_id TEXT NOT NULL,
         pattern TEXT NOT NULL,
         PRIMARY KEY (provider_id, pattern));",
    // 10: kept region tags (ticket 26), stored as their letters only. They
    // feed `channel.shown` along with the channel rules.
    "CREATE TABLE kept_tag (
         provider_id TEXT NOT NULL,
         tag TEXT NOT NULL,
         PRIMARY KEY (provider_id, tag));",
    // 11: recordings (recording ticket 02). `channel` is the channel's name
    // when scheduled, for the file name and for a channel since gone.
    // `guide_id` and `title` are NULL for a recording of what plays with no
    // guide, whose `stop` is its `start` until it ends (recording ticket 04).
    // `note` is why one failed, or why a done one was cut short.
    "CREATE TABLE recording (
         id INTEGER PRIMARY KEY,
         provider_id TEXT NOT NULL,
         stream_id INTEGER NOT NULL,
         channel TEXT NOT NULL,
         guide_id TEXT,
         title TEXT,
         start INTEGER NOT NULL,
         stop INTEGER NOT NULL,
         padded_start INTEGER NOT NULL,
         padded_stop INTEGER NOT NULL,
         state TEXT NOT NULL,
         file TEXT,
         note TEXT);
     CREATE INDEX recording_by_state ON recording (state, padded_start);",
    // 12: a finished recording whose file `d` moved to the trash (recording
    // ticket 05). Kept, hidden while its file is gone, so a restore from the
    // trash brings it back to the Recordings view.
    "ALTER TABLE recording ADD COLUMN trashed INTEGER NOT NULL DEFAULT 0;",
    // 13: how many days back the channel's archive reaches, 0 for none
    // (catch-up ticket 02). The EPG import keeps past programmes that long.
    "ALTER TABLE channel ADD COLUMN archive_days INTEGER NOT NULL DEFAULT 0;",
    // 14: the channels with an archive (catch-up ticket 03), about 1k of
    // 58k, for how far back the guide grid reaches: 0.5 ms, against 19 ms
    // reading every channel. Partial, so no other query's plan can pick it,
    // and covering, so no row is read.
    "CREATE INDEX channel_archive ON channel (provider_id, archive_days) WHERE archive_days > 0;",
    // 15: whether the provider's archive has the programme, from its
    // channel's `get_simple_data_table` (catch-up ticket 06); NULL where
    // no table said, so the channel's `archive_days` decides.
    "ALTER TABLE programme ADD COLUMN archive INTEGER;",
    // 16: tables stored by 15's first version took their times as UTC; they
    // are the server's clock (SPEC 6.1). Drops the older days they added,
    // before each guide id's first programme from `xmltv.php`, and every
    // flag, as those may sit on the wrong programme. Fetched again.
    "DELETE FROM programme WHERE archive IS NOT NULL
         AND start < (SELECT min(start) FROM programme q
                      WHERE q.provider_id = programme.provider_id
                        AND q.epg_channel_id = programme.epg_channel_id
                        AND q.archive IS NULL);
     UPDATE programme SET archive = NULL WHERE archive IS NOT NULL;",
    // 17: movies and series (VOD ticket 02). Categories, their overrides
    // and rules gain a kind, in the key: the provider numbers each kind's
    // categories on its own. Each kind has its own rules (SPEC 5). The
    // lists are imported whole like channels; `*_by_category` serve a
    // category's titles in provider order and manage mode's counts.
    "CREATE TABLE category_new (
         provider_id TEXT NOT NULL,
         kind TEXT NOT NULL,
         category_id TEXT NOT NULL,
         name TEXT NOT NULL,
         sort INTEGER NOT NULL,
         shown INTEGER NOT NULL DEFAULT 1,
         PRIMARY KEY (provider_id, kind, category_id));
     INSERT INTO category_new SELECT provider_id, 'live', category_id, name, sort, shown FROM category;
     DROP TABLE category;
     ALTER TABLE category_new RENAME TO category;
     CREATE TABLE category_override_new (
         provider_id TEXT NOT NULL,
         kind TEXT NOT NULL,
         category_id TEXT NOT NULL,
         name TEXT NOT NULL,
         shown INTEGER NOT NULL,
         PRIMARY KEY (provider_id, kind, category_id));
     INSERT INTO category_override_new SELECT provider_id, 'live', category_id, name, shown FROM category_override;
     DROP TABLE category_override;
     ALTER TABLE category_override_new RENAME TO category_override;
     CREATE TABLE category_rule_new (
         provider_id TEXT NOT NULL,
         kind TEXT NOT NULL,
         pattern TEXT NOT NULL,
         PRIMARY KEY (provider_id, kind, pattern));
     INSERT INTO category_rule_new SELECT provider_id, 'live', pattern FROM category_rule ORDER BY rowid;
     DROP TABLE category_rule;
     ALTER TABLE category_rule_new RENAME TO category_rule;
     CREATE TABLE movie (
         provider_id TEXT NOT NULL,
         stream_id INTEGER NOT NULL,
         num INTEGER,
         name TEXT NOT NULL,
         name_norm TEXT NOT NULL,
         category_id TEXT,
         rating REAL,
         poster_url TEXT,
         extension TEXT,
         added INTEGER,
         PRIMARY KEY (provider_id, stream_id));
     CREATE INDEX movie_by_category ON movie (provider_id, category_id, num);
     CREATE TABLE series (
         provider_id TEXT NOT NULL,
         series_id INTEGER NOT NULL,
         num INTEGER,
         name TEXT NOT NULL,
         name_norm TEXT NOT NULL,
         category_id TEXT,
         year INTEGER,
         rating REAL,
         cover_url TEXT,
         genre TEXT,
         plot TEXT,
         actors TEXT,
         director TEXT,
         episode_run_time INTEGER,
         last_modified INTEGER,
         PRIMARY KEY (provider_id, series_id));
     CREATE INDEX series_by_category ON series (provider_id, category_id, num);
     ALTER TABLE provider ADD COLUMN last_movies_refresh INTEGER;
     ALTER TABLE provider ADD COLUMN last_series_refresh INTEGER;",
    // 18: the movie and series lists (VOD ticket 03), each order read
    // straight from an index as the channels' are (migrations 7 and 8):
    // All in provider and A–Z order, a category A–Z, and Recently added,
    // newest first by `added` or `last_modified`.
    "CREATE INDEX movie_by_num ON movie (provider_id, num, category_id);
     CREATE INDEX movie_by_name ON movie (provider_id, name_norm, num, category_id);
     CREATE INDEX movie_by_category_name ON movie (provider_id, category_id, name_norm, num);
     CREATE INDEX movie_by_added ON movie (provider_id, added, category_id);
     CREATE INDEX series_by_num ON series (provider_id, num, category_id);
     CREATE INDEX series_by_name ON series (provider_id, name_norm, num, category_id);
     CREATE INDEX series_by_category_name ON series (provider_id, category_id, name_norm, num);
     CREATE INDEX series_by_added ON series (provider_id, last_modified, category_id);",
    // 19: a movie's info (VOD ticket 04), fetched when it is selected. On
    // the movie's row, so the next import of the list clears it with the
    // row; `info_at` says it was fetched, as any of the rest may be empty.
    "ALTER TABLE movie ADD COLUMN year INTEGER;
     ALTER TABLE movie ADD COLUMN length INTEGER;
     ALTER TABLE movie ADD COLUMN genre TEXT;
     ALTER TABLE movie ADD COLUMN plot TEXT;
     ALTER TABLE movie ADD COLUMN actors TEXT;
     ALTER TABLE movie ADD COLUMN director TEXT;
     ALTER TABLE movie ADD COLUMN info_at INTEGER;",
    // 20: a series' info (VOD ticket 05), fetched when it is selected: its
    // episodes, and on its row how many seasons the provider has and the
    // latest episode's year. A series import clears both, as 19's.
    "ALTER TABLE series ADD COLUMN seasons INTEGER;
     ALTER TABLE series ADD COLUMN last_year INTEGER;
     ALTER TABLE series ADD COLUMN info_at INTEGER;
     CREATE TABLE episode (
         provider_id TEXT NOT NULL,
         series_id INTEGER NOT NULL,
         season INTEGER NOT NULL,
         episode_num INTEGER,
         stream_id INTEGER NOT NULL,
         title TEXT NOT NULL,
         extension TEXT,
         length INTEGER,
         sort INTEGER NOT NULL,
         PRIMARY KEY (provider_id, series_id, stream_id));",
    // 21: where a movie or episode was stopped (VOD ticket 07). By stream
    // id, as history is, so it outlives the imports that clear episodes;
    // an episode keeps its series, season and number to find the next.
    "CREATE TABLE progress (
         provider_id TEXT NOT NULL,
         kind TEXT NOT NULL,
         stream_id INTEGER NOT NULL,
         series_id INTEGER,
         season INTEGER,
         episode_num INTEGER,
         pos INTEGER NOT NULL,
         length INTEGER,
         watched INTEGER NOT NULL,
         at INTEGER NOT NULL,
         PRIMARY KEY (provider_id, kind, stream_id));
     CREATE INDEX progress_by_series ON progress (provider_id, series_id, at);",
    // 22: favourites and recently played across kinds (VOD ticket 11).
    // Series ids collide with live and movie ids (VOD ticket 01), so the
    // kind is part of the key; a series' `stream_id` is its `series_id`.
    // Existing rows are live. Rowids are kept: history breaks ties by them.
    "CREATE TABLE favourite_new (
         provider_id TEXT NOT NULL,
         kind TEXT NOT NULL,
         stream_id INTEGER NOT NULL,
         name_norm TEXT NOT NULL,
         group_id INTEGER,
         position INTEGER NOT NULL,
         name TEXT NOT NULL DEFAULT '',
         PRIMARY KEY (provider_id, kind, stream_id));
     INSERT INTO favourite_new (rowid, provider_id, kind, stream_id, name_norm, group_id, position, name)
         SELECT rowid, provider_id, 'live', stream_id, name_norm, group_id, position, name FROM favourite;
     DROP TABLE favourite;
     ALTER TABLE favourite_new RENAME TO favourite;
     CREATE TABLE history_new (
         provider_id TEXT NOT NULL,
         kind TEXT NOT NULL,
         stream_id INTEGER NOT NULL,
         played_at INTEGER NOT NULL,
         PRIMARY KEY (provider_id, kind, stream_id));
     INSERT INTO history_new (rowid, provider_id, kind, stream_id, played_at)
         SELECT rowid, provider_id, 'live', stream_id, played_at FROM history;
     DROP TABLE history;
     ALTER TABLE history_new RENAME TO history;",
    // 23: downloads (VOD ticket 10). `kind` is `movie` or `episode`; an
    // episode keeps its series, season and number for its name. `file` is
    // the finished file's path, fixed at the first start; until done it is
    // written as `<file>.part`. `total` is the size in bytes, once the
    // provider has said. Queued in `added` order.
    "CREATE TABLE download (
         id INTEGER PRIMARY KEY,
         provider_id TEXT NOT NULL,
         kind TEXT NOT NULL,
         stream_id INTEGER NOT NULL,
         series_id INTEGER,
         season INTEGER,
         episode_num INTEGER,
         title TEXT NOT NULL,
         episode TEXT,
         extension TEXT NOT NULL,
         state TEXT NOT NULL,
         file TEXT,
         total INTEGER,
         note TEXT,
         added INTEGER NOT NULL,
         ended INTEGER);
     CREATE INDEX download_by_title ON download (provider_id, kind, stream_id);",
    // 24: a done download whose file `d` moved to the trash, kept as a
    // recording is (12), so a restore from the trash brings it back.
    "ALTER TABLE download ADD COLUMN trashed INTEGER NOT NULL DEFAULT 0;",
    // 25: guide sources (EPG sources ticket 02). `channel.guide_id` is the
    // guide id the channel's programmes are under, from the first enabled
    // source with programmes for it: its own `epg_channel_id` for the
    // provider's guide, a [`source_guide_id`] for an external one.
    // `epg_source` is each provider's order, `ident` empty for its
    // `xmltv.php`; `source_channel` a source's whole channel list;
    // `source_match` which feed channel each provider channel matched,
    // `guide_id` NULL when ambiguous.
    "ALTER TABLE channel ADD COLUMN guide_id TEXT;
     UPDATE channel SET guide_id = epg_channel_id
         WHERE EXISTS (SELECT 1 FROM programme p WHERE p.provider_id = channel.provider_id
                         AND p.epg_channel_id = channel.epg_channel_id);
     CREATE TABLE epg_source (
         id INTEGER PRIMARY KEY,
         provider_id TEXT NOT NULL,
         ident TEXT NOT NULL,
         position INTEGER NOT NULL,
         enabled INTEGER NOT NULL,
         shift_minutes INTEGER NOT NULL,
         imported INTEGER,
         error TEXT,
         UNIQUE (provider_id, ident));
     CREATE TABLE source_channel (
         source_id INTEGER NOT NULL,
         channel_id TEXT NOT NULL,
         names TEXT NOT NULL,
         icon_url TEXT);
     CREATE INDEX source_channel_by_source ON source_channel (source_id, channel_id);
     CREATE TABLE source_match (
         source_id INTEGER NOT NULL,
         stream_id INTEGER NOT NULL,
         guide_id TEXT,
         PRIMARY KEY (source_id, stream_id)) WITHOUT ROWID;",
    // 26: fetch limits (EPG sources ticket 03). A source's validators for
    // a conditional request, `mtime` (ns) for a path, when it was last
    // asked (`fetched`) and until when it backs off; `source_request` each
    // request in the last 24 h.
    "ALTER TABLE epg_source ADD COLUMN etag TEXT;
     ALTER TABLE epg_source ADD COLUMN last_modified TEXT;
     ALTER TABLE epg_source ADD COLUMN mtime INTEGER;
     ALTER TABLE epg_source ADD COLUMN fetched INTEGER;
     ALTER TABLE epg_source ADD COLUMN retry_at INTEGER;
     CREATE TABLE source_request (source_id INTEGER NOT NULL, at INTEGER NOT NULL);
     CREATE INDEX source_request_by_source ON source_request (source_id, at);",
    // 27: pins and manual mappings (EPG sources ticket 05). A pin puts one
    // source first for a channel, its shift added to the source's; a
    // mapping names a source's channel for it, by the source's ident so it
    // outlives the source's removal. Both keep the channel's `name_norm`
    // for the favourites fallback. `reimport` marks a source to fetch
    // whole at the next refresh.
    "CREATE TABLE source_pin (
         provider_id TEXT NOT NULL,
         stream_id INTEGER NOT NULL,
         source_id INTEGER NOT NULL,
         shift_minutes INTEGER NOT NULL,
         name_norm TEXT NOT NULL,
         PRIMARY KEY (provider_id, stream_id)) WITHOUT ROWID;
     CREATE TABLE source_mapping (
         provider_id TEXT NOT NULL,
         ident TEXT NOT NULL,
         stream_id INTEGER NOT NULL,
         channel_id TEXT NOT NULL,
         name_norm TEXT NOT NULL,
         PRIMARY KEY (provider_id, ident, stream_id)) WITHOUT ROWID;
     ALTER TABLE epg_source ADD COLUMN reimport INTEGER NOT NULL DEFAULT 0;",
    // 28: a channel's looser name form (SPEC 6.1), for its alternates
    // (stream recovery ticket 03). [`migrate`] fills it for the rows there.
    "ALTER TABLE channel ADD COLUMN name_loose TEXT NOT NULL DEFAULT '';
     CREATE INDEX channel_by_loose ON channel (provider_id, name_loose, num);",
];

/// Whether a category or title is live, a movie or a series (VOD ticket 02).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Kind {
    #[default]
    Live,
    Movie,
    Series,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Live, Kind::Movie, Kind::Series];

    /// As stored in `category.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Live => "live",
            Kind::Movie => "movie",
            Kind::Series => "series",
        }
    }

    /// The table holding the kind's titles.
    fn table(self) -> &'static str {
        match self {
            Kind::Live => "channel",
            Kind::Movie => "movie",
            Kind::Series => "series",
        }
    }

    /// The table's id column: what `favourite` and `history` store as
    /// `stream_id`.
    fn id_column(self) -> &'static str {
        match self {
            Kind::Live | Kind::Movie => "stream_id",
            Kind::Series => "series_id",
        }
    }

    /// The column Recently added orders by; channels have none.
    fn added(self) -> &'static str {
        match self {
            Kind::Live => "NULL",
            Kind::Movie => "added",
            Kind::Series => "last_modified",
        }
    }
}

impl rusqlite::ToSql for Kind {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl rusqlite::types::FromSql for Kind {
    fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let s = v.as_str()?;
        Kind::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or(rusqlite::types::FromSqlError::InvalidType)
    }
}

/// How many entries `history` keeps per provider (SPEC 5), of all kinds.
const HISTORY_KEPT: i64 = 50;

/// `app_state` key: the provider last switched to.
const LAST_PROVIDER: &str = "last_provider";

/// `app_state` key: the list [`Order`], `az` or absent.
const ORDER: &str = "order";

/// `app_state` key prefix, then the provider id: when its Continue
/// watching was last cleared (VOD ticket 07).
const CONTINUE_CLEARED: &str = "continue_cleared:";

/// One channel as the UI shows it. Fetched a row at a time by `rowid`.
pub struct ChannelRow {
    pub stream_id: i64,
    pub num: Option<i64>,
    pub name: String,
    pub extension: String,
    pub category: Option<String>,
    pub epg_channel_id: Option<String>,
    /// Where its programmes are: the guide id of the first enabled source
    /// with programmes for it (EPG sources ticket 02).
    pub guide_id: Option<String>,
    pub icon_url: Option<String>,
    /// How many days back the channel's archive reaches, 0 for none.
    pub archive_days: i64,
    /// The programme airing at the time the row was read.
    pub now: Option<Programme>,
    pub favourite: bool,
    /// False for a favourite whose channel is gone from the provider's list
    /// (SPEC 5): shown dimmed, not playable.
    pub available: bool,
    /// Live, or an unavailable favourite's own kind (VOD ticket 11).
    pub kind: Kind,
}

/// A programme as the UI shows it. Times are Unix seconds, UTC.
#[derive(Clone, Debug, PartialEq)]
pub struct Programme {
    pub start: i64,
    pub stop: i64,
    pub title: String,
    /// Whether the provider's archive has it, if its channel's table said
    /// (catch-up ticket 06).
    pub archive: Option<bool>,
}

/// The order of the provider's categories and of the channels in All and in
/// a category. Favourites and Recently played have their own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Order {
    Provider,
    /// By `name_norm`, ties in provider order.
    Az,
}

/// Which channels a list shows.
pub enum View<'a> {
    All(Order),
    /// In the user's order.
    Favourites,
    /// Recently played, latest first.
    Recent,
    Category(&'a str, Order),
}

pub struct NewChannel<'a> {
    pub stream_id: i64,
    pub num: Option<i64>,
    pub name: &'a str,
    pub category_id: Option<&'a str>,
    pub epg_channel_id: Option<&'a str>,
    pub icon_url: Option<&'a str>,
    pub extension: &'a str,
    pub archive_days: i64,
}

/// Opens (creating if needed) and migrates the database. Call once for the
/// worker's writer and once for the UI's reader; building the search index
/// opens a short-lived reader of its own with [`open_reader`].
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    migrate(&mut conn)?;
    Ok(conn)
}

/// Opens a read-only connection to a database [`open`] has already
/// migrated, without migrating. [`open`] takes the write lock to migrate, so
/// opened while an import writes it waits out the busy timeout and fails
/// (ticket 23); this never waits for the writer. WAL: it reads the last
/// committed snapshot.
pub fn open_reader(path: &Path) -> rusqlite::Result<Connection> {
    use rusqlite::OpenFlags;
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn)
}

fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in (0..).zip(MIGRATIONS).skip(version as usize) {
        log::info!("database migration {}", i + 1);
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i + 1)?;
    }
    if version < 28 {
        fill_name_loose(&tx)?;
    }
    tx.commit()
}

/// A channel's looser name form (SPEC 6.1), for its alternates; none for
/// a divider such as `##### SPORT #####`, which providers list as a
/// channel to head the ones after it.
fn loose_name(name: &str) -> String {
    let name = name.trim();
    if name.starts_with('#') && name.ends_with('#') {
        return String::new();
    }
    epg_norm(name)
}

/// Migration 28's column for the channels stored before it.
fn fill_name_loose(tx: &Connection) -> rusqlite::Result<()> {
    let rows: Vec<(i64, String)> = tx
        .prepare("SELECT rowid, name FROM channel")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut update = tx.prepare("UPDATE channel SET name_loose = ?2 WHERE rowid = ?1")?;
    for (rowid, name) in rows {
        update.execute(params![rowid, loose_name(&name)])?;
    }
    Ok(())
}

/// The shown categories' `(category_id, name)` of one kind in `order`.
/// A–Z compares [`name_norm`]s, as the channel queries do.
pub fn categories(
    conn: &Connection,
    provider_id: &str,
    kind: Kind,
    order: Order,
) -> rusqlite::Result<Vec<(String, String)>> {
    let mut cats: Vec<(String, String)> = conn
        .prepare_cached(
            "SELECT category_id, name FROM category WHERE provider_id = ?1 AND kind = ?2 AND shown ORDER BY sort",
        )?
        .query_map(params![provider_id, kind], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if order == Order::Az {
        // Stable: ties keep provider order.
        cats.sort_by_cached_key(|(_, name)| name_norm(name));
    }
    Ok(cats)
}

/// A category as manage mode lists it (ticket 20).
#[derive(Clone, Debug, PartialEq)]
pub struct ManagedCategory {
    pub id: String,
    pub name: String,
    pub shown: bool,
    /// Shown or hidden by its own override rather than by the rules.
    pub overridden: bool,
    /// How many channels, movies or series it holds.
    pub titles: i64,
    /// Of `titles`, how many the channel rules hide.
    pub hidden: i64,
}

/// Every category of the provider of one kind, shown or not, in `order`.
pub fn managed_categories(
    conn: &Connection,
    provider_id: &str,
    kind: Kind,
    order: Order,
) -> rusqlite::Result<Vec<ManagedCategory>> {
    // Only channels have rules of their own.
    let hidden = match kind {
        Kind::Live => {
            "(SELECT count(*) FROM channel c
              WHERE c.provider_id = k.provider_id AND c.category_id = k.category_id
                AND NOT c.shown)"
        }
        Kind::Movie | Kind::Series => "0",
    };
    let mut cats: Vec<ManagedCategory> = conn
        .prepare_cached(&format!(
            "SELECT k.category_id, k.name, k.shown, o.shown IS NOT NULL,
                    (SELECT count(*) FROM {table} c
                     WHERE c.provider_id = k.provider_id AND c.category_id = k.category_id),
                    {hidden}
             FROM category k
             LEFT JOIN category_override o
                 ON o.provider_id = k.provider_id AND o.kind = k.kind
                AND o.category_id = k.category_id
             WHERE k.provider_id = ?1 AND k.kind = ?2 ORDER BY k.sort",
            table = kind.table(),
        ))?
        .query_map(params![provider_id, kind], |r| {
            Ok(ManagedCategory {
                id: r.get(0)?,
                name: r.get(1)?,
                shown: r.get(2)?,
                overridden: r.get(3)?,
                titles: r.get(4)?,
                hidden: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    if order == Order::Az {
        cats.sort_by_cached_key(|c| name_norm(&c.name));
    }
    Ok(cats)
}

/// The provider's category rules for one kind, oldest first.
pub fn category_rules(
    conn: &Connection,
    provider_id: &str,
    kind: Kind,
) -> rusqlite::Result<Vec<String>> {
    conn.prepare_cached(
        "SELECT pattern FROM category_rule WHERE provider_id = ?1 AND kind = ?2 ORDER BY rowid",
    )?
    .query_map(params![provider_id, kind], |r| r.get(0))?
    .collect()
}

/// Whether any of the provider's categories, or any channel by name, is
/// hidden.
pub fn any_hidden(conn: &Connection, provider_id: &str) -> rusqlite::Result<bool> {
    conn.prepare_cached(
        "SELECT EXISTS (SELECT 1 FROM category WHERE provider_id = ?1 AND NOT shown)
             OR EXISTS (SELECT 1 FROM channel INDEXED BY channel_hidden
                        WHERE provider_id = ?1 AND NOT shown)",
    )?
    .query_row([provider_id], |r| r.get(0))
}

/// How many channels All shows.
pub fn shown_channel_count(conn: &Connection, provider_id: &str) -> rusqlite::Result<i64> {
    conn.prepare_cached(&format!(
        "SELECT count(*) FROM channel c WHERE provider_id = ?1 AND {NOT_HIDDEN} AND {NOT_RULED}"
    ))?
    .query_row([provider_id], |r| r.get(0))
}

/// Which live channels [`alternate_spread`] counts the alternates of.
#[cfg(feature = "devtools")]
#[derive(Clone, Copy)]
pub enum AlternatesOf {
    /// All's.
    Shown,
    Favourites,
    Recent,
}

/// How many of `of`'s channels have no alternate, one, two, and three or
/// more: other channels All shows with the same looser name, as
/// [`alternates`] finds them (stream recovery tickets 01, 03). A favourite
/// whose channel is gone has none.
#[cfg(feature = "devtools")]
pub fn alternate_spread(
    conn: &Connection,
    provider_id: &str,
    of: AlternatesOf,
) -> rusqlite::Result<[i64; 4]> {
    let base = match of {
        AlternatesOf::Shown => format!(
            "SELECT c.stream_id, c.name_loose FROM channel c
             WHERE provider_id = ?1 AND {NOT_HIDDEN} AND {NOT_RULED}"
        ),
        AlternatesOf::Favourites => {
            "SELECT f.stream_id, coalesce(c.name_loose, '') AS name_loose
             FROM favourite f LEFT JOIN channel c
                 ON c.provider_id = f.provider_id AND c.stream_id = f.stream_id
             WHERE f.provider_id = ?1 AND f.kind = 'live'"
                .into()
        }
        AlternatesOf::Recent => "SELECT h.stream_id, c.name_loose
             FROM history h JOIN channel c
                 ON c.provider_id = h.provider_id AND c.stream_id = h.stream_id
             WHERE h.provider_id = ?1 AND h.kind = 'live'"
            .into(),
    };
    let sql = format!(
        "SELECT min(n, 3), count(*) FROM (
             SELECT (SELECT count(*) FROM channel c
                     WHERE provider_id = ?1 AND c.name_loose = b.name_loose AND b.name_loose != ''
                         AND c.stream_id != b.stream_id AND {NOT_HIDDEN} AND {NOT_RULED}) AS n
             FROM ({base}) AS b)
         GROUP BY 1"
    );
    let mut spread = [0; 4];
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([provider_id])?;
    while let Some(r) = rows.next()? {
        spread[r.get::<_, i64>(0)? as usize] = r.get(1)?;
    }
    Ok(spread)
}

/// How many movies or series are in shown categories, or in none the
/// provider lists.
pub fn shown_title_count(
    conn: &Connection,
    provider_id: &str,
    kind: Kind,
) -> rusqlite::Result<i64> {
    conn.prepare_cached(&format!(
        "SELECT count(*) FROM {} WHERE provider_id = ?1
           AND (category_id IS NULL OR category_id NOT IN
                (SELECT category_id FROM category WHERE provider_id = ?1 AND kind = ?2 AND NOT shown))",
        kind.table()
    ))?
    .query_row(params![provider_id, kind], |r| r.get(0))
}

/// Every channel's `(category_id, name)`, for counting what a channel rule
/// hides in manage mode. `+provider_id` makes it a table scan: through an
/// index each row is a lookup, and at 50k channels that took half again as
/// long.
pub fn channel_names_by_category(
    conn: &Connection,
    provider_id: &str,
) -> rusqlite::Result<Vec<(Option<String>, String)>> {
    conn.prepare_cached("SELECT category_id, name FROM channel WHERE +provider_id = ?1")?
        .query_map([provider_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect()
}

/// Adds `pattern` as a rule for `kind`'s categories, or removes the rule it
/// already is (compared case-insensitively). Returns whether it was added.
/// Blank patterns are ignored.
pub fn toggle_category_rule(
    conn: &mut Connection,
    provider_id: &str,
    kind: Kind,
    pattern: &str,
) -> rusqlite::Result<bool> {
    toggle_rule(
        conn,
        "category_rule",
        provider_id,
        Some(kind),
        pattern,
        apply_category_rules,
    )
}

/// The provider's channel name rules (ticket 25), oldest first.
pub fn channel_rules(conn: &Connection, provider_id: &str) -> rusqlite::Result<Vec<String>> {
    conn.prepare_cached("SELECT pattern FROM channel_rule WHERE provider_id = ?1 ORDER BY rowid")?
        .query_map([provider_id], |r| r.get(0))?
        .collect()
}

/// As [`toggle_category_rule`], for a channel name rule.
pub fn toggle_channel_rule(
    conn: &mut Connection,
    provider_id: &str,
    pattern: &str,
) -> rusqlite::Result<bool> {
    toggle_rule(
        conn,
        "channel_rule",
        provider_id,
        None,
        pattern,
        apply_channel_rules,
    )
}

/// The provider's kept region tags (ticket 26), oldest first.
pub fn kept_tags(conn: &Connection, provider_id: &str) -> rusqlite::Result<Vec<String>> {
    conn.prepare_cached("SELECT tag FROM kept_tag WHERE provider_id = ?1 ORDER BY rowid")?
        .query_map([provider_id], |r| r.get(0))?
        .collect()
}

/// Keeps the region tag `input` names (`uk`, `UK:` and `uk|` alike), or
/// stops keeping it. Returns whether it was added. Input that is not a tag
/// ([`kept_tag`]) is ignored.
pub fn toggle_kept_tag(
    conn: &mut Connection,
    provider_id: &str,
    input: &str,
) -> rusqlite::Result<bool> {
    let Some(tag) = kept_tag(input) else {
        return Ok(false);
    };
    let tx = conn.transaction()?;
    let removed = tx.execute(
        "DELETE FROM kept_tag WHERE provider_id = ?1 AND tag = ?2",
        params![provider_id, tag],
    )?;
    if removed == 0 {
        tx.execute(
            "INSERT INTO kept_tag VALUES (?1, ?2)",
            params![provider_id, tag],
        )?;
    }
    apply_channel_rules(&tx, provider_id)?;
    tx.commit()?;
    Ok(removed == 0)
}

/// Adds `pattern` to the rule `table`, or removes the rule it already is,
/// then re-derives what the rules decide with `apply`. `kind` scopes a
/// table with a `kind` column.
fn toggle_rule(
    conn: &mut Connection,
    table: &str,
    provider_id: &str,
    kind: Option<Kind>,
    pattern: &str,
    apply: fn(&Connection, &str) -> rusqlite::Result<()>,
) -> rusqlite::Result<bool> {
    if pattern.trim().is_empty() {
        return Ok(false);
    }
    let (scope, kind) = match kind {
        Some(k) => ("kind = ?2", k.as_str()),
        // A placeholder that always matches, so the parameters line up.
        None => ("?2 = ''", ""),
    };
    let tx = conn.transaction()?;
    let existing = tx
        .prepare(&format!(
            "SELECT pattern FROM {table} WHERE provider_id = ?1 AND {scope}"
        ))?
        .query_map(params![provider_id, kind], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .find(|r| fold(r) == fold(pattern));
    let added = match existing {
        Some(rule) => {
            tx.execute(
                &format!("DELETE FROM {table} WHERE provider_id = ?1 AND {scope} AND pattern = ?3"),
                params![provider_id, kind, rule],
            )?;
            false
        }
        None if kind.is_empty() => {
            tx.execute(
                &format!("INSERT INTO {table} (provider_id, pattern) VALUES (?1, ?2)"),
                params![provider_id, pattern],
            )?;
            true
        }
        None => {
            tx.execute(
                &format!("INSERT INTO {table} (provider_id, kind, pattern) VALUES (?1, ?2, ?3)"),
                params![provider_id, kind, pattern],
            )?;
            true
        }
    };
    apply(&tx, provider_id)?;
    tx.commit()?;
    Ok(added)
}

/// Shows a hidden category or hides a shown one with an override. An
/// override that would agree with the rules is dropped instead, so toggling
/// twice leaves the category to the rules again. Returns whether it is now
/// shown.
pub fn toggle_category(
    conn: &mut Connection,
    provider_id: &str,
    kind: Kind,
    category_id: &str,
) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let Some((name, shown)): Option<(String, bool)> = tx
        .query_row(
            "SELECT name, shown FROM category
             WHERE provider_id = ?1 AND kind = ?2 AND category_id = ?3",
            params![provider_id, kind, category_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
    else {
        return Ok(false);
    };
    let rules: Vec<String> = category_rules(&tx, provider_id, kind)?
        .iter()
        .map(|r| fold(r))
        .collect();
    if matches_rules(&rules, &name) == !shown {
        tx.execute(
            "DELETE FROM category_override WHERE provider_id = ?1 AND kind = ?2 AND category_id = ?3",
            params![provider_id, kind, category_id],
        )?;
    } else {
        tx.execute(
            "INSERT OR REPLACE INTO category_override (provider_id, kind, category_id, name, shown)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![provider_id, kind, category_id, name, !shown],
        )?;
    }
    apply_category_rules(&tx, provider_id)?;
    tx.commit()?;
    Ok(!shown)
}

/// How category rules compare names: case-insensitive, and
/// compatibility-folded so a rule typed `hd` also matches a name tagged `ᴴᴰ`.
pub fn fold(s: &str) -> String {
    if s.is_ascii() {
        // NFKC leaves ASCII as it is.
        return s.to_ascii_lowercase();
    }
    s.nfkc().collect::<String>().to_lowercase()
}

/// Whether a category named `name` shows by the rules alone. `rules` are
/// [`fold`]ed; none means everything shows.
pub(crate) fn matches_rules(rules: &[String], name: &str) -> bool {
    if rules.is_empty() {
        return true;
    }
    let name = fold(name);
    rules.iter().any(|r| name.contains(r.as_str()))
}

/// Whether a channel rule hides a channel: `name` contains `rule` at the
/// start of a word, so `fr:` hides `FR: x`, `x | FR: y` and `BE/FR: z` but
/// not `AFR: x`. Both are [`fold`]ed.
pub fn hides(name: &str, rule: &str) -> bool {
    let mut from = 0;
    while let Some(i) = name[from..].find(rule).map(|i| from + i) {
        if !name[..i]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
        {
            return true;
        }
        // On by one character: occurrences may overlap.
        from = i + name[i..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// The region tag at the start of a [`fold`]ed name (ticket 26): 2 or 3
/// letters followed by `:` or `|`, spaces allowed before it, several codes
/// joined by `/`. Returns the codes, `be/fr` for `BE/FR: x`. Digits are
/// never a tag, so `4K:` and `24/7:` have none.
pub fn region_tag(name: &str) -> Option<&str> {
    let s = name.trim_start();
    let mut end = 0;
    loop {
        let letters = s[end..].chars().take_while(|c| c.is_alphabetic());
        let (n, len) = letters.fold((0, 0), |(n, len), c| (n + 1, len + c.len_utf8()));
        if !(2..=3).contains(&n) {
            return None;
        }
        end += len;
        if !s[end..].starts_with('/') {
            break;
        }
        end += 1;
    }
    s[end..]
        .trim_start()
        .starts_with([':', '|'])
        .then(|| &s[..end])
}

/// The tag `input` keeps, as stored: its letters, [`fold`]ed, with a
/// trailing `:` or `|` dropped. `None` unless that is 2 or 3 letters.
pub fn kept_tag(input: &str) -> Option<String> {
    let tag = fold(input);
    let tag = tag.trim().trim_end_matches([':', '|']).trim_end();
    let n = tag.chars().count();
    ((2..=3).contains(&n) && tag.chars().all(char::is_alphabetic)).then(|| tag.to_owned())
}

/// Whether kept tags `kept` let a channel with region tag `tag` show: with
/// none kept, or no tag, it does; otherwise one of its regions must be kept.
pub fn tag_shown(tag: Option<&str>, kept: &[String]) -> bool {
    match tag {
        Some(tag) if !kept.is_empty() => tag.split('/').any(|r| kept.iter().any(|k| k == r)),
        _ => true,
    }
}

/// Recomputes `channel.shown` for the provider: hidden if any channel rule
/// hides its name, or kept tags (ticket 26) hide its region tag. Only rows
/// that change are written.
fn apply_channel_rules(tx: &Connection, provider_id: &str) -> rusqlite::Result<()> {
    let rules: Vec<String> = channel_rules(tx, provider_id)?
        .iter()
        .map(|r| fold(r))
        .collect();
    let kept = kept_tags(tx, provider_id)?;
    if rules.is_empty() && kept.is_empty() {
        tx.execute(
            "UPDATE channel SET shown = 1 WHERE provider_id = ?1 AND NOT shown",
            [provider_id],
        )?;
        return Ok(());
    }
    let mut changed = Vec::new();
    let mut stmt =
        tx.prepare_cached("SELECT rowid, name, shown FROM channel WHERE provider_id = ?1")?;
    let mut rows = stmt.query([provider_id])?;
    while let Some(r) = rows.next()? {
        let name = fold(r.get_ref(1)?.as_str()?);
        let shown =
            !rules.iter().any(|rule| hides(&name, rule)) && tag_shown(region_tag(&name), &kept);
        if shown != r.get::<_, bool>(2)? {
            changed.push((r.get::<_, i64>(0)?, shown));
        }
    }
    let mut set = tx.prepare_cached("UPDATE channel SET shown = ?2 WHERE rowid = ?1")?;
    for (key, shown) in changed {
        set.execute(params![key, shown])?;
    }
    Ok(())
}

/// Recomputes `category.shown` for the provider: its override if it has
/// one, else whether its name matches a rule of its kind.
fn apply_category_rules(tx: &Connection, provider_id: &str) -> rusqlite::Result<()> {
    let mut set = tx.prepare_cached(
        "UPDATE category SET shown = ?4
         WHERE provider_id = ?1 AND kind = ?2 AND category_id = ?3 AND shown != ?4",
    )?;
    for kind in Kind::ALL {
        let rules: Vec<String> = category_rules(tx, provider_id, kind)?
            .iter()
            .map(|r| fold(r))
            .collect();
        let cats: Vec<(String, String, Option<bool>)> = tx
            .prepare_cached(
                "SELECT k.category_id, k.name, o.shown FROM category k
                 LEFT JOIN category_override o
                     ON o.provider_id = k.provider_id AND o.kind = k.kind
                    AND o.category_id = k.category_id
                 WHERE k.provider_id = ?1 AND k.kind = ?2",
            )?
            .query_map(params![provider_id, kind], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (id, name, forced) in cats {
            let shown = forced.unwrap_or_else(|| matches_rules(&rules, &name));
            set.execute(params![provider_id, kind, id, shown])?;
        }
    }
    Ok(())
}

/// Like the favourites fallback (SPEC 5), inside an import's transaction:
/// an override of `kind` whose category is gone moves to the one category
/// of that kind with the same [`name_norm`]. Otherwise it stays, in case
/// the category comes back. Then the overrides' names are brought up to
/// date, for the next ID change.
fn repoint_category_overrides(
    tx: &Connection,
    provider_id: &str,
    kind: Kind,
) -> rusqlite::Result<()> {
    let stale: Vec<(String, String)> = tx
        .prepare(
            "SELECT category_id, name FROM category_override o WHERE provider_id = ?1 AND kind = ?2
             AND NOT EXISTS (SELECT 1 FROM category k
                             WHERE k.provider_id = o.provider_id AND k.kind = o.kind
                               AND k.category_id = o.category_id)",
        )?
        .query_map(params![provider_id, kind], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if !stale.is_empty() {
        let cats: Vec<(String, String)> = tx
            .prepare("SELECT category_id, name FROM category WHERE provider_id = ?1 AND kind = ?2")?
            .query_map(params![provider_id, kind], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (old, name) in stale {
            let norm = name_norm(&name);
            let mut matches = cats.iter().filter(|(_, n)| name_norm(n) == norm);
            let (Some((new, _)), None) = (matches.next(), matches.next()) else {
                continue;
            };
            // The new id has an override of its own: that one wins.
            tx.execute(
                "UPDATE OR IGNORE category_override SET category_id = ?4
                 WHERE provider_id = ?1 AND kind = ?2 AND category_id = ?3",
                params![provider_id, kind, old, new],
            )?;
            tx.execute(
                "DELETE FROM category_override WHERE provider_id = ?1 AND kind = ?2 AND category_id = ?3",
                params![provider_id, kind, old],
            )?;
        }
    }
    tx.execute(
        "UPDATE category_override SET name = k.name FROM category k
         WHERE category_override.provider_id = ?1 AND category_override.kind = ?2
           AND k.provider_id = category_override.provider_id
           AND k.kind = category_override.kind
           AND k.category_id = category_override.category_id",
        params![provider_id, kind],
    )?;
    Ok(())
}

/// A channel not in a hidden category. One whose category the provider does
/// not list is never hidden. The set is built once per query, and All's
/// indexes carry `category_id`, so no channel row is read.
const NOT_HIDDEN: &str = "(category_id IS NULL OR category_id NOT IN
     (SELECT category_id FROM category WHERE provider_id = ?1 AND kind = 'live' AND NOT shown))";

/// A channel no channel rule hides. Like [`NOT_HIDDEN`], a set built once
/// per query, from `channel_hidden`, which holds hidden channels only.
/// Named, as without statistics SQLite weighs it by the table's width and
/// since `archive_days` (migration 13) picks the primary key, reading every
/// channel to find the hidden ones.
const NOT_RULED: &str = "c.rowid NOT IN (SELECT rowid FROM channel INDEXED BY channel_hidden
     WHERE provider_id = ?1 AND NOT shown)";

/// The keys of a view's channels in display order: channel `rowid`s, and
/// for unavailable favourites the favourite's `rowid` negated. Favourites
/// and Recently played hold movies and series too, as [`list_key`]s (VOD
/// ticket 11). Keys only: rows are read on demand with [`channel`] and
/// [`title`] (ADR 0003).
pub fn channel_keys(
    conn: &Connection,
    provider_id: &str,
    view: View,
) -> rusqlite::Result<Vec<i64>> {
    keys(conn, provider_id, view, "")
}

/// As [`channel_keys`], only the channels with a guide: any programme in
/// the kept window, which is all the import keeps (SPEC 7.1). One seek on
/// the programme key per channel. Movies and series have none.
pub fn guide_keys(conn: &Connection, provider_id: &str, view: View) -> rusqlite::Result<Vec<i64>> {
    keys(conn, provider_id, view, HAS_GUIDE)
}

/// Channel `c` has programmes: every import that changes them sets
/// `guide_id` ([`resolve_guides`]).
const HAS_GUIDE: &str = "AND c.guide_id IS NOT NULL";

/// A view's keys, with `and` (empty, or a condition on channel `c`) added.
fn keys(conn: &Connection, provider_id: &str, view: View, and: &str) -> rusqlite::Result<Vec<i64>> {
    let key = |r: &rusqlite::Row| r.get(0);
    match view {
        // Channels sharing a number break ties by `category_id` before
        // `rowid`: All's indexes end in `category_id` (migration 8), so this
        // order is read from them with no sort.
        View::All(Order::Provider) => conn
            .prepare_cached(&format!(
                "SELECT rowid FROM channel c
                 WHERE provider_id = ?1 AND {NOT_HIDDEN} AND {NOT_RULED} {and}
                 ORDER BY num, category_id, rowid"
            ))?
            .query_map([provider_id], key)?
            .collect(),
        View::All(Order::Az) => conn
            .prepare_cached(&format!(
                "SELECT rowid FROM channel c
                 WHERE provider_id = ?1 AND {NOT_HIDDEN} AND {NOT_RULED} {and}
                 ORDER BY name_norm, num, category_id, rowid"
            ))?
            .query_map([provider_id], key)?
            .collect(),
        View::Category(id, Order::Provider) => conn
            .prepare_cached(&format!(
                "SELECT rowid FROM channel c
                 WHERE provider_id = ?1 AND category_id = ?2 AND {NOT_RULED} {and}
                 ORDER BY num, rowid"
            ))?
            .query_map([provider_id, id], key)?
            .collect(),
        View::Category(id, Order::Az) => conn
            .prepare_cached(&format!(
                "SELECT rowid FROM channel c
                 WHERE provider_id = ?1 AND category_id = ?2 AND {NOT_RULED} {and}
                 ORDER BY name_norm, num, rowid"
            ))?
            .query_map([provider_id, id], key)?
            .collect(),
        View::Favourites => conn
            .prepare_cached(&format!(
                "SELECT {LIST_KEY}, -f.rowid) FROM favourite f {KINDS_OF_F}
                 WHERE f.provider_id = ?1 {and} ORDER BY f.position"
            ))?
            .query_map([provider_id], key)?
            .collect(),
        View::Recent => conn
            .prepare_cached(&format!(
                "SELECT {LIST_KEY}) FROM history f {KINDS_OF_F}
                 WHERE f.provider_id = ?1 AND {LIST_KEY}) IS NOT NULL {and}
                 ORDER BY f.played_at DESC, f.rowid DESC"
            ))?
            .query_map([provider_id], key)?
            .collect(),
    }
}

/// Where the keys of movies and series start in the lists that hold
/// several kinds: Favourites, Recently played (VOD ticket 11) and Continue
/// watching. A channel's key is its row key there too, so channel code
/// reads them as it reads any list's; row keys never come near 2^40.
const MOVIE_KEYS: i64 = 1 << 40;
const SERIES_KEYS: i64 = 2 << 40;

/// A key in a list that holds several kinds: a channel's row key, or a
/// movie's or series' moved into its own range.
pub fn list_key(kind: Kind, rowid: i64) -> i64 {
    match kind {
        Kind::Live => rowid,
        Kind::Movie => MOVIE_KEYS + rowid,
        Kind::Series => SERIES_KEYS + rowid,
    }
}

/// What a [`list_key`] is a key of, and its row key. An unavailable
/// favourite's key, negative, reads as a channel's (see [`channel`]).
pub fn from_list_key(key: i64) -> (Kind, i64) {
    if key >= SERIES_KEYS {
        (Kind::Series, key - SERIES_KEYS)
    } else if key >= MOVIE_KEYS {
        (Kind::Movie, key - MOVIE_KEYS)
    } else {
        (Kind::Live, key)
    }
}

/// The [`list_key`] of favourite or history row `f`, left open for a
/// fallback; NULL if its title is gone. `c` is its channel, if live.
const LIST_KEY: &str = "coalesce(c.rowid, m.rowid + 1099511627776, s.rowid + 2199023255552";

/// Joins favourite or history row `f` to its title, by its kind.
const KINDS_OF_F: &str = "
    LEFT JOIN channel c ON f.kind = 'live' AND c.provider_id = f.provider_id AND c.stream_id = f.stream_id
    LEFT JOIN movie m ON f.kind = 'movie' AND m.provider_id = f.provider_id AND m.stream_id = f.stream_id
    LEFT JOIN series s ON f.kind = 'series' AND s.provider_id = f.provider_id AND s.series_id = f.stream_id";

/// How many titles Recently added lists (VOD ticket 03).
pub const RECENTLY_ADDED: usize = 200;

/// Which movies or series a list shows (VOD ticket 03).
pub enum TitleView<'a> {
    All(Order),
    /// The latest [`RECENTLY_ADDED`], newest first.
    Added,
    Category(&'a str, Order),
}

/// The keys of a view's movies or series in display order: their `rowid`s.
/// Keys only, as for channels: rows are read on demand with [`title`]
/// (ADR 0003). Each order is read from an index (migration 18).
pub fn title_keys(
    conn: &Connection,
    provider_id: &str,
    kind: Kind,
    view: TitleView,
) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare_cached(&title_query(kind, &view))?;
    let key = |r: &rusqlite::Row| r.get(0);
    match view {
        TitleView::Category(id, _) => stmt.query_map([provider_id, id], key)?.collect(),
        TitleView::All(_) | TitleView::Added => stmt.query_map([provider_id], key)?.collect(),
    }
}

/// [`title_keys`]' query: `?1` the provider, `?2` a category's id.
fn title_query(kind: Kind, view: &TitleView) -> String {
    debug_assert!(kind != Kind::Live, "channels have channel_keys");
    let table = kind.table();
    // As NOT_HIDDEN, for the kind's categories.
    let not_hidden = format!(
        "(category_id IS NULL OR category_id NOT IN
             (SELECT category_id FROM category WHERE provider_id = ?1 AND kind = '{}' AND NOT shown))",
        kind.as_str()
    );
    match view {
        TitleView::All(order) => {
            let by = match order {
                Order::Provider => "num, category_id, rowid",
                Order::Az => "name_norm, num, category_id, rowid",
            };
            format!(
                "SELECT rowid FROM {table} WHERE provider_id = ?1 AND {not_hidden} ORDER BY {by}"
            )
        }
        TitleView::Added => format!(
            "SELECT rowid FROM {table}
             WHERE provider_id = ?1 AND {added} IS NOT NULL AND {not_hidden}
             ORDER BY {added} DESC, category_id DESC, rowid DESC LIMIT {RECENTLY_ADDED}",
            added = kind.added(),
        ),
        TitleView::Category(_, order) => {
            let by = match order {
                Order::Provider => "num, rowid",
                Order::Az => "name_norm, num, rowid",
            };
            format!(
                "SELECT rowid FROM {table} WHERE provider_id = ?1 AND category_id = ?2 ORDER BY {by}"
            )
        }
    }
}

/// A movie or series as its row and the detail panel show it: the list's
/// fields (SPEC 5), and its info once fetched (VOD tickets 04 and 05).
#[derive(Debug, Default, PartialEq)]
pub struct TitleRow {
    pub stream_id: i64,
    pub name: String,
    pub category: Option<String>,
    /// Out of 10.
    pub rating: Option<f64>,
    pub year: Option<i64>,
    pub poster_url: Option<String>,
    /// A movie's container extension.
    pub extension: Option<String>,
    /// When the provider added the movie, or last changed the series.
    pub added: Option<i64>,
    pub genre: Option<String>,
    pub plot: Option<String>,
    pub actors: Option<String>,
    pub director: Option<String>,
    /// A series' episode length in minutes.
    pub run_time: Option<i64>,
    /// A movie's length in seconds, from its info's `duration`.
    pub length: Option<i64>,
    /// How many seasons the provider has of a series, from its info.
    pub seasons: Option<i64>,
    /// The year of a series' latest episode, from its info.
    pub last_year: Option<i64>,
    /// Its info has been fetched since the list was imported.
    pub info: bool,
    pub provider_id: String,
    /// Where a movie was stopped, if it was played (VOD ticket 07).
    pub progress: Option<Progress>,
    pub favourite: bool,
}

/// A movie or series by a key from [`title_keys`].
pub fn title(conn: &Connection, kind: Kind, key: i64) -> rusqlite::Result<Option<TitleRow>> {
    let sql = match kind {
        Kind::Movie => {
            "SELECT t.name, cat.name, t.rating, t.year, t.poster_url, t.extension, t.added,
                    t.genre, t.plot, t.actors, t.director, NULL, t.stream_id, t.length,
                    t.info_at IS NOT NULL, NULL, NULL, t.provider_id, p.pos, p.length, p.watched,
                    EXISTS (SELECT 1 FROM favourite f WHERE f.provider_id = t.provider_id
                            AND f.kind = 'movie' AND f.stream_id = t.stream_id)
             FROM movie t
             LEFT JOIN category cat ON cat.provider_id = t.provider_id AND cat.kind = 'movie'
                 AND cat.category_id = t.category_id
             LEFT JOIN progress p ON p.provider_id = t.provider_id AND p.kind = 'movie'
                 AND p.stream_id = t.stream_id
             WHERE t.rowid = ?1"
        }
        Kind::Series => {
            "SELECT t.name, cat.name, t.rating, t.year, t.cover_url, NULL, t.last_modified,
                    t.genre, t.plot, t.actors, t.director, t.episode_run_time, t.series_id,
                    NULL, t.info_at IS NOT NULL, t.seasons, t.last_year, t.provider_id,
                    NULL, NULL, NULL,
                    EXISTS (SELECT 1 FROM favourite f WHERE f.provider_id = t.provider_id
                            AND f.kind = 'series' AND f.stream_id = t.series_id)
             FROM series t
             LEFT JOIN category cat ON cat.provider_id = t.provider_id AND cat.kind = 'series'
                 AND cat.category_id = t.category_id
             WHERE t.rowid = ?1"
        }
        Kind::Live => unreachable!("channels are read with channel"),
    };
    // Empty strings from the list read as missing.
    let text = |r: &rusqlite::Row, i| -> rusqlite::Result<Option<String>> {
        Ok(r.get::<_, Option<String>>(i)?
            .filter(|s| !s.trim().is_empty()))
    };
    conn.prepare_cached(sql)?
        .query_row([key], |r| {
            Ok(TitleRow {
                name: r.get(0)?,
                category: r.get(1)?,
                rating: r.get(2)?,
                year: r.get(3)?,
                poster_url: text(r, 4)?,
                extension: text(r, 5)?,
                added: r.get(6)?,
                genre: text(r, 7)?,
                plot: text(r, 8)?,
                actors: text(r, 9)?,
                director: text(r, 10)?,
                run_time: r.get(11)?,
                stream_id: r.get(12)?,
                length: r.get(13)?,
                info: r.get(14)?,
                seasons: r.get(15)?,
                last_year: r.get(16)?,
                provider_id: r.get(17)?,
                progress: match r.get::<_, Option<i64>>(18)? {
                    Some(pos) => Some(Progress {
                        pos,
                        length: r.get(19)?,
                        watched: r.get(20)?,
                    }),
                    None => None,
                },
                favourite: r.get(21)?,
            })
        })
        .optional()
}

/// A movie's info from `get_vod_info` (SPEC 6.1). Empty text is none.
#[derive(Debug, Default, PartialEq)]
pub struct MovieInfo {
    pub year: Option<i64>,
    /// Seconds, from `duration`.
    pub length: Option<i64>,
    pub genre: Option<String>,
    pub plot: Option<String>,
    pub actors: Option<String>,
    pub director: Option<String>,
}

/// Stores movie `stream_id`'s info on its row, if the list still has it.
pub fn set_movie_info(
    conn: &Connection,
    provider_id: &str,
    stream_id: i64,
    info: &MovieInfo,
    now: i64,
) -> rusqlite::Result<()> {
    conn.prepare_cached(
        "UPDATE movie SET year = ?3, length = ?4, genre = ?5, plot = ?6, actors = ?7,
             director = ?8, info_at = ?9
         WHERE provider_id = ?1 AND stream_id = ?2",
    )?
    .execute(params![
        provider_id,
        stream_id,
        info.year,
        info.length,
        info.genre,
        info.plot,
        info.actors,
        info.director,
        now,
    ])?;
    Ok(())
}

/// An episode from a series' `get_series_info` (SPEC 6.1).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Episode {
    /// The `episodes` key it is listed under.
    pub season: i64,
    pub num: Option<i64>,
    /// Its `id`: the stream id, not its info's `id`.
    pub stream_id: i64,
    /// The provider's whole title, often longer than the episode's name.
    pub title: String,
    pub extension: Option<String>,
    /// Seconds, from its info's `duration`.
    pub length: Option<i64>,
}

/// A series' info: its episodes in the provider's order, and the latest
/// one's year.
#[derive(Debug, Default, PartialEq)]
pub struct SeriesInfo {
    pub episodes: Vec<Episode>,
    pub last_year: Option<i64>,
}

/// Stores series `series_id`'s info, replacing any it had, if the list
/// still has it.
pub fn set_series_info(
    conn: &mut Connection,
    provider_id: &str,
    series_id: i64,
    info: &SeriesInfo,
    now: i64,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let mut seasons: Vec<i64> = info.episodes.iter().map(|e| e.season).collect();
    seasons.sort_unstable();
    seasons.dedup();
    let found = tx
        .prepare_cached(
            "UPDATE series SET seasons = ?3, last_year = ?4, info_at = ?5
             WHERE provider_id = ?1 AND series_id = ?2",
        )?
        .execute(params![
            provider_id,
            series_id,
            seasons.len() as i64,
            info.last_year,
            now
        ])?;
    if found == 0 {
        return Ok(());
    }
    tx.execute(
        "DELETE FROM episode WHERE provider_id = ?1 AND series_id = ?2",
        params![provider_id, series_id],
    )?;
    {
        let mut ins = tx.prepare_cached(
            "INSERT OR REPLACE INTO episode (provider_id, series_id, season, episode_num,
                 stream_id, title, extension, length, sort)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        for (sort, e) in info.episodes.iter().enumerate() {
            ins.execute(params![
                provider_id,
                series_id,
                e.season,
                e.num,
                e.stream_id,
                e.title,
                e.extension,
                e.length,
                sort as i64,
            ])?;
        }
    }
    tx.commit()
}

/// A series' episodes as stored by [`set_series_info`]: by season, then
/// number, then the provider's order. One series' worth, so read whole.
pub fn episodes(
    conn: &Connection,
    provider_id: &str,
    series_id: i64,
) -> rusqlite::Result<Vec<Episode>> {
    conn.prepare_cached(
        "SELECT season, episode_num, stream_id, title, extension, length FROM episode
         WHERE provider_id = ?1 AND series_id = ?2
         ORDER BY season, episode_num IS NULL, episode_num, sort",
    )?
    .query_map(params![provider_id, series_id], |r| {
        Ok(Episode {
            season: r.get(0)?,
            num: r.get(1)?,
            stream_id: r.get(2)?,
            title: r.get(3)?,
            extension: r.get::<_, Option<String>>(4)?.filter(|e| !e.is_empty()),
            length: r.get(5)?,
        })
    })?
    .collect()
}

/// A movie or an episode, as its progress is kept (VOD ticket 07).
#[derive(Clone, Debug, PartialEq)]
pub enum Watchable {
    Movie(i64),
    Episode {
        series_id: i64,
        stream_id: i64,
        season: i64,
        num: Option<i64>,
    },
}

impl Watchable {
    fn kind(&self) -> &'static str {
        match self {
            Watchable::Movie(_) => "movie",
            Watchable::Episode { .. } => "episode",
        }
    }

    fn stream_id(&self) -> i64 {
        match self {
            Watchable::Movie(id) | Watchable::Episode { stream_id: id, .. } => *id,
        }
    }
}

/// Where a movie or episode was stopped. `pos` is 0 when it starts from
/// the beginning: never played past [`RESUME_FROM`], or just watched.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Progress {
    pub pos: i64,
    /// mpv's `duration`, once it was known.
    pub length: Option<i64>,
    /// Watched to the end once, kept when it is watched again.
    pub watched: bool,
}

/// A position before this many seconds in is not kept: it plays from the
/// start.
pub const RESUME_FROM: i64 = 60;

/// Whether `pos` of `length` seconds counts as watched (SPEC 5): past 90 %
/// or in the last 5 minutes, whichever comes first, the 5 minutes never
/// more than a quarter of it, so a short one is not watched as it starts.
pub fn is_watched(pos: f64, length: f64) -> bool {
    if length <= 0.0 {
        return false;
    }
    let tail = (length * 0.1).max(300f64.min(length / 4.0));
    length - pos <= tail
}

/// Keeps where `item` stopped at `at`: `pos` of mpv's `length`, or with
/// `ended` played to its end. A play that did not get past
/// [`RESUME_FROM`] keeps the time it was last played, so a glance at an
/// earlier episode does not make it the series' latest.
pub fn save_progress(
    conn: &Connection,
    provider_id: &str,
    item: &Watchable,
    pos: f64,
    length: Option<f64>,
    ended: bool,
    at: i64,
) -> rusqlite::Result<()> {
    let watched = ended || length.is_some_and(|l| is_watched(pos, l));
    let pos = pos as i64;
    let pos = if watched || pos < RESUME_FROM { 0 } else { pos };
    let (series_id, season, num) = match item {
        Watchable::Movie(_) => (None, None, None),
        Watchable::Episode {
            series_id,
            season,
            num,
            ..
        } => (Some(*series_id), Some(*season), *num),
    };
    conn.prepare_cached(
        "INSERT INTO progress (provider_id, kind, stream_id, series_id, season, episode_num,
             pos, length, watched, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (provider_id, kind, stream_id) DO UPDATE SET
             series_id = ?4, season = ?5, episode_num = ?6, pos = ?7,
             length = coalesce(?8, length), watched = watched OR ?9,
             at = CASE WHEN ?7 > 0 OR ?9 THEN ?10 ELSE at END",
    )?
    .execute(params![
        provider_id,
        item.kind(),
        item.stream_id(),
        series_id,
        season,
        num,
        pos,
        length.map(|l| l as i64),
        watched,
        at,
    ])?;
    Ok(())
}

/// Marks `items` watched at `at`, from the start, or unwatched: forgotten,
/// resume point and all (`w`, VOD ticket 07).
pub fn set_watched(
    conn: &mut Connection,
    provider_id: &str,
    items: &[Watchable],
    watched: bool,
    at: i64,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for item in items {
        if watched {
            save_progress(&tx, provider_id, item, 0.0, None, true, at)?;
        } else {
            tx.prepare_cached(
                "DELETE FROM progress WHERE provider_id = ?1 AND kind = ?2 AND stream_id = ?3",
            )?
            .execute(params![provider_id, item.kind(), item.stream_id()])?;
        }
    }
    tx.commit()
}

/// Where `item` was stopped, if it was played. The app reads it with the
/// title's row ([`title`]) or the series' ([`episode_progress`]).
#[cfg(test)]
pub fn progress(
    conn: &Connection,
    provider_id: &str,
    item: &Watchable,
) -> rusqlite::Result<Option<Progress>> {
    conn.prepare_cached(
        "SELECT pos, length, watched FROM progress
         WHERE provider_id = ?1 AND kind = ?2 AND stream_id = ?3",
    )?
    .query_row(params![provider_id, item.kind(), item.stream_id()], |r| {
        Ok(Progress {
            pos: r.get(0)?,
            length: r.get(1)?,
            watched: r.get(2)?,
        })
    })
    .optional()
}

/// Series `series_id`'s episodes played, by stream id.
pub fn episode_progress(
    conn: &Connection,
    provider_id: &str,
    series_id: i64,
) -> rusqlite::Result<HashMap<i64, Progress>> {
    conn.prepare_cached(
        "SELECT stream_id, pos, length, watched FROM progress
         WHERE provider_id = ?1 AND kind = 'episode' AND series_id = ?2",
    )?
    .query_map(params![provider_id, series_id], |r| {
        Ok((
            r.get(0)?,
            Progress {
                pos: r.get(1)?,
                length: r.get(2)?,
                watched: r.get(3)?,
            },
        ))
    })?
    .collect()
}

/// The episode of a series played last, part way or watched.
#[derive(Clone, Debug, PartialEq)]
pub struct LastEpisode {
    pub stream_id: i64,
    pub season: i64,
    pub num: Option<i64>,
    pub progress: Progress,
}

/// Series `series_id`'s episode played last, ignoring one stopped before
/// [`RESUME_FROM`] that was never watched; of several at once (a season
/// marked watched), the furthest on.
pub fn last_episode(
    conn: &Connection,
    provider_id: &str,
    series_id: i64,
) -> rusqlite::Result<Option<LastEpisode>> {
    conn.prepare_cached(
        "SELECT stream_id, season, episode_num, pos, length, watched FROM progress
         WHERE provider_id = ?1 AND kind = 'episode' AND series_id = ?2
             AND (pos > 0 OR watched)
         ORDER BY at DESC, season DESC, episode_num DESC, rowid DESC LIMIT 1",
    )?
    .query_row(params![provider_id, series_id], |r| {
        Ok(LastEpisode {
            stream_id: r.get(0)?,
            season: r.get(1)?,
            num: r.get(2)?,
            progress: Progress {
                pos: r.get(3)?,
                length: r.get(4)?,
                watched: r.get(5)?,
            },
        })
    })
    .optional()
}

/// Where a series carries on (VOD ticket 07).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum UpNext {
    /// Nothing of it played.
    #[default]
    Nothing,
    /// Part way through this episode.
    Resume(Episode, Progress),
    /// The episode after the last one watched.
    Next(Episode),
    /// Its last episode was watched.
    Finished,
    /// Something was played, but its episodes are not read or no longer
    /// have it.
    Unknown,
}

/// Where a series with `episodes` (as [`episodes`] orders them) carries on
/// after `last`: the episode itself while part way, else the one after it,
/// into the next season at a season's end. Found by stream id, or by
/// season and number if the provider renumbered its streams.
pub fn up_next(episodes: &[Episode], last: Option<&LastEpisode>) -> UpNext {
    let Some(last) = last else {
        return UpNext::Nothing;
    };
    let at = episodes
        .iter()
        .position(|e| e.stream_id == last.stream_id)
        .or_else(|| {
            last.num.and_then(|n| {
                episodes
                    .iter()
                    .position(|e| e.season == last.season && e.num == Some(n))
            })
        });
    let Some(at) = at else {
        return UpNext::Unknown;
    };
    if last.progress.pos > 0 {
        return UpNext::Resume(episodes[at].clone(), last.progress);
    }
    match episodes.get(at + 1) {
        Some(e) => UpNext::Next(e.clone()),
        None => UpNext::Finished,
    }
}

/// Series `series_id`'s [`UpNext`], from its episodes if its info is in.
pub fn series_up_next(
    conn: &Connection,
    provider_id: &str,
    series_id: i64,
) -> rusqlite::Result<UpNext> {
    let Some(last) = last_episode(conn, provider_id, series_id)? else {
        return Ok(UpNext::Nothing);
    };
    let episodes = episodes(conn, provider_id, series_id)?;
    if episodes.is_empty() {
        return Ok(UpNext::Unknown);
    }
    Ok(up_next(&episodes, Some(&last)))
}

/// Continue watching (SPEC 8.2): the movies stopped part way and the
/// series with an episode to carry on with, latest first, as
/// [`list_key`]s. A series whose episodes are not read yet is listed
/// while its last episode is known; one finished is not. Titles the lists
/// no longer have are left out, and so is what was last played before it
/// was cleared ([`clear_continue`]).
pub fn continue_watching(conn: &Connection, provider_id: &str) -> rusqlite::Result<Vec<i64>> {
    let cleared: i64 = conn
        .query_row(
            "SELECT value FROM app_state WHERE key = ?1",
            [format!("{CONTINUE_CLEARED}{provider_id}")],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|v| v.parse().ok())
        .unwrap_or(i64::MIN);
    let mut found: Vec<(i64, i64)> = conn
        .prepare_cached(
            "SELECT m.rowid, p.at FROM progress p
             JOIN movie m ON m.provider_id = p.provider_id AND m.stream_id = p.stream_id
             WHERE p.provider_id = ?1 AND p.kind = 'movie' AND p.pos > 0 AND p.at > ?2",
        )?
        .query_map(params![provider_id, cleared], |r| {
            Ok((list_key(Kind::Movie, r.get(0)?), r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let series: Vec<(i64, i64, i64)> = conn
        .prepare_cached(
            "SELECT s.rowid, s.series_id, max(p.at) FROM progress p
             JOIN series s ON s.provider_id = p.provider_id AND s.series_id = p.series_id
             WHERE p.provider_id = ?1 AND p.kind = 'episode' AND (p.pos > 0 OR p.watched)
             GROUP BY s.rowid HAVING max(p.at) > ?2",
        )?
        .query_map(params![provider_id, cleared], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (rowid, series_id, at) in series {
        if series_up_next(conn, provider_id, series_id)? != UpNext::Finished {
            found.push((list_key(Kind::Series, rowid), at));
        }
    }
    found.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
    Ok(found.into_iter().map(|(k, _)| k).collect())
}

/// Empties the provider's Continue watching as of `at`. Positions and
/// watched marks stay; a title played again comes back.
pub fn clear_continue(conn: &Connection, provider_id: &str, at: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_state (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![format!("{CONTINUE_CLEARED}{provider_id}"), at.to_string()],
    )?;
    Ok(())
}

/// The channel with provider number `num` (the first, if several share it).
pub fn channel_by_num(
    conn: &Connection,
    provider_id: &str,
    num: i64,
) -> rusqlite::Result<Option<i64>> {
    conn.prepare_cached(
        "SELECT rowid FROM channel WHERE provider_id = ?1 AND num = ?2 ORDER BY rowid LIMIT 1",
    )?
    .query_row(params![provider_id, num], |r| r.get(0))
    .optional()
}

/// The row key of the provider's channel `stream_id`, if it still exists.
pub fn channel_key(
    conn: &Connection,
    provider_id: &str,
    stream_id: i64,
) -> rusqlite::Result<Option<i64>> {
    conn.prepare_cached("SELECT rowid FROM channel WHERE provider_id = ?1 AND stream_id = ?2")?
        .query_row(params![provider_id, stream_id], |r| r.get(0))
        .optional()
}

/// A channel and what it airs at `now`, by a key from [`channel_keys`].
pub fn channel(conn: &Connection, key: i64, now: i64) -> rusqlite::Result<Option<ChannelRow>> {
    if key < 0 {
        return conn
            .prepare_cached("SELECT stream_id, name, kind FROM favourite WHERE rowid = ?1")?
            .query_row([-key], |r| {
                Ok(ChannelRow {
                    stream_id: r.get(0)?,
                    num: None,
                    name: r.get(1)?,
                    extension: String::new(),
                    category: None,
                    epg_channel_id: None,
                    guide_id: None,
                    icon_url: None,
                    archive_days: 0,
                    now: None,
                    favourite: true,
                    available: false,
                    kind: r.get(2)?,
                })
            })
            .optional();
    }
    conn.prepare_cached(
        "SELECT c.stream_id, c.num, c.name, c.extension, cat.name, c.epg_channel_id,
                p.start, p.stop, p.title,
                EXISTS (SELECT 1 FROM favourite f
                        WHERE f.provider_id = c.provider_id AND f.kind = 'live'
                          AND f.stream_id = c.stream_id),
                c.icon_url, c.archive_days, p.archive, c.guide_id
         FROM channel c
         LEFT JOIN category cat ON cat.provider_id = c.provider_id AND cat.kind = 'live' AND cat.category_id = c.category_id
         LEFT JOIN programme p ON p.provider_id = c.provider_id AND p.epg_channel_id = c.guide_id
             AND p.start = (SELECT max(start) FROM programme
                            WHERE provider_id = c.provider_id AND epg_channel_id = c.guide_id
                              AND start <= ?2)
             AND p.stop > ?2
         WHERE c.rowid = ?1",
    )?
    .query_row(params![key, now], |r| {
        let now = match r.get::<_, Option<i64>>(6)? {
            Some(start) => Some(Programme {
                start,
                stop: r.get(7)?,
                title: r.get(8)?,
                archive: r.get(12)?,
            }),
            None => None,
        };
        Ok(ChannelRow {
            stream_id: r.get(0)?,
            num: r.get(1)?,
            name: r.get(2)?,
            extension: r.get(3)?,
            category: r.get(4)?,
            epg_channel_id: r.get(5)?,
            guide_id: r.get(13)?,
            icon_url: r.get(10)?,
            archive_days: r.get(11)?,
            now,
            favourite: r.get(9)?,
            available: true,
            kind: Kind::Live,
        })
    })
    .optional()
}

/// A channel's logo URL, for fetching ahead of the rows on screen.
pub fn icon_url(conn: &Connection, key: i64) -> rusqlite::Result<Option<String>> {
    conn.prepare_cached("SELECT icon_url FROM channel WHERE rowid = ?1")?
        .query_row([key], |r| r.get(0))
        .optional()
        .map(Option::flatten)
}

/// A movie's poster or a series' cover by a key from [`title_keys`] (VOD
/// ticket 06); none if empty.
pub fn poster_url(conn: &Connection, kind: Kind, key: i64) -> rusqlite::Result<Option<String>> {
    let sql = match kind {
        Kind::Movie => "SELECT poster_url FROM movie WHERE rowid = ?1",
        Kind::Series => "SELECT cover_url FROM series WHERE rowid = ?1",
        Kind::Live => unreachable!("channels have logos"),
    };
    conn.prepare_cached(sql)?
        .query_row([key], |r| r.get::<_, Option<String>>(0))
        .optional()
        .map(|u| u.flatten().filter(|u| !u.trim().is_empty()))
}

/// The same by the movie's `stream_id` or the series' `series_id`.
pub fn poster_url_by_id(
    conn: &Connection,
    provider_id: &str,
    kind: Kind,
    id: i64,
) -> rusqlite::Result<Option<String>> {
    let sql = match kind {
        Kind::Movie => "SELECT poster_url FROM movie WHERE provider_id = ?1 AND stream_id = ?2",
        Kind::Series => "SELECT cover_url FROM series WHERE provider_id = ?1 AND series_id = ?2",
        Kind::Live => unreachable!("channels have logos"),
    };
    conn.prepare_cached(sql)?
        .query_row(params![provider_id, id], |r| r.get::<_, Option<String>>(0))
        .optional()
        .map(|u| u.flatten().filter(|u| !u.trim().is_empty()))
}

/// A channel's programmes starting after `now` and before `until`, in
/// order, or at least the first one after `now` if it starts later, so there
/// is always a "next" when the guide has one.
pub fn later(
    conn: &Connection,
    provider_id: &str,
    epg_channel_id: &str,
    now: i64,
    until: i64,
) -> rusqlite::Result<Vec<Programme>> {
    conn.prepare_cached(
        "SELECT start, stop, title, archive FROM programme
         WHERE provider_id = ?1 AND epg_channel_id = ?2 AND start > ?3
           AND (start < ?4 OR start = (SELECT min(start) FROM programme
                                       WHERE provider_id = ?1 AND epg_channel_id = ?2 AND start > ?3))
         ORDER BY start",
    )?
    .query_map(params![provider_id, epg_channel_id, now, until], |r| {
        Ok(Programme {
            start: r.get(0)?,
            stop: r.get(1)?,
            title: r.get(2)?,
            archive: r.get(3)?,
        })
    })?
    .collect()
}

/// A channel's programmes airing at any time in `from..to`, by start. The
/// guide grid's row for that span (ADR 0003: read per row).
pub fn programmes(
    conn: &Connection,
    provider_id: &str,
    epg_channel_id: &str,
    from: i64,
    to: i64,
) -> rusqlite::Result<Vec<Programme>> {
    conn.prepare_cached(
        "SELECT start, stop, title, archive FROM programme
         WHERE provider_id = ?1 AND epg_channel_id = ?2 AND start < ?4 AND stop > ?3
         ORDER BY start",
    )?
    .query_map(params![provider_id, epg_channel_id, from, to], |r| {
        Ok(Programme {
            start: r.get(0)?,
            stop: r.get(1)?,
            title: r.get(2)?,
            archive: r.get(3)?,
        })
    })?
    .collect()
}

/// The description of a channel's programme starting at `start`, if the
/// guide has one. The guide grid's detail strip (SPEC 8.2).
pub fn description(
    conn: &Connection,
    provider_id: &str,
    epg_channel_id: &str,
    start: i64,
) -> rusqlite::Result<Option<String>> {
    conn.prepare_cached(
        "SELECT description FROM programme
         WHERE provider_id = ?1 AND epg_channel_id = ?2 AND start = ?3",
    )?
    .query_row(params![provider_id, epg_channel_id, start], |r| r.get(0))
    .optional()
    .map(Option::flatten)
}

/// Adds the channel, movie or series to the end of the favourites, or
/// removes it. Returns whether it is now a favourite. An unavailable
/// favourite can be removed. A series' `stream_id` is its `series_id`.
pub fn toggle_favourite(
    conn: &Connection,
    provider_id: &str,
    kind: Kind,
    stream_id: i64,
) -> rusqlite::Result<bool> {
    let removed = conn.execute(
        "DELETE FROM favourite WHERE provider_id = ?1 AND kind = ?2 AND stream_id = ?3",
        params![provider_id, kind, stream_id],
    )?;
    if removed > 0 {
        return Ok(false);
    }
    let added = conn.execute(
        &format!(
            "INSERT INTO favourite (provider_id, kind, stream_id, name_norm, group_id, position, name)
             SELECT provider_id, ?2, {id}, name_norm, NULL,
                    (SELECT coalesce(max(position), 0) + 1 FROM favourite WHERE provider_id = ?1),
                    name
             FROM {table} WHERE provider_id = ?1 AND {id} = ?3",
            id = kind.id_column(),
            table = kind.table(),
        ),
        params![provider_id, kind, stream_id],
    )?;
    Ok(added > 0)
}

/// Swaps a favourite with its neighbour: `delta` 1 moves it down, -1 up.
/// Returns false if it was already at that end.
pub fn move_favourite(
    conn: &mut Connection,
    provider_id: &str,
    kind: Kind,
    stream_id: i64,
    delta: isize,
) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let Some(pos): Option<i64> = tx
        .query_row(
            "SELECT position FROM favourite WHERE provider_id = ?1 AND kind = ?2 AND stream_id = ?3",
            params![provider_id, kind, stream_id],
            |r| r.get(0),
        )
        .optional()?
    else {
        return Ok(false);
    };
    let neighbour = if delta > 0 {
        "SELECT rowid, position FROM favourite WHERE provider_id = ?1 AND position > ?2
         ORDER BY position LIMIT 1"
    } else {
        "SELECT rowid, position FROM favourite WHERE provider_id = ?1 AND position < ?2
         ORDER BY position DESC LIMIT 1"
    };
    let Some((other, other_pos)): Option<(i64, i64)> = tx
        .query_row(neighbour, params![provider_id, pos], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?
    else {
        return Ok(false);
    };
    tx.execute(
        "UPDATE favourite SET position = ?4 WHERE provider_id = ?1 AND kind = ?2 AND stream_id = ?3",
        params![provider_id, kind, stream_id, other_pos],
    )?;
    tx.execute(
        "UPDATE favourite SET position = ?2 WHERE rowid = ?1",
        params![other, pos],
    )?;
    tx.commit()?;
    Ok(true)
}

/// Records a play of the channel, movie or series (an episode's series)
/// at `at`, keeping the provider's [`HISTORY_KEPT`] most recent. A
/// series' `stream_id` is its `series_id`.
pub fn record_play(
    conn: &Connection,
    provider_id: &str,
    kind: Kind,
    stream_id: i64,
    at: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO history (provider_id, kind, stream_id, played_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![provider_id, kind, stream_id, at],
    )?;
    conn.execute(
        "DELETE FROM history WHERE provider_id = ?1 AND rowid NOT IN
             (SELECT rowid FROM history WHERE provider_id = ?1
              ORDER BY played_at DESC, rowid DESC LIMIT ?2)",
        params![provider_id, HISTORY_KEPT],
    )?;
    Ok(())
}

/// Empties the provider's recently played.
pub fn clear_history(conn: &Connection, provider_id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM history WHERE provider_id = ?1", [provider_id])?;
    Ok(())
}

/// Row keys of the `limit` channels played most recently, latest first:
/// the detail panel's RECENTLY PLAYED, which is for channels only.
pub fn recent(conn: &Connection, provider_id: &str, limit: usize) -> rusqlite::Result<Vec<i64>> {
    conn.prepare_cached(
        "SELECT c.rowid FROM history h
         JOIN channel c ON c.provider_id = h.provider_id AND c.stream_id = h.stream_id
         WHERE h.provider_id = ?1 AND h.kind = 'live'
         ORDER BY h.played_at DESC, h.rowid DESC LIMIT ?2",
    )?
    .query_map(params![provider_id, limit as i64], |r| r.get(0))?
    .collect()
}

/// The `-` key: the row key of the channel played most recently other than
/// `current` (the one playing, if any). Movies and series are left out.
pub fn last_channel(
    conn: &Connection,
    provider_id: &str,
    current: Option<i64>,
) -> rusqlite::Result<Option<i64>> {
    conn.prepare_cached(
        "SELECT c.rowid FROM history h
         JOIN channel c ON c.provider_id = h.provider_id AND c.stream_id = h.stream_id
         WHERE h.provider_id = ?1 AND h.kind = 'live' AND h.stream_id IS NOT ?2
         ORDER BY h.played_at DESC, h.rowid DESC LIMIT 1",
    )?
    .query_row(params![provider_id, current], |r| r.get(0))
    .optional()
}

/// Channel `key`'s alternates (stream recovery ticket 03): the other
/// shown channels with its looser name form (SPEC 6.1), at most `limit`.
/// Those with its `name_norm` first, then its own category's, then in
/// provider order. Hidden ones are left out row by row: a name has few
/// listings, and this runs on each move in the list.
pub fn alternates(
    conn: &Connection,
    provider_id: &str,
    key: i64,
    limit: usize,
) -> rusqlite::Result<Vec<i64>> {
    conn.prepare_cached(
        "SELECT c.rowid FROM channel c,
             (SELECT rowid AS r, name_loose AS loose, name_norm AS norm, category_id AS cat
              FROM channel WHERE rowid = ?2) a
         WHERE c.provider_id = ?1 AND c.name_loose = a.loose AND a.loose != ''
             AND c.rowid != a.r AND c.shown
             AND NOT EXISTS (SELECT 1 FROM category g
                             WHERE g.provider_id = ?1 AND g.kind = 'live'
                                 AND g.category_id = c.category_id AND NOT g.shown)
         ORDER BY c.name_norm != a.norm, c.category_id IS NOT a.cat, c.num, c.rowid
         LIMIT ?3",
    )?
    .query_map(params![provider_id, key, limit as i64], |r| r.get(0))?
    .collect()
}

/// SPEC 5 favourites fallback, inside an import of `kind`'s list: a
/// favourite of that kind whose `stream_id` is gone moves to the one title
/// of the kind with the same `name_norm`. With no match or several it
/// stays, unavailable.
/// The favourites fallback for guide pins and manual mappings (EPG sources
/// ticket 05): one whose channel is gone is moved to the one channel with
/// its `name_norm`, if exactly one; one the channel has already is
/// dropped. Otherwise it is kept, in case the channel comes back.
fn repoint_guide_choices(tx: &Connection, provider_id: &str) -> rusqlite::Result<()> {
    for table in ["source_pin", "source_mapping"] {
        let stale: Vec<(i64, String)> = tx
            .prepare(&format!(
                "SELECT DISTINCT stream_id, name_norm FROM {table} n WHERE provider_id = ?1
                 AND NOT EXISTS (SELECT 1 FROM channel c
                                 WHERE c.provider_id = n.provider_id AND c.stream_id = n.stream_id)"
            ))?
            .query_map([provider_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (old, norm) in stale {
            let matches: Vec<i64> = tx
                .prepare_cached(
                    "SELECT stream_id FROM channel WHERE provider_id = ?1 AND name_norm = ?2 LIMIT 2",
                )?
                .query_map(params![provider_id, norm], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let [new] = matches[..] else {
                continue;
            };
            tx.execute(
                &format!(
                    "UPDATE OR IGNORE {table} SET stream_id = ?3 WHERE provider_id = ?1 AND stream_id = ?2"
                ),
                params![provider_id, old, new],
            )?;
            tx.execute(
                &format!("DELETE FROM {table} WHERE provider_id = ?1 AND stream_id = ?2"),
                params![provider_id, old],
            )?;
        }
    }
    Ok(())
}

fn repoint_favourites(tx: &Connection, provider_id: &str, kind: Kind) -> rusqlite::Result<()> {
    let (table, id) = (kind.table(), kind.id_column());
    let stale: Vec<(i64, String)> = tx
        .prepare(&format!(
            "SELECT stream_id, name_norm FROM favourite f WHERE provider_id = ?1 AND kind = ?2
             AND NOT EXISTS (SELECT 1 FROM {table} c
                             WHERE c.provider_id = f.provider_id AND c.{id} = f.stream_id)"
        ))?
        .query_map(params![provider_id, kind], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let (mut moved, mut unavailable) = (0, 0);
    for (old, norm) in stale {
        let matches: Vec<i64> = tx
            .prepare_cached(&format!(
                "SELECT {id} FROM {table} WHERE provider_id = ?1 AND name_norm = ?2 LIMIT 2"
            ))?
            .query_map(params![provider_id, norm], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let [new] = matches[..] else {
            unavailable += 1;
            continue;
        };
        // Already a favourite under its new id: the old entry is a duplicate.
        let duplicate: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM favourite
                            WHERE provider_id = ?1 AND kind = ?2 AND stream_id = ?3)",
            params![provider_id, kind, new],
            |r| r.get(0),
        )?;
        if duplicate {
            tx.execute(
                "DELETE FROM favourite WHERE provider_id = ?1 AND kind = ?2 AND stream_id = ?3",
                params![provider_id, kind, old],
            )?;
        } else {
            tx.execute(
                "UPDATE favourite SET stream_id = ?4
                 WHERE provider_id = ?1 AND kind = ?2 AND stream_id = ?3",
                params![provider_id, kind, old, new],
            )?;
        }
        moved += 1;
    }
    // Keep display names current for when a title later disappears.
    tx.execute(
        &format!(
            "UPDATE favourite SET name = c.name FROM {table} c
             WHERE favourite.provider_id = ?1 AND favourite.kind = ?2
               AND c.provider_id = favourite.provider_id AND c.{id} = favourite.stream_id"
        ),
        params![provider_id, kind],
    )?;
    if moved + unavailable > 0 {
        log::info!(
            "{} favourites: {moved} re-pointed by name, {unavailable} unavailable",
            kind.as_str()
        );
    }
    Ok(())
}

/// Inside an import of `kind`'s list: what of that kind it no longer has
/// drops out of recently played (SPEC 5).
fn drop_gone_history(tx: &Connection, provider_id: &str, kind: Kind) -> rusqlite::Result<()> {
    tx.execute(
        &format!(
            "DELETE FROM history WHERE provider_id = ?1 AND kind = ?2
             AND NOT EXISTS (SELECT 1 FROM {table} c
                             WHERE c.provider_id = history.provider_id
                               AND c.{id} = history.stream_id)",
            table = kind.table(),
            id = kind.id_column(),
        ),
        params![provider_id, kind],
    )?;
    Ok(())
}

/// Calls `each` with every channel's, movie's or series' `rowid` and name,
/// for the search index.
pub fn names(
    conn: &Connection,
    provider_id: &str,
    kind: Kind,
    mut each: impl FnMut(i64, &str),
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!(
        "SELECT rowid, name FROM {} WHERE provider_id = ?1",
        kind.table()
    ))?;
    let mut rows = stmt.query([provider_id])?;
    while let Some(r) = rows.next()? {
        each(r.get(0)?, r.get_ref(1)?.as_str()?);
    }
    Ok(())
}

/// The provider last switched to, if any.
pub fn last_provider(conn: &Connection) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM app_state WHERE key = ?1",
        [LAST_PROVIDER],
        |r| r.get(0),
    )
    .optional()
}

pub fn set_last_provider(conn: &Connection, provider_id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_state (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![LAST_PROVIDER, provider_id],
    )?;
    Ok(())
}

/// What removing a provider deletes and what it keeps unless asked
/// (onboarding ticket 02), counted for its confirm.
#[derive(Debug, Default, PartialEq)]
pub struct ProviderData {
    pub channels: i64,
    pub movies: i64,
    pub series: i64,
    /// Scheduled recordings, which are cancelled.
    pub scheduled: i64,
    /// Queued or running downloads, which are forgotten.
    pub queued: i64,
    pub favourites: i64,
    /// Movies and episodes with a resume point or watched.
    pub progress: i64,
    /// Recordings and downloads past scheduled or queued.
    pub recorded: i64,
}

pub fn provider_data(conn: &Connection, provider_id: &str) -> rusqlite::Result<ProviderData> {
    let count = |sql: &str| conn.query_row(sql, [provider_id], |r| r.get(0));
    Ok(ProviderData {
        channels: count("SELECT count(*) FROM channel WHERE provider_id = ?1")?,
        movies: count("SELECT count(*) FROM movie WHERE provider_id = ?1")?,
        series: count("SELECT count(*) FROM series WHERE provider_id = ?1")?,
        scheduled: count(
            "SELECT count(*) FROM recording WHERE provider_id = ?1 AND state = 'scheduled'",
        )?,
        queued: count(
            "SELECT count(*) FROM download WHERE provider_id = ?1
             AND state IN ('queued', 'downloading')",
        )?,
        favourites: count("SELECT count(*) FROM favourite WHERE provider_id = ?1")?,
        progress: count("SELECT count(*) FROM progress WHERE provider_id = ?1")?,
        recorded: count(
            "SELECT (SELECT count(*) FROM recording WHERE provider_id = ?1
                     AND state NOT IN ('scheduled', 'cancelled'))
                  + (SELECT count(*) FROM download WHERE provider_id = ?1
                     AND state NOT IN ('queued', 'downloading'))",
        )?,
    })
}

/// Removes a provider's data (onboarding ticket 02) in one transaction:
/// its lists, guide, guide pins and refresh times; its scheduled
/// recordings are cancelled and its queued downloads forgotten. Its
/// favourites, progress, recently played, recordings, downloads, library
/// rules and manual guide mappings stay, so the
/// same id added again gets them back, unless `personal` deletes them too.
/// Files on disk are never touched.
pub fn remove_provider(
    conn: &Connection,
    provider_id: &str,
    personal: bool,
) -> rusqlite::Result<()> {
    // On the recorder's `&Connection`; nothing else holds a transaction on it.
    let tx = conn.unchecked_transaction()?;
    for table in [
        "category",
        "channel",
        "movie",
        "series",
        "episode",
        "programme",
    ] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE provider_id = ?1"),
            [provider_id],
        )?;
    }
    for table in ["source_channel", "source_match"] {
        tx.execute(
            &format!(
                "DELETE FROM {table} WHERE source_id IN
                     (SELECT id FROM epg_source WHERE provider_id = ?1)"
            ),
            [provider_id],
        )?;
    }
    tx.execute(
        "DELETE FROM epg_source WHERE provider_id = ?1",
        [provider_id],
    )?;
    tx.execute(
        "DELETE FROM source_pin WHERE provider_id = ?1",
        [provider_id],
    )?;
    tx.execute(
        "INSERT INTO channel_fts(channel_fts) VALUES ('rebuild')",
        [],
    )?;
    tx.execute("DELETE FROM provider WHERE id = ?1", [provider_id])?;
    tx.execute(
        "UPDATE recording SET state = 'cancelled'
         WHERE provider_id = ?1 AND state = 'scheduled'",
        [provider_id],
    )?;
    tx.execute(
        "DELETE FROM download WHERE provider_id = ?1 AND state IN ('queued', 'downloading')",
        [provider_id],
    )?;
    tx.execute(
        "DELETE FROM app_state WHERE key = ?1 AND value = ?2",
        params![LAST_PROVIDER, provider_id],
    )?;
    if personal {
        for table in [
            "favourite",
            "progress",
            "history",
            "recording",
            "download",
            "category_rule",
            "category_override",
            "channel_rule",
            "kept_tag",
            "source_mapping",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE provider_id = ?1"),
                [provider_id],
            )?;
        }
        tx.execute(
            "DELETE FROM app_state WHERE key = ?1",
            [format!("{CONTINUE_CLEARED}{provider_id}")],
        )?;
    }
    tx.commit()
}

/// The list order last chosen, for every provider.
pub fn order(conn: &Connection) -> rusqlite::Result<Order> {
    let value: Option<String> = conn
        .query_row("SELECT value FROM app_state WHERE key = ?1", [ORDER], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(match value.as_deref() {
        Some("az") => Order::Az,
        _ => Order::Provider,
    })
}

pub fn set_order(conn: &Connection, order: Order) -> rusqlite::Result<()> {
    let value = match order {
        Order::Provider => "provider",
        Order::Az => "az",
    };
    conn.execute(
        "INSERT INTO app_state (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![ORDER, value],
    )?;
    Ok(())
}

/// When the provider's channels and EPG were last imported, if ever.
pub fn last_refreshes(
    conn: &Connection,
    provider_id: &str,
) -> rusqlite::Result<(Option<i64>, Option<i64>)> {
    Ok(conn
        .prepare_cached(
            "SELECT last_channels_refresh, last_epg_refresh FROM provider WHERE id = ?1",
        )?
        .query_row([provider_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .unwrap_or_default())
}

/// When the provider's movies and series were last imported, if ever.
pub fn last_vod_refreshes(
    conn: &Connection,
    provider_id: &str,
) -> rusqlite::Result<(Option<i64>, Option<i64>)> {
    Ok(conn
        .prepare_cached(
            "SELECT last_movies_refresh, last_series_refresh FROM provider WHERE id = ?1",
        )?
        .query_row([provider_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .unwrap_or_default())
}

/// First half of a channel import, inside the caller's transaction: records
/// the provider, replaces its categories, and clears its channels for
/// [`insert_channel`].
pub fn begin_channel_import<'a>(
    tx: &Connection,
    provider_id: &str,
    provider_name: &str,
    categories: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> rusqlite::Result<()> {
    begin_import(tx, provider_id, provider_name, Kind::Live, categories)
}

/// First half of any import, inside the caller's transaction: records the
/// provider, replaces its categories of `kind` with `categories`, and
/// clears its titles of that kind for `insert_*`.
pub fn begin_import<'a>(
    tx: &Connection,
    provider_id: &str,
    provider_name: &str,
    kind: Kind,
    categories: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO provider (id, name) VALUES (?1, ?2) ON CONFLICT (id) DO UPDATE SET name = excluded.name",
        params![provider_id, provider_name],
    )?;
    tx.execute(
        "DELETE FROM category WHERE provider_id = ?1 AND kind = ?2",
        params![provider_id, kind],
    )?;
    tx.execute(
        &format!("DELETE FROM {} WHERE provider_id = ?1", kind.table()),
        [provider_id],
    )?;
    if kind == Kind::Series {
        tx.execute("DELETE FROM episode WHERE provider_id = ?1", [provider_id])?;
    }
    let mut ins = tx.prepare(
        "INSERT OR REPLACE INTO category (provider_id, kind, category_id, name, sort)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (sort, (id, name)) in categories.into_iter().enumerate() {
        ins.execute(params![provider_id, kind, id, name, sort as i64])?;
    }
    Ok(())
}

/// A movie from `get_vod_streams` (SPEC 6.1).
pub struct NewMovie<'a> {
    pub stream_id: i64,
    pub num: Option<i64>,
    pub name: &'a str,
    pub category_id: Option<&'a str>,
    pub rating: Option<f64>,
    pub poster_url: Option<&'a str>,
    pub extension: Option<&'a str>,
    pub added: Option<i64>,
}

/// A series from `get_series` (SPEC 6.1).
pub struct NewSeries<'a> {
    pub series_id: i64,
    pub num: Option<i64>,
    pub name: &'a str,
    pub category_id: Option<&'a str>,
    pub year: Option<i64>,
    pub rating: Option<f64>,
    pub cover_url: Option<&'a str>,
    pub genre: Option<&'a str>,
    pub plot: Option<&'a str>,
    pub actors: Option<&'a str>,
    pub director: Option<&'a str>,
    pub episode_run_time: Option<i64>,
    pub last_modified: Option<i64>,
}

/// Duplicate `stream_id`s keep the last one.
pub fn insert_movie(tx: &Connection, provider_id: &str, m: &NewMovie) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT OR REPLACE INTO movie (provider_id, stream_id, num, name, name_norm,
             category_id, rating, poster_url, extension, added)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?
    .execute(params![
        provider_id,
        m.stream_id,
        m.num,
        m.name,
        name_norm(m.name),
        m.category_id,
        m.rating,
        m.poster_url,
        m.extension,
        m.added,
    ])?;
    Ok(())
}

/// Duplicate `series_id`s keep the last one.
pub fn insert_series(tx: &Connection, provider_id: &str, s: &NewSeries) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT OR REPLACE INTO series (provider_id, series_id, num, name, name_norm,
             category_id, year, rating, cover_url, genre, plot, actors, director,
             episode_run_time, last_modified)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
    )?
    .execute(params![
        provider_id,
        s.series_id,
        s.num,
        s.name,
        name_norm(s.name),
        s.category_id,
        s.year,
        s.rating,
        s.cover_url,
        s.genre,
        s.plot,
        s.actors,
        s.director,
        s.episode_run_time,
        s.last_modified,
    ])?;
    Ok(())
}

/// Second half: runs the kind's favourites fallback, drops titles gone
/// from recently played, re-points and renames the kind's overrides,
/// applies the category rules and stamps the refresh time.
pub fn finish_vod_import(
    tx: &Connection,
    provider_id: &str,
    kind: Kind,
    now: i64,
) -> rusqlite::Result<()> {
    repoint_favourites(tx, provider_id, kind)?;
    drop_gone_history(tx, provider_id, kind)?;
    repoint_category_overrides(tx, provider_id, kind)?;
    apply_category_rules(tx, provider_id)?;
    let column = match kind {
        Kind::Movie => "last_movies_refresh",
        Kind::Series => "last_series_refresh",
        Kind::Live => unreachable!("channels finish with finish_channel_import"),
    };
    tx.execute(
        &format!("UPDATE provider SET {column} = ?2 WHERE id = ?1"),
        params![provider_id, now],
    )?;
    Ok(())
}

/// Duplicate `stream_id`s keep the last one.
pub fn insert_channel(tx: &Connection, provider_id: &str, c: &NewChannel) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT OR REPLACE INTO channel (provider_id, stream_id, num, name, name_norm,
             category_id, epg_channel_id, icon_url, extension, archive_days, name_loose)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?
    .execute(params![
        provider_id,
        c.stream_id,
        c.num,
        c.name,
        name_norm(c.name),
        c.category_id,
        c.epg_channel_id,
        c.icon_url,
        c.extension,
        c.archive_days,
        loose_name(c.name),
    ])?;
    Ok(())
}

/// Second half: runs the live favourites fallback, for guide pins and
/// mappings too, applies the category rules
/// and overrides to the new categories and the channel rules to the new
/// channels, drops channels gone from recently
/// played, rebuilds `channel_fts` (all providers, about 100 ms per 60k
/// channels) and stamps the refresh time.
pub fn finish_channel_import(tx: &Connection, provider_id: &str, now: i64) -> rusqlite::Result<()> {
    repoint_favourites(tx, provider_id, Kind::Live)?;
    repoint_guide_choices(tx, provider_id)?;
    repoint_category_overrides(tx, provider_id, Kind::Live)?;
    apply_category_rules(tx, provider_id)?;
    apply_channel_rules(tx, provider_id)?;
    drop_gone_history(tx, provider_id, Kind::Live)?;
    resolve_guides(tx, provider_id)?;
    tx.execute(
        "INSERT INTO channel_fts(channel_fts) VALUES ('rebuild')",
        [],
    )?;
    tx.execute(
        "UPDATE provider SET last_channels_refresh = ?2 WHERE id = ?1",
        params![provider_id, now],
    )?;
    Ok(())
}

/// Starts an EPG import inside the caller's transaction: clears the
/// provider's own programmes, not its sources', for [`insert_programme`]. Those that have ended on
/// a guide id with an archive are set aside first, for
/// [`finish_epg_import`] to keep what the new guide does not reach
/// (catch-up ticket 06).
pub fn begin_epg_import(tx: &Connection, provider_id: &str, now: i64) -> rusqlite::Result<()> {
    tx.execute_batch(
        "DROP TABLE IF EXISTS temp.past_programme;
         CREATE TEMP TABLE past_programme AS SELECT * FROM programme WHERE 0;",
    )?;
    tx.execute(
        "INSERT INTO temp.past_programme SELECT * FROM programme
         WHERE provider_id = ?1 AND stop <= ?2 AND epg_channel_id IN
             (SELECT epg_channel_id FROM channel WHERE provider_id = ?1 AND archive_days > 0)",
        params![provider_id, now],
    )?;
    delete_provider_guide(tx, provider_id)
}

/// Deletes the provider's own programmes, keeping its sources', and the
/// shifted copies of them for pinned channels.
fn delete_provider_guide(tx: &Connection, provider_id: &str) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM programme WHERE provider_id = ?1
             AND (epg_channel_id < char(1) OR epg_channel_id >= char(2))",
        [provider_id],
    )?;
    if let Some(own) = own_source(tx, provider_id)? {
        delete_pin_copies(tx, provider_id, own)?;
    }
    Ok(())
}

/// Guide id to the longest archive, in days, of the provider's channels
/// that have one and a guide id. About 1k entries on a large provider, not
/// the channel list (ADR 0003).
pub fn archive_days_by_guide(
    conn: &Connection,
    provider_id: &str,
) -> rusqlite::Result<HashMap<String, i64>> {
    conn.prepare(
        "SELECT epg_channel_id, max(archive_days) FROM channel
         WHERE provider_id = ?1 AND archive_days > 0 AND epg_channel_id IS NOT NULL
         GROUP BY epg_channel_id",
    )?
    .query_map([provider_id], |r| Ok((r.get(0)?, r.get(1)?)))?
    .collect()
}

/// Archive days of the provider's channels with an archive, by row key,
/// for how far back the guide grid reaches (catch-up ticket 03).
pub fn archive_days_by_key(
    conn: &Connection,
    provider_id: &str,
) -> rusqlite::Result<HashMap<i64, i64>> {
    conn.prepare_cached(
        "SELECT rowid, archive_days FROM channel WHERE provider_id = ?1 AND archive_days > 0",
    )?
    .query_map([provider_id], |r| Ok((r.get(0)?, r.get(1)?)))?
    .collect()
}

pub struct NewProgramme<'a> {
    pub epg_channel_id: &'a str,
    pub start: i64,
    pub stop: i64,
    pub title: &'a str,
    pub description: Option<&'a str>,
}

/// Duplicate `(channel, start)` pairs keep the last one.
pub fn insert_programme(
    tx: &Connection,
    provider_id: &str,
    p: &NewProgramme,
) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT OR REPLACE INTO programme (provider_id, epg_channel_id, start, stop, title, description)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?
        .execute(params![
            provider_id,
            p.epg_channel_id,
            p.start,
            p.stop,
            p.title,
            p.description,
        ])?;
    Ok(())
}

/// A programme from a channel's `get_simple_data_table` (catch-up ticket
/// 06).
pub struct TableProgramme {
    pub start: i64,
    pub stop: i64,
    pub title: String,
    pub description: Option<String>,
    /// `has_archive`, if the table said.
    pub archive: Option<bool>,
}

/// Adds a channel's table to its guide id's programmes: those that end
/// before the guide's first and within the channel's `archive_days`, as
/// an EPG import would keep them. Where the guide has a programme at the
/// same start, only its archive flag is taken. Returns how many were
/// added.
pub fn add_table(
    conn: &mut Connection,
    provider_id: &str,
    guide_id: &str,
    archive_days: i64,
    progs: &[TableProgramme],
    now: i64,
) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;
    let first: Option<i64> = tx.query_row(
        "SELECT min(start) FROM programme WHERE provider_id = ?1 AND epg_channel_id = ?2",
        params![provider_id, guide_id],
        |r| r.get(0),
    )?;
    let before = first.unwrap_or(now).min(now);
    let mut added = 0;
    for p in progs {
        if p.stop <= before && p.stop > now - archive_days * 86_400 && p.start < p.stop {
            added += tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO programme VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )?
                .execute(params![
                    provider_id,
                    guide_id,
                    p.start,
                    p.stop,
                    p.title,
                    p.description,
                    p.archive,
                ])?;
        } else if p.archive.is_some() {
            tx.prepare_cached(
                "UPDATE programme SET archive = ?4
                 WHERE provider_id = ?1 AND epg_channel_id = ?2 AND start = ?3",
            )?
            .execute(params![provider_id, guide_id, p.start, p.archive])?;
        }
    }
    // Its channels had no guide before if the provider's had nothing for
    // them; now it is the first with programmes.
    tx.execute(
        "UPDATE channel SET guide_id = ?2
         WHERE provider_id = ?1 AND epg_channel_id = ?2 AND guide_id IS NULL",
        params![provider_id, guide_id],
    )?;
    tx.commit()?;
    Ok(added)
}

/// Stamps the refresh time, after moving schedules with their programmes.
/// The past programmes [`begin_epg_import`] set aside come back where they
/// end before the new guide's first on their guide id, and still within
/// its longest archive; the rest go.
pub fn finish_epg_import(tx: &Connection, provider_id: &str, now: i64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO programme SELECT k.* FROM temp.past_programme k
         JOIN (SELECT epg_channel_id, max(archive_days) AS days FROM channel
               WHERE provider_id = ?1 AND archive_days > 0 GROUP BY epg_channel_id) a
             ON a.epg_channel_id = k.epg_channel_id
         WHERE k.stop > ?2 - 86400 * a.days
           AND k.stop <= coalesce((SELECT min(start) FROM programme
                                   WHERE provider_id = ?1 AND epg_channel_id = k.epg_channel_id), ?2)",
        params![provider_id, now],
    )?;
    tx.execute("DROP TABLE temp.past_programme", [])?;
    if let Some(own) = own_source(tx, provider_id)? {
        copy_pins(tx, provider_id, own)?;
    }
    follow_schedules(tx, provider_id)?;
    resolve_guides(tx, provider_id)?;
    stamp_epg(tx, provider_id, now)
}

/// Stamps the EPG refresh time, as an import does. The EPG stage with the
/// provider's guide off, so it counts as done (EPG sources ticket 02).
pub fn stamp_epg(conn: &Connection, provider_id: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE provider SET last_epg_refresh = ?2 WHERE id = ?1",
        params![provider_id, now],
    )?;
    Ok(())
}

/// The guide id of channel `channel` of source `source` (EPG sources
/// ticket 02). Its programmes are the provider's, under this id. It starts
/// with `\u{1}`, which XML cannot hold, so it never meets a provider's own
/// guide id, and a source's ids are one range of the programme key.
pub fn source_guide_id(source: i64, channel: &str) -> String {
    format!("\u{1}{source}\u{1}{channel}")
}

/// Whether `guide_id` is an external source's rather than the provider's.
pub fn is_source_guide(guide_id: &str) -> bool {
    guide_id.starts_with('\u{1}')
}

/// The ident of the source whose guide id `guide_id` is, `None` for the
/// provider's own guide, a pinned channel's shifted copy of it included.
pub fn source_ident(conn: &Connection, guide_id: &str) -> rusqlite::Result<Option<String>> {
    let Some(id) = guide_id
        .strip_prefix('\u{1}')
        .and_then(|g| g.split('\u{1}').next())
        .and_then(|id| id.parse::<i64>().ok())
    else {
        return Ok(None);
    };
    Ok(conn
        .prepare_cached("SELECT ident FROM epg_source WHERE id = ?1")?
        .query_row([id], |r| r.get::<_, String>(0))
        .optional()?
        .filter(|ident| !ident.is_empty()))
}

/// Source `source`'s guide ids are `from..to`.
fn source_range(source: i64) -> (String, String) {
    (format!("\u{1}{source}\u{1}"), format!("\u{1}{source}\u{2}"))
}

/// One `[[provider.epg]]` entry as stored: `ident` empty for the
/// provider's own guide.
pub struct SourceEntry<'a> {
    pub ident: &'a str,
    pub enabled: bool,
    pub shift_minutes: i64,
}

/// Stores a provider's guide sources in `entries`' order, adding new ones
/// and deleting, with their channels, matches, programmes and pins, those
/// gone from it; manual mappings stay, by ident. With the provider's guide off its programmes go. A new shift
/// drops a source's validators, so its next fetch imports whatever the
/// feed says. Channels' guides are resolved again if anything changed.
/// Returns each entry's source id.
pub fn sync_sources(
    conn: &mut Connection,
    provider_id: &str,
    entries: &[SourceEntry],
) -> rusqlite::Result<Vec<i64>> {
    let tx = conn.transaction()?;
    let before: Vec<(i64, String, i64, bool)> = tx
        .prepare("SELECT id, ident, position, enabled FROM epg_source WHERE provider_id = ?1")?
        .query_map([provider_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<Result<_, _>>()?;
    let mut changed = false;
    for (id, ident, ..) in &before {
        if !entries.iter().any(|e| e.ident == ident) {
            delete_source_data(&tx, provider_id, *id)?;
            tx.execute("DELETE FROM source_request WHERE source_id = ?1", [id])?;
            tx.execute("DELETE FROM source_pin WHERE source_id = ?1", [id])?;
            tx.execute("DELETE FROM epg_source WHERE id = ?1", [id])?;
            changed = true;
        }
    }
    let mut ids = Vec::with_capacity(entries.len());
    for (position, e) in (0..).zip(entries) {
        let id: i64 = tx
            .prepare_cached(
                "INSERT INTO epg_source (provider_id, ident, position, enabled, shift_minutes)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (provider_id, ident) DO UPDATE SET
                     position = ?3, enabled = ?4, shift_minutes = ?5,
                     etag = iif(shift_minutes = ?5, etag, NULL),
                     last_modified = iif(shift_minutes = ?5, last_modified, NULL),
                     mtime = iif(shift_minutes = ?5, mtime, NULL)
                 RETURNING id",
            )?
            .query_row(
                params![provider_id, e.ident, position, e.enabled, e.shift_minutes],
                |r| r.get(0),
            )?;
        changed |= !before.contains(&(id, e.ident.to_owned(), position, e.enabled));
        if e.ident.is_empty() && !e.enabled {
            delete_provider_guide(&tx, provider_id)?;
        }
        ids.push(id);
    }
    if changed {
        resolve_guides(&tx, provider_id)?;
    }
    tx.commit()?;
    Ok(ids)
}

/// Whether the provider's own guide is on, and its shift in minutes. On,
/// unshifted, until its sources are first stored.
pub fn provider_guide(conn: &Connection, provider_id: &str) -> rusqlite::Result<(bool, i64)> {
    Ok(conn
        .prepare_cached(
            "SELECT enabled, shift_minutes FROM epg_source WHERE provider_id = ?1 AND ident = ''",
        )?
        .query_row([provider_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .unwrap_or((true, 0)))
}

/// Deletes source `source`'s programmes, channels and matches.
fn delete_source_data(tx: &Connection, provider_id: &str, source: i64) -> rusqlite::Result<()> {
    let (from, to) = source_range(source);
    tx.execute(
        "DELETE FROM programme WHERE provider_id = ?1 AND epg_channel_id >= ?2 AND epg_channel_id < ?3",
        params![provider_id, from, to],
    )?;
    tx.execute("DELETE FROM source_channel WHERE source_id = ?1", [source])?;
    tx.execute("DELETE FROM source_match WHERE source_id = ?1", [source])?;
    Ok(())
}

/// Sets each of the provider's channels' `guide_id` to its pin's when the
/// pinned source is enabled and has programmes for it, else to the first
/// enabled source in its order with programmes for it, or NULL. Writes
/// only the channels whose guide changed.
pub fn resolve_guides(tx: &Connection, provider_id: &str) -> rusqlite::Result<()> {
    let mut order: Vec<(i64, String)> = tx
        .prepare_cached(
            "SELECT id, ident FROM epg_source WHERE provider_id = ?1 AND enabled ORDER BY position",
        )?
        .query_map([provider_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let stored: bool = tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM epg_source WHERE provider_id = ?1 AND ident = '')",
        )?
        .query_row([provider_id], |r| r.get(0))?;
    if !stored {
        order.insert(0, (0, String::new()));
    }
    // Ids are integers from the database, so written into the SQL.
    let candidates: Vec<String> = order
        .iter()
        .map(|(id, ident)| {
            if ident.is_empty() {
                "(SELECT c.epg_channel_id WHERE EXISTS (SELECT 1 FROM programme p
                     WHERE p.provider_id = ?1 AND p.epg_channel_id = c.epg_channel_id))"
                    .to_owned()
            } else {
                format!(
                    "(SELECT m.guide_id FROM source_match m WHERE m.source_id = {id}
                         AND m.stream_id = c.stream_id AND EXISTS (SELECT 1 FROM programme p
                             WHERE p.provider_id = ?1 AND p.epg_channel_id = m.guide_id))"
                )
            }
        })
        .collect();
    let pin = "(SELECT n.guide FROM pinned n WHERE n.stream_id = c.stream_id AND n.enabled
                   AND EXISTS (SELECT 1 FROM programme p
                               WHERE p.provider_id = ?1 AND p.epg_channel_id = n.guide))";
    let guide = format!(
        "coalesce({})",
        std::iter::once(pin)
            .chain(candidates.iter().map(String::as_str))
            // coalesce() takes two or more.
            .chain(std::iter::once("NULL"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    tx.execute(
        &format!(
            "WITH {PINNED}, r AS (SELECT c.rowid AS key, {guide} AS guide
                                  FROM channel c WHERE c.provider_id = ?1)
             UPDATE channel SET guide_id = r.guide FROM r
             WHERE channel.rowid = r.key AND channel.guide_id IS NOT r.guide"
        ),
        [provider_id],
    )?;
    Ok(())
}

/// The provider's (`?1`) pins, as a `WITH` table `pinned (stream_id,
/// source_id, shift, enabled, base, guide)`: `base` the guide id the
/// pinned source has the channel's programmes under, `guide` the one the
/// channel reads, `base` itself unshifted, else a copy of `base`'s
/// programmes moved by the shift ([`copy_pins`]): `\u{1}<source>\u{1}\u{1}
/// <minutes>\u{1}` and the channel's id in the source, which a feed's
/// channel id cannot start with, so the copies are one range in the
/// source's.
const PINNED: &str = "pin AS MATERIALIZED (
         SELECT n.stream_id, n.source_id, n.shift_minutes AS shift, s.enabled, s.ident = '' AS own,
             CASE WHEN s.ident = '' THEN c.epg_channel_id ELSE m.guide_id END AS base
         FROM source_pin n
         JOIN epg_source s ON s.id = n.source_id
         JOIN channel c ON c.provider_id = n.provider_id AND c.stream_id = n.stream_id
         LEFT JOIN source_match m ON m.source_id = n.source_id AND m.stream_id = n.stream_id
         WHERE n.provider_id = ?1),
     pinned AS (
         SELECT stream_id, source_id, shift, enabled, base,
             CASE WHEN shift = 0 THEN base
                  ELSE char(1) || source_id || char(1) || char(1) || shift || char(1)
                      || CASE WHEN own THEN base ELSE substr(base, length(source_id) + 3) END
             END AS guide
         FROM pin WHERE base IS NOT NULL AND base != '')";

/// Deletes the shifted copies of source `source`'s programmes made for
/// pinned channels.
fn delete_pin_copies(tx: &Connection, provider_id: &str, source: i64) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM programme WHERE provider_id = ?1
             AND epg_channel_id >= char(1) || ?2 || char(1) || char(1)
             AND epg_channel_id < char(1) || ?2 || char(1) || char(2)",
        params![provider_id, source],
    )?;
    Ok(())
}

/// Makes again the copies of source `source`'s programmes that channels
/// pinned to it with a shift read, moved by that shift ([`PINNED`]).
fn copy_pins(tx: &Connection, provider_id: &str, source: i64) -> rusqlite::Result<()> {
    delete_pin_copies(tx, provider_id, source)?;
    tx.execute(
        &format!(
            "WITH {PINNED}
             INSERT OR IGNORE INTO programme
                 (provider_id, epg_channel_id, start, stop, title, description, archive)
             SELECT p.provider_id, g.guide, p.start + 60 * g.shift, p.stop + 60 * g.shift,
                 p.title, p.description, p.archive
             FROM (SELECT DISTINCT base, guide, shift FROM pinned
                   WHERE source_id = ?2 AND shift != 0) g
             JOIN programme p ON p.provider_id = ?1 AND p.epg_channel_id = g.base"
        ),
        params![provider_id, source],
    )?;
    Ok(())
}

/// The id of the provider's own guide in its order, once stored.
fn own_source(tx: &Connection, provider_id: &str) -> rusqlite::Result<Option<i64>> {
    tx.prepare_cached("SELECT id FROM epg_source WHERE provider_id = ?1 AND ident = ''")?
        .query_row([provider_id], |r| r.get(0))
        .optional()
}

/// Starts an import of source `source` inside the caller's transaction:
/// clears its channels, matches and programmes, setting aside those that
/// have ended for [`finish_source_import`], as [`begin_epg_import`] does.
pub fn begin_source_import(
    tx: &Connection,
    provider_id: &str,
    source: i64,
    now: i64,
) -> rusqlite::Result<()> {
    let (from, to) = source_range(source);
    tx.execute_batch(
        "DROP TABLE IF EXISTS temp.past_programme;
         CREATE TEMP TABLE past_programme AS SELECT * FROM programme WHERE 0;",
    )?;
    tx.execute(
        "INSERT INTO temp.past_programme SELECT * FROM programme
         WHERE provider_id = ?1 AND epg_channel_id >= ?2 AND epg_channel_id < ?3 AND stop <= ?4",
        params![provider_id, from, to, now],
    )?;
    delete_source_data(tx, provider_id, source)
}

/// One of a source's channels, kept whole for the mapping picker.
pub fn insert_source_channel(
    tx: &Connection,
    source: i64,
    channel_id: &str,
    names: &[String],
    icon_url: Option<&str>,
) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT INTO source_channel (source_id, channel_id, names, icon_url) VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![source, channel_id, names.join("\n"), icon_url])?;
    Ok(())
}

/// The provider's channels, `(stream_id, epg_channel_id, name,
/// archive_days)`, a row at a time, for matching against a source.
pub fn each_channel(
    tx: &Connection,
    provider_id: &str,
    mut f: impl FnMut(i64, Option<&str>, &str, i64) -> rusqlite::Result<()>,
) -> rusqlite::Result<()> {
    let mut stmt = tx.prepare(
        "SELECT stream_id, NULLIF(epg_channel_id, ''), name, archive_days FROM channel
         WHERE provider_id = ?1",
    )?;
    let mut rows = stmt.query([provider_id])?;
    while let Some(r) = rows.next()? {
        let guide: Option<String> = r.get(1)?;
        f(
            r.get(0)?,
            guide.as_deref(),
            &r.get::<_, String>(2)?,
            r.get(3)?,
        )?;
    }
    Ok(())
}

/// Channel `stream_id` matched `guide_id` of source `source`, or several
/// of its channels when `None`.
pub fn insert_source_match(
    tx: &Connection,
    source: i64,
    stream_id: i64,
    guide_id: Option<&str>,
) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT OR REPLACE INTO source_match (source_id, stream_id, guide_id) VALUES (?1, ?2, ?3)",
    )?
    .execute(params![source, stream_id, guide_id])?;
    Ok(())
}

/// Ends an import of source `source`: past programmes set aside come back
/// as [`finish_epg_import`] has them, by the longest archive among the
/// channels matched to each; schedules follow their programmes; guides are
/// resolved, pinned channels' shifted copies made again first; the import
/// is stamped, and any error and re-import mark cleared.
pub fn finish_source_import(
    tx: &Connection,
    provider_id: &str,
    source: i64,
    now: i64,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO programme SELECT k.* FROM temp.past_programme k
         JOIN (SELECT m.guide_id, max(c.archive_days) AS days FROM source_match m
               JOIN channel c ON c.provider_id = ?1 AND c.stream_id = m.stream_id
               WHERE m.source_id = ?3 AND c.archive_days > 0 GROUP BY m.guide_id) a
             ON a.guide_id = k.epg_channel_id
         WHERE k.stop > ?2 - 86400 * a.days
           AND k.stop <= coalesce((SELECT min(start) FROM programme
                                   WHERE provider_id = ?1 AND epg_channel_id = k.epg_channel_id), ?2)",
        params![provider_id, now, source],
    )?;
    tx.execute("DROP TABLE temp.past_programme", [])?;
    copy_pins(tx, provider_id, source)?;
    follow_schedules(tx, provider_id)?;
    resolve_guides(tx, provider_id)?;
    tx.execute(
        "UPDATE epg_source SET imported = ?2, error = NULL, reimport = 0 WHERE id = ?1",
        params![source, now],
    )?;
    Ok(())
}

/// What the next fetch of a source goes by (EPG sources ticket 03).
#[derive(Debug, Default, PartialEq)]
pub struct SourceFetch {
    /// Never imported nor failed.
    pub new: bool,
    /// Marked for a whole fetch by a mapping (EPG sources ticket 05).
    pub reimport: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// A path's modification time when last read, in nanoseconds.
    pub mtime: Option<i64>,
    /// When it was last asked.
    pub fetched: Option<i64>,
    pub retry_at: Option<i64>,
    /// Its requests after `now` - 24 h, oldest first.
    pub requests: Vec<i64>,
}

/// Source `source`'s [`SourceFetch`] at `now`.
pub fn source_fetch(conn: &Connection, source: i64, now: i64) -> rusqlite::Result<SourceFetch> {
    let mut f = conn
        .prepare_cached(
            "SELECT imported IS NULL AND error IS NULL, etag, last_modified, mtime, fetched, retry_at,
                 reimport
             FROM epg_source WHERE id = ?1",
        )?
        .query_row([source], |r| {
            Ok(SourceFetch {
                new: r.get(0)?,
                etag: r.get(1)?,
                last_modified: r.get(2)?,
                mtime: r.get(3)?,
                fetched: r.get(4)?,
                retry_at: r.get(5)?,
                reimport: r.get(6)?,
                requests: Vec::new(),
            })
        })?;
    f.requests = conn
        .prepare_cached(
            "SELECT at FROM source_request WHERE source_id = ?1 AND at > ?2 ORDER BY at",
        )?
        .query_map(params![source, now - 86_400], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(f)
}

/// Counts a request to source `source` at `now`, dropping those 24 h old,
/// and ends any backoff.
pub fn source_requested(conn: &Connection, source: i64, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM source_request WHERE source_id = ?1 AND at <= ?2",
        params![source, now - 86_400],
    )?;
    conn.execute(
        "INSERT INTO source_request (source_id, at) VALUES (?1, ?2)",
        params![source, now],
    )?;
    conn.execute(
        "UPDATE epg_source SET fetched = ?2, retry_at = NULL WHERE id = ?1",
        params![source, now],
    )?;
    Ok(())
}

/// Source `source`'s data is the feed's as of now: the feed's validators
/// for the next conditional request, any error cleared.
pub fn source_current(
    conn: &Connection,
    source: i64,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE epg_source SET etag = ?2, last_modified = ?3, error = NULL WHERE id = ?1",
        params![source, etag, last_modified],
    )?;
    Ok(())
}

/// The modification time of source `source`'s file when last read.
pub fn source_mtime(conn: &Connection, source: i64, mtime: Option<i64>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE epg_source SET mtime = ?2 WHERE id = ?1",
        params![source, mtime],
    )?;
    Ok(())
}

/// Source `source` asked, with a `Retry-After`, to wait until `retry_at`.
pub fn source_backoff(conn: &Connection, source: i64, retry_at: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE epg_source SET retry_at = ?2 WHERE id = ?1",
        params![source, retry_at],
    )?;
    Ok(())
}

/// Why source `source`'s last import failed; its data stays as it was.
pub fn source_failed(conn: &Connection, source: i64, error: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE epg_source SET error = ?2 WHERE id = ?1",
        params![source, error],
    )?;
    Ok(())
}

/// A guide source's row in Settings › EPG (EPG sources ticket 04).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceStats {
    /// The provider's channels it matched to exactly one of its channels,
    /// and to several.
    pub matched: i64,
    pub ambiguous: i64,
    /// The provider's channels whose guide comes from it now.
    pub guiding: i64,
    /// Requests in the last 24 h.
    pub requests: i64,
    pub imported: Option<i64>,
    pub error: Option<String>,
    pub retry_at: Option<i64>,
    /// Channels pinned to it, and mapped by hand to its channels, by its
    /// ident (EPG sources ticket 05).
    pub pins: i64,
    pub mappings: i64,
    /// Marked for a whole fetch at the next refresh by a mapping.
    pub reimport: bool,
}

/// The provider's channel count, and each of its stored guide sources'
/// [`SourceStats`] at `now` by ident, `""` for its own guide: there,
/// matched is the channels with programmes under their own guide id.
pub fn source_stats(
    conn: &Connection,
    provider_id: &str,
    now: i64,
) -> rusqlite::Result<(i64, HashMap<String, SourceStats>)> {
    let channels = conn.query_row(
        "SELECT count(*) FROM channel WHERE provider_id = ?1",
        [provider_id],
        |r| r.get(0),
    )?;
    let mut stats: HashMap<String, SourceStats> = conn
        .prepare(
            "SELECT s.ident, s.imported, s.error, s.retry_at,
                 (SELECT count(*) FROM source_match m
                  WHERE m.source_id = s.id AND m.guide_id IS NOT NULL),
                 (SELECT count(*) FROM source_match m
                  WHERE m.source_id = s.id AND m.guide_id IS NULL),
                 (SELECT count(*) FROM source_request q WHERE q.source_id = s.id AND q.at > ?2),
                 (SELECT count(*) FROM channel c WHERE c.provider_id = ?1
                  AND c.guide_id >= char(1) || s.id || char(1)
                  AND c.guide_id < char(1) || s.id || char(2)),
                 (SELECT count(*) FROM source_pin n WHERE n.source_id = s.id),
                 (SELECT count(*) FROM source_mapping n
                  WHERE n.provider_id = ?1 AND n.ident = s.ident),
                 s.reimport
             FROM epg_source s WHERE s.provider_id = ?1 AND s.ident != ''",
        )?
        .query_map(params![provider_id, now - 86_400], |r| {
            Ok((
                r.get(0)?,
                SourceStats {
                    imported: r.get(1)?,
                    error: r.get(2)?,
                    retry_at: r.get(3)?,
                    matched: r.get(4)?,
                    ambiguous: r.get(5)?,
                    requests: r.get(6)?,
                    guiding: r.get(7)?,
                    pins: r.get(8)?,
                    mappings: r.get(9)?,
                    reimport: r.get(10)?,
                },
            ))
        })?
        .collect::<Result<_, _>>()?;
    let own = SourceStats {
        guiding: conn.query_row(
            "SELECT count(*) FROM channel WHERE provider_id = ?1 AND guide_id = epg_channel_id",
            [provider_id],
            |r| r.get(0),
        )?,
        matched: conn.query_row(
            "SELECT count(*) FROM channel c WHERE c.provider_id = ?1
             AND EXISTS (SELECT 1 FROM programme p WHERE p.provider_id = ?1
                         AND p.epg_channel_id = c.epg_channel_id)",
            [provider_id],
            |r| r.get(0),
        )?,
        imported: last_refreshes(conn, provider_id)?.1,
        ..Default::default()
    };
    stats.insert(String::new(), own);
    Ok((channels, stats))
}

/// How many of the provider's live favourites have their guide from its
/// own guide and from no enabled external source: those turning it off
/// leaves with none (EPG sources ticket 04).
pub fn only_provider_guide(conn: &Connection, provider_id: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT count(*) FROM favourite f
         JOIN channel c ON c.provider_id = f.provider_id AND c.stream_id = f.stream_id
         WHERE f.provider_id = ?1 AND f.kind = 'live'
           AND c.guide_id = c.epg_channel_id
           AND NOT EXISTS (SELECT 1 FROM source_match m
                           JOIN epg_source s ON s.id = m.source_id AND s.enabled
                           WHERE s.provider_id = ?1 AND m.stream_id = c.stream_id
                             AND EXISTS (SELECT 1 FROM programme p WHERE p.provider_id = ?1
                                         AND p.epg_channel_id = m.guide_id))",
        [provider_id],
        |r| r.get(0),
    )
}

/// Pins channel `stream_id` to source `source`, its programmes moved by
/// `shift_minutes` more than the source's, or with `None` unpins it (EPG
/// sources ticket 05). The shifted copies are made from what is stored,
/// so the guide changes at once.
pub fn set_pin(
    conn: &mut Connection,
    provider_id: &str,
    stream_id: i64,
    pin: Option<(i64, i64)>,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let old: Option<i64> = tx
        .query_row(
            "SELECT source_id FROM source_pin WHERE provider_id = ?1 AND stream_id = ?2",
            params![provider_id, stream_id],
            |r| r.get(0),
        )
        .optional()?;
    match pin {
        Some((source, shift)) => tx.execute(
            "INSERT INTO source_pin (provider_id, stream_id, source_id, shift_minutes, name_norm)
             SELECT ?1, ?2, ?3, ?4, name_norm FROM channel WHERE provider_id = ?1 AND stream_id = ?2
             ON CONFLICT (provider_id, stream_id) DO UPDATE SET
                 source_id = ?3, shift_minutes = ?4, name_norm = excluded.name_norm",
            params![provider_id, stream_id, source, shift],
        )?,
        None => tx.execute(
            "DELETE FROM source_pin WHERE provider_id = ?1 AND stream_id = ?2",
            params![provider_id, stream_id],
        )?,
    };
    let new = pin.map(|(source, _)| source);
    for source in old.into_iter().chain(new.filter(|&n| Some(n) != old)) {
        copy_pins(&tx, provider_id, source)?;
    }
    resolve_guides(&tx, provider_id)?;
    tx.commit()
}

/// Maps channel `stream_id` to channel `channel_id` of source `source`, or
/// with `None` forgets its mapping (EPG sources ticket 05). The match is
/// stored at once, so the guide changes now if the source has programmes
/// for that channel already; either way the source is marked for a whole
/// fetch at the next refresh, which keeps the programmes of what it
/// matches now.
pub fn set_mapping(
    conn: &mut Connection,
    provider_id: &str,
    source: i64,
    stream_id: i64,
    channel_id: Option<&str>,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let ident: String = tx.query_row(
        "SELECT ident FROM epg_source WHERE id = ?1",
        [source],
        |r| r.get(0),
    )?;
    match channel_id {
        Some(channel) => {
            tx.execute(
                "INSERT INTO source_mapping (provider_id, ident, stream_id, channel_id, name_norm)
                 SELECT ?1, ?2, ?3, ?4, name_norm FROM channel
                 WHERE provider_id = ?1 AND stream_id = ?3
                 ON CONFLICT (provider_id, ident, stream_id) DO UPDATE SET
                     channel_id = ?4, name_norm = excluded.name_norm",
                params![provider_id, ident, stream_id, channel],
            )?;
            insert_source_match(
                &tx,
                source,
                stream_id,
                Some(&source_guide_id(source, channel)),
            )?;
        }
        None => {
            tx.execute(
                "DELETE FROM source_mapping WHERE provider_id = ?1 AND ident = ?2 AND stream_id = ?3",
                params![provider_id, ident, stream_id],
            )?;
            // Matched again by id or name at the re-import.
            tx.execute(
                "DELETE FROM source_match WHERE source_id = ?1 AND stream_id = ?2",
                params![source, stream_id],
            )?;
        }
    }
    tx.execute(
        "UPDATE epg_source SET reimport = 1, etag = NULL, last_modified = NULL, mtime = NULL
         WHERE id = ?1",
        [source],
    )?;
    copy_pins(&tx, provider_id, source)?;
    resolve_guides(&tx, provider_id)?;
    tx.commit()
}

/// Forgets the provider's manual mappings to the source `ident`, as a
/// removal with "Also delete" does. Returns how many.
pub fn forget_mappings(
    conn: &Connection,
    provider_id: &str,
    ident: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM source_mapping WHERE provider_id = ?1 AND ident = ?2",
        params![provider_id, ident],
    )
}

/// The provider's manual mappings to source `source`, provider channel to
/// feed channel id, for its import's matching.
pub fn source_mappings(
    tx: &Connection,
    provider_id: &str,
    source: i64,
) -> rusqlite::Result<HashMap<i64, String>> {
    tx.prepare(
        "SELECT n.stream_id, n.channel_id FROM source_mapping n
         JOIN epg_source s ON s.provider_id = n.provider_id AND s.ident = n.ident
         WHERE n.provider_id = ?1 AND s.id = ?2",
    )?
    .query_map(params![provider_id, source], |r| Ok((r.get(0)?, r.get(1)?)))?
    .collect()
}

/// What one of the provider's guide sources gives one channel, for the
/// `E` card (EPG sources ticket 05).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelSource {
    pub id: i64,
    /// Empty for the provider's own guide.
    pub ident: String,
    pub enabled: bool,
    /// The source's channel it gives this one, matched or mapped, and that
    /// channel's first display name.
    pub channel: Option<String>,
    pub channel_name: Option<String>,
    /// Several of its channels matched, so none.
    pub ambiguous: bool,
    /// The feed channel it is mapped to by hand, there or not.
    pub mapped: Option<String>,
    /// It has programmes for the channel.
    pub programmes: bool,
    /// The channel is pinned to it, with this shift in minutes.
    pub pin: Option<i64>,
    /// The channel's guide comes from it now.
    pub guiding: bool,
    /// Marked for a whole fetch at the next refresh.
    pub reimport: bool,
}

/// Each of the provider's stored guide sources in order, with what it
/// gives channel `stream_id`.
pub fn channel_sources(
    conn: &Connection,
    provider_id: &str,
    stream_id: i64,
) -> rusqlite::Result<Vec<ChannelSource>> {
    let (epg_id, guide_id): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT NULLIF(epg_channel_id, ''), guide_id FROM channel
             WHERE provider_id = ?1 AND stream_id = ?2",
            params![provider_id, stream_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or_default();
    let pin: Option<(i64, i64)> = conn
        .query_row(
            "SELECT source_id, shift_minutes FROM source_pin WHERE provider_id = ?1 AND stream_id = ?2",
            params![provider_id, stream_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let has_programmes = |guide: &str| -> rusqlite::Result<bool> {
        conn.prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM programme WHERE provider_id = ?1 AND epg_channel_id = ?2)",
        )?
        .query_row(params![provider_id, guide], |r| r.get(0))
    };
    let mut sources: Vec<ChannelSource> = conn
        .prepare(
            "SELECT id, ident, enabled, reimport FROM epg_source WHERE provider_id = ?1
             ORDER BY position",
        )?
        .query_map([provider_id], |r| {
            Ok(ChannelSource {
                id: r.get(0)?,
                ident: r.get(1)?,
                enabled: r.get(2)?,
                reimport: r.get(3)?,
                ..Default::default()
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    for s in &mut sources {
        s.pin = pin.filter(|p| p.0 == s.id).map(|p| p.1);
        let (from, to) = source_range(s.id);
        let in_range = |g: &str| g >= from.as_str() && g < to.as_str();
        if s.ident.is_empty() {
            s.channel = epg_id.clone();
            s.programmes = match &epg_id {
                Some(g) => has_programmes(g)?,
                None => false,
            };
            s.guiding = guide_id
                .as_deref()
                .is_some_and(|g| Some(g) == epg_id.as_deref() || in_range(g));
            continue;
        }
        let matched: Option<Option<String>> = conn
            .prepare_cached(
                "SELECT guide_id FROM source_match WHERE source_id = ?1 AND stream_id = ?2",
            )?
            .query_row(params![s.id, stream_id], |r| r.get(0))
            .optional()?;
        s.ambiguous = matches!(matched, Some(None));
        if let Some(g) = matched.flatten() {
            s.programmes = has_programmes(&g)?;
            s.channel = g.strip_prefix(from.as_str()).map(str::to_owned);
        }
        if let Some(channel) = &s.channel {
            s.channel_name = conn
                .prepare_cached(
                    "SELECT names FROM source_channel WHERE source_id = ?1 AND channel_id = ?2",
                )?
                .query_row(params![s.id, channel], |r| r.get::<_, String>(0))
                .optional()?
                .and_then(|names| names.lines().next().map(str::to_owned));
        }
        s.mapped = conn
            .prepare_cached(
                "SELECT channel_id FROM source_mapping
                 WHERE provider_id = ?1 AND ident = ?2 AND stream_id = ?3",
            )?
            .query_row(params![provider_id, s.ident, stream_id], |r| r.get(0))
            .optional()?;
        s.guiding = guide_id.as_deref().is_some_and(in_range);
    }
    Ok(sources)
}

/// The stored id of the provider's source `ident`, `""` for its own guide.
pub fn source_id(
    conn: &Connection,
    provider_id: &str,
    ident: &str,
) -> rusqlite::Result<Option<i64>> {
    conn.prepare_cached("SELECT id FROM epg_source WHERE provider_id = ?1 AND ident = ?2")?
        .query_row(params![provider_id, ident], |r| r.get(0))
        .optional()
}

/// One of a source's channels, for the mapping picker.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceChannel {
    pub id: String,
    pub names: Vec<String>,
}

/// Up to `limit` of source `source`'s channels whose id or display names
/// hold every word of `query`, ignoring ASCII case, by name.
pub fn find_source_channels(
    conn: &Connection,
    source: i64,
    query: &str,
    limit: usize,
) -> rusqlite::Result<Vec<SourceChannel>> {
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| {
            format!(
                "%{}%",
                w.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            )
        })
        .collect();
    let mut sql = String::from("SELECT channel_id, names FROM source_channel WHERE source_id = ?1");
    for i in 0..words.len() {
        let p = i + 3;
        sql += &format!(" AND (channel_id LIKE ?{p} ESCAPE '\\' OR names LIKE ?{p} ESCAPE '\\')");
    }
    sql += " ORDER BY names COLLATE NOCASE LIMIT ?2";
    let limit = limit as i64;
    let mut values: Vec<&dyn rusqlite::ToSql> = vec![&source, &limit];
    values.extend(words.iter().map(|w| w as &dyn rusqlite::ToSql));
    conn.prepare(&sql)?
        .query_map(values.as_slice(), |r| {
            Ok(SourceChannel {
                id: r.get(0)?,
                names: r.get::<_, String>(1)?.lines().map(str::to_owned).collect(),
            })
        })?
        .collect()
}

/// The provider's channels that matched several of source `source`'s, by
/// name: `(stream_id, name)`, for Settings › EPG's ambiguous list.
pub fn ambiguous_channels(
    conn: &Connection,
    provider_id: &str,
    source: i64,
) -> rusqlite::Result<Vec<(i64, String)>> {
    conn.prepare(
        "SELECT c.stream_id, c.name FROM source_match m
         JOIN channel c ON c.provider_id = ?1 AND c.stream_id = m.stream_id
         WHERE m.source_id = ?2 AND m.guide_id IS NULL ORDER BY c.name_norm",
    )?
    .query_map(params![provider_id, source], |r| Ok((r.get(0)?, r.get(1)?)))?
    .collect()
}

/// A re-import moves a schedule to its programme's new times if they start
/// no further than this from the old (SPEC 9.5): further off is more likely
/// another showing than a move.
const FOLLOW_WITHIN: i64 = 3 * 3600;

/// Moves each of the provider's unstarted schedules to the times of the
/// programme on its channel with its title starting nearest its old start,
/// within [`FOLLOW_WITHIN`]; the padding moves with them. None leaves it.
fn follow_schedules(tx: &Connection, provider_id: &str) -> rusqlite::Result<()> {
    let schedules: Vec<(i64, String, String, i64, i64)> = tx
        .prepare(
            "SELECT id, guide_id, title, start, stop FROM recording
             WHERE provider_id = ?1 AND state = 'scheduled'
               AND guide_id IS NOT NULL AND title IS NOT NULL",
        )?
        .query_map([provider_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<Result<_, _>>()?;
    let mut nearest = tx.prepare(
        "SELECT start, stop FROM programme
         WHERE provider_id = ?1 AND epg_channel_id = ?2 AND title = ?3
           AND start BETWEEN ?4 - ?5 AND ?4 + ?5
         ORDER BY abs(start - ?4) LIMIT 1",
    )?;
    for (id, guide_id, title, start, stop) in schedules {
        let found = nearest
            .query_row(
                params![provider_id, guide_id, title, start, FOLLOW_WITHIN],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        let Some((new_start, new_stop)) = found.filter(|&t| t != (start, stop)) else {
            continue;
        };
        log::info!(
            "a schedule moved with its programme: {:+} min",
            (new_start - start) / 60
        );
        tx.execute(
            "UPDATE recording SET start = ?2, stop = ?3,
                 padded_start = padded_start + ?2 - start, padded_stop = padded_stop + ?3 - stop
             WHERE id = ?1",
            params![id, new_start, new_stop],
        )?;
    }
    Ok(())
}

/// Where a recording is (SPEC 9.5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RecState {
    Scheduled,
    Recording,
    Done,
    Failed,
    Missed,
    Cancelled,
}

impl RecState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Recording => "recording",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Missed => "missed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "scheduled" => Self::Scheduled,
            "recording" => Self::Recording,
            "done" => Self::Done,
            "missed" => Self::Missed,
            "cancelled" => Self::Cancelled,
            _ => Self::Failed,
        }
    }
}

/// A programme to record, as scheduled.
#[derive(Clone, Debug)]
pub struct NewRecording {
    pub provider_id: String,
    pub stream_id: i64,
    pub channel: String,
    pub guide_id: Option<String>,
    pub title: Option<String>,
    pub start: i64,
    pub stop: i64,
}

/// A row of `recording`. Times are Unix seconds, UTC.
#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    pub id: i64,
    pub provider_id: String,
    pub stream_id: i64,
    pub channel: String,
    pub title: Option<String>,
    pub start: i64,
    pub stop: i64,
    pub padded_start: i64,
    pub padded_stop: i64,
    pub state: RecState,
    pub file: Option<String>,
    pub note: Option<String>,
    /// Its file went to the trash (recording ticket 05).
    pub trashed: bool,
}

const RECORDING_COLUMNS: &str = "id, provider_id, stream_id, channel, title, start, stop,
     padded_start, padded_stop, state, file, note, trashed";

fn recording_row(r: &rusqlite::Row) -> rusqlite::Result<Recording> {
    Ok(Recording {
        id: r.get(0)?,
        provider_id: r.get(1)?,
        stream_id: r.get(2)?,
        channel: r.get(3)?,
        title: r.get(4)?,
        start: r.get(5)?,
        stop: r.get(6)?,
        padded_start: r.get(7)?,
        padded_stop: r.get(8)?,
        state: RecState::parse(&r.get::<_, String>(9)?),
        file: r.get(10)?,
        note: r.get(11)?,
        trashed: r.get(12)?,
    })
}

/// Schedules `r`, padded by `before` and `after` seconds. Returns its id.
pub fn schedule_recording(
    conn: &Connection,
    r: &NewRecording,
    before: i64,
    after: i64,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO recording (provider_id, stream_id, channel, guide_id, title,
             start, stop, padded_start, padded_stop, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'scheduled')",
        params![
            r.provider_id,
            r.stream_id,
            r.channel,
            r.guide_id,
            r.title,
            r.start,
            r.stop,
            r.start - before,
            r.stop + after,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn recording(conn: &Connection, id: i64) -> rusqlite::Result<Option<Recording>> {
    conn.prepare_cached(&format!(
        "SELECT {RECORDING_COLUMNS} FROM recording WHERE id = ?1"
    ))?
    .query_row([id], recording_row)
    .optional()
}

/// Every unstarted schedule, soonest first.
pub fn scheduled(conn: &Connection) -> rusqlite::Result<Vec<Recording>> {
    conn.prepare_cached(&format!(
        "SELECT {RECORDING_COLUMNS} FROM recording
         WHERE state = 'scheduled' ORDER BY padded_start, id"
    ))?
    .query_map([], recording_row)?
    .collect()
}

/// The scheduled or running recording of a channel's programme starting at
/// `start`, as the guide grid's `R` toggles it (SPEC 8.2).
pub fn active_recording(
    conn: &Connection,
    provider_id: &str,
    stream_id: i64,
    start: i64,
) -> rusqlite::Result<Option<Recording>> {
    conn.prepare_cached(&format!(
        "SELECT {RECORDING_COLUMNS} FROM recording
         WHERE state IN ('scheduled', 'recording')
           AND provider_id = ?1 AND stream_id = ?2 AND start = ?3
         ORDER BY id LIMIT 1"
    ))?
    .query_row(params![provider_id, stream_id, start], recording_row)
    .optional()
}

/// A provider's scheduled and running recordings, as channel, guide start
/// and state: the guide grid's marks. Few rows, by `recording_by_state`.
pub fn recording_marks(
    conn: &Connection,
    provider_id: &str,
) -> rusqlite::Result<Vec<(i64, i64, RecState)>> {
    conn.prepare_cached(
        "SELECT stream_id, start, state FROM recording
         WHERE state IN ('scheduled', 'recording') AND provider_id = ?1",
    )?
    .query_map([provider_id], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            RecState::parse(&r.get::<_, String>(2)?),
        ))
    })?
    .collect()
}

/// A recording of what plays (SPEC 9.4) started at `r.start`, writing to
/// `file`: stored as recording, its stop its start until it ends, with no
/// padding. Returns its id.
pub fn record_playing(conn: &Connection, r: &NewRecording, file: &str) -> rusqlite::Result<i64> {
    forget_trashed_at(conn, file)?;
    conn.execute(
        "INSERT INTO recording (provider_id, stream_id, channel, guide_id, title,
             start, stop, padded_start, padded_stop, state, file)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6, ?6, 'recording', ?7)",
        params![
            r.provider_id,
            r.stream_id,
            r.channel,
            r.guide_id,
            r.title,
            r.start,
            file,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// A recording of what plays ended at `t`.
pub fn recording_stopped_at(conn: &Connection, id: i64, t: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE recording SET stop = ?2, padded_stop = ?2 WHERE id = ?1",
        params![id, t],
    )?;
    Ok(())
}

/// Every recording, for the Recordings view (SPEC 8.2), which groups them.
pub fn recordings(conn: &Connection) -> rusqlite::Result<Vec<Recording>> {
    conn.prepare_cached(&format!(
        "SELECT {RECORDING_COLUMNS} FROM recording ORDER BY id"
    ))?
    .query_map([], recording_row)?
    .collect()
}

/// Forgets the finished recording `id`, which had no file to trash
/// (recording ticket 05). A scheduled or running one stays.
pub fn forget_recording(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM recording WHERE id = ?1 AND state NOT IN ('scheduled', 'recording')",
        [id],
    )?;
    Ok(())
}

/// Marks the finished recording `id` as trashed: its file went to the
/// trash, and the row stays, hidden while the file is gone.
pub fn trash_recording(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE recording SET trashed = 1
         WHERE id = ?1 AND state NOT IN ('scheduled', 'recording')",
        [id],
    )?;
    Ok(())
}

/// Whether the Recordings view shows `r`: not when trashed, unless its file
/// was restored.
pub fn recording_shown(r: &Recording) -> bool {
    !r.trashed || r.file.as_deref().is_some_and(|f| Path::new(f).exists())
}

/// Forgets trashed recordings at `file`, a name a new recording now takes:
/// they would show its file as theirs.
fn forget_trashed_at(conn: &Connection, file: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM recording WHERE trashed AND file = ?1", [file])?;
    Ok(())
}

/// Forgets every finished recording (done, failed, missed, cancelled),
/// keeping their files. Returns how many the view showed, and how many of
/// those had a file, not counting trashed ones.
pub fn clear_finished_recordings(conn: &Connection) -> rusqlite::Result<(usize, usize)> {
    let finished: Vec<Recording> = conn
        .prepare(&format!(
            "SELECT {RECORDING_COLUMNS} FROM recording
             WHERE state NOT IN ('scheduled', 'recording')"
        ))?
        .query_map([], recording_row)?
        .collect::<rusqlite::Result<_>>()?;
    let shown = finished.iter().filter(|r| recording_shown(r));
    let n = shown.clone().count();
    let with_file = shown.filter(|r| r.file.is_some() && !r.trashed).count();
    conn.execute(
        "DELETE FROM recording WHERE state NOT IN ('scheduled', 'recording')",
        [],
    )?;
    Ok((n, with_file))
}

/// A schedule has started, writing to `file`.
pub fn recording_started(conn: &Connection, id: i64, file: &str) -> rusqlite::Result<()> {
    forget_trashed_at(conn, file)?;
    conn.execute(
        "UPDATE recording SET state = 'recording', file = ?2 WHERE id = ?1",
        params![id, file],
    )?;
    Ok(())
}

/// Moves a recording to `state` with `note`, which replaces any earlier
/// one. `drop_file` forgets its file, e.g. one removed as empty.
pub fn set_recording_state(
    conn: &Connection,
    id: i64,
    state: RecState,
    note: Option<&str>,
    drop_file: bool,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE recording SET state = ?2, note = ?3,
             file = CASE WHEN ?4 THEN NULL ELSE file END
         WHERE id = ?1",
        params![id, state.as_str(), note, drop_file],
    )?;
    Ok(())
}

/// Cancels a schedule that has not started. False if it had.
pub fn cancel_schedule(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE recording SET state = 'cancelled' WHERE id = ?1 AND state = 'scheduled'",
        [id],
    )?;
    Ok(n > 0)
}

/// At launch: recordings left running by an app that did not quit cleanly
/// are done, cut short with `note`; schedules whose padded stop passed are
/// missed. Returns how many were missed.
pub fn settle_recordings(conn: &Connection, now: i64, note: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE recording SET state = 'done', note = ?1 WHERE state = 'recording'",
        [note],
    )?;
    miss_passed(conn, now)
}

/// Schedules whose padded stop passed unstarted are missed. Returns how many.
pub fn miss_passed(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE recording SET state = 'missed' WHERE state = 'scheduled' AND padded_stop <= ?1",
        [now],
    )
}

/// Where a download is (VOD ticket 10). A cancelled one is forgotten.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DlState {
    Queued,
    Downloading,
    Done,
    Failed,
}

impl DlState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Downloading => "downloading",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "queued" => Self::Queued,
            "downloading" => Self::Downloading,
            "done" => Self::Done,
            _ => Self::Failed,
        }
    }
}

/// A movie or episode to download. An episode has its series' id, its
/// season and number, and its own name in `episode`; `title` is the
/// movie's or series' name.
#[derive(Clone, Debug, PartialEq)]
pub struct NewDownload {
    pub provider_id: String,
    pub stream_id: i64,
    pub series_id: Option<i64>,
    pub season: Option<i64>,
    pub episode_num: Option<i64>,
    pub title: String,
    pub episode: Option<String>,
    pub extension: String,
}

/// A row of `download`.
#[derive(Clone, Debug, PartialEq)]
pub struct Download {
    pub id: i64,
    pub provider_id: String,
    pub stream_id: i64,
    pub series_id: Option<i64>,
    pub season: Option<i64>,
    pub episode_num: Option<i64>,
    pub title: String,
    pub episode: Option<String>,
    pub extension: String,
    pub state: DlState,
    pub file: Option<String>,
    pub total: Option<i64>,
    pub note: Option<String>,
    pub added: i64,
    pub ended: Option<i64>,
    /// Its file went to the trash.
    pub trashed: bool,
}

impl Download {
    /// The file it writes until done: `<file>.part`.
    pub fn part(&self) -> Option<std::path::PathBuf> {
        self.file.as_ref().map(|f| format!("{f}.part").into())
    }

    /// How many bytes it has on disk: its file once done, else the part.
    pub fn have(&self) -> u64 {
        let file = match self.state {
            DlState::Done => self.file.as_ref().map(std::path::PathBuf::from),
            _ => self.part(),
        };
        file.and_then(|f| std::fs::metadata(f).ok())
            .map_or(0, |m| m.len())
    }
}

const DOWNLOAD_COLUMNS: &str = "id, provider_id, stream_id, series_id, season, episode_num,
     title, episode, extension, state, file, total, note, added, ended, trashed";

fn download_row(r: &rusqlite::Row) -> rusqlite::Result<Download> {
    Ok(Download {
        id: r.get(0)?,
        provider_id: r.get(1)?,
        stream_id: r.get(2)?,
        series_id: r.get(3)?,
        season: r.get(4)?,
        episode_num: r.get(5)?,
        title: r.get(6)?,
        episode: r.get(7)?,
        extension: r.get(8)?,
        state: DlState::parse(&r.get::<_, String>(9)?),
        file: r.get(10)?,
        total: r.get(11)?,
        note: r.get(12)?,
        added: r.get(13)?,
        ended: r.get(14)?,
        trashed: r.get(15)?,
    })
}

/// What [`queue_downloads`] did with each title asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Queued {
    /// Added to the queue, or a failed one queued again.
    pub added: usize,
    /// Queued, downloading or downloaded already.
    pub already: usize,
}

/// Queues `titles` at the end of the queue, at `now`. One queued,
/// downloading or done with its file still there is left as it is; a
/// failed one is queued again, resuming its part; a done one whose file is
/// gone is forgotten and queued anew.
pub fn queue_downloads(
    conn: &Connection,
    titles: &[NewDownload],
    now: i64,
) -> rusqlite::Result<Queued> {
    let mut queued = Queued::default();
    for t in titles {
        let kind = if t.series_id.is_some() {
            "episode"
        } else {
            "movie"
        };
        match download_of(conn, &t.provider_id, t.series_id.is_some(), t.stream_id)? {
            Some(d) if d.state == DlState::Failed => {
                conn.execute(
                    "UPDATE download SET state = 'queued', note = NULL, added = ?2, ended = NULL
                     WHERE id = ?1",
                    params![d.id, now],
                )?;
                queued.added += 1;
                continue;
            }
            Some(d)
                if d.state != DlState::Done
                    || d.file.as_deref().is_some_and(|f| Path::new(f).exists()) =>
            {
                queued.already += 1;
                continue;
            }
            Some(d) => forget_download(conn, d.id)?,
            None => {}
        }
        conn.execute(
            "INSERT INTO download (provider_id, kind, stream_id, series_id, season,
                 episode_num, title, episode, extension, state, added)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'queued', ?10)",
            params![
                t.provider_id,
                kind,
                t.stream_id,
                t.series_id,
                t.season,
                t.episode_num,
                t.title,
                t.episode,
                t.extension,
                now,
            ],
        )?;
        queued.added += 1;
    }
    Ok(queued)
}

/// A movie's (or with `episode`, an episode's) download, if any.
pub fn download_of(
    conn: &Connection,
    provider_id: &str,
    episode: bool,
    stream_id: i64,
) -> rusqlite::Result<Option<Download>> {
    let kind = if episode { "episode" } else { "movie" };
    conn.prepare_cached(&format!(
        "SELECT {DOWNLOAD_COLUMNS} FROM download
         WHERE provider_id = ?1 AND kind = ?2 AND stream_id = ?3
         ORDER BY id DESC LIMIT 1"
    ))?
    .query_row(params![provider_id, kind, stream_id], download_row)
    .optional()
}

/// A series' episode downloads, by the episode's stream id.
pub fn episode_downloads(
    conn: &Connection,
    provider_id: &str,
    series_id: i64,
) -> rusqlite::Result<HashMap<i64, Download>> {
    conn.prepare_cached(&format!(
        "SELECT {DOWNLOAD_COLUMNS} FROM download
         WHERE provider_id = ?1 AND kind = 'episode' AND series_id = ?2 ORDER BY id"
    ))?
    .query_map(params![provider_id, series_id], download_row)?
    .map(|d| d.map(|d| (d.stream_id, d)))
    .collect()
}

pub fn download(conn: &Connection, id: i64) -> rusqlite::Result<Option<Download>> {
    conn.prepare_cached(&format!(
        "SELECT {DOWNLOAD_COLUMNS} FROM download WHERE id = ?1"
    ))?
    .query_row([id], download_row)
    .optional()
}

/// Every download, in queue order, for the Downloads tab (SPEC 8.2).
pub fn downloads(conn: &Connection) -> rusqlite::Result<Vec<Download>> {
    conn.prepare_cached(&format!(
        "SELECT {DOWNLOAD_COLUMNS} FROM download ORDER BY added, id"
    ))?
    .query_map([], download_row)?
    .collect()
}

/// The provider's next queued download.
pub fn next_download(conn: &Connection, provider_id: &str) -> rusqlite::Result<Option<Download>> {
    conn.prepare_cached(&format!(
        "SELECT {DOWNLOAD_COLUMNS} FROM download
         WHERE provider_id = ?1 AND state = 'queued' ORDER BY added, id LIMIT 1"
    ))?
    .query_row([provider_id], download_row)
    .optional()
}

/// The providers with a download queued.
pub fn queued_download_providers(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    conn.prepare_cached("SELECT DISTINCT provider_id FROM download WHERE state = 'queued'")?
        .query_map([], |r| r.get(0))?
        .collect()
}

/// A download started, writing toward `file`. Trashed ones at `file`
/// are forgotten: they would show its file as theirs.
pub fn download_started(conn: &Connection, id: i64, file: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM download WHERE trashed AND file = ?1 AND id != ?2",
        params![file, id],
    )?;
    conn.execute(
        "UPDATE download SET state = 'downloading', file = ?2 WHERE id = ?1",
        params![id, file],
    )?;
    Ok(())
}

/// The provider said how big a download is.
pub fn download_total(conn: &Connection, id: i64, total: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE download SET total = ?2 WHERE id = ?1",
        params![id, total],
    )?;
    Ok(())
}

/// Moves a download to `state`, with `note`, at `now` if it ended. A done
/// one's `file` is where it was renamed to.
pub fn set_download_state(
    conn: &Connection,
    id: i64,
    state: DlState,
    note: Option<&str>,
    file: Option<&str>,
    now: i64,
) -> rusqlite::Result<()> {
    let ended = matches!(state, DlState::Done | DlState::Failed).then_some(now);
    conn.execute(
        "UPDATE download SET state = ?2, note = ?3, file = coalesce(?4, file), ended = ?5
         WHERE id = ?1",
        params![id, state.as_str(), note, file, ended],
    )?;
    Ok(())
}

/// Marks done download `id` as trashed: its file went to the trash, and
/// the row stays, hidden while the file is gone.
pub fn trash_download(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE download SET trashed = 1 WHERE id = ?1 AND state = 'done'",
        [id],
    )?;
    Ok(())
}

/// Whether the Downloads tab shows `d`: not when trashed, unless its file
/// was restored.
pub fn download_shown(d: &Download) -> bool {
    !d.trashed || d.file.as_deref().is_some_and(|f| Path::new(f).exists())
}

pub fn forget_download(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM download WHERE id = ?1", [id])?;
    Ok(())
}

/// Forgets every done and failed download, trashed ones too, keeping done
/// ones' files. Returns them as they were, so failed ones' parts can be
/// removed.
pub fn clear_finished_downloads(conn: &Connection) -> rusqlite::Result<Vec<Download>> {
    let finished: Vec<Download> = conn
        .prepare(&format!(
            "SELECT {DOWNLOAD_COLUMNS} FROM download WHERE state IN ('done', 'failed')"
        ))?
        .query_map([], download_row)?
        .collect::<rusqlite::Result<_>>()?;
    conn.execute("DELETE FROM download WHERE state IN ('done', 'failed')", [])?;
    Ok(finished)
}

/// At launch and quit: downloads left running are queued again, to resume.
pub fn settle_downloads(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE download SET state = 'queued' WHERE state = 'downloading'",
        [],
    )
}

/// A channel's stream extension, if the provider still lists it.
pub fn channel_extension(
    conn: &Connection,
    provider_id: &str,
    stream_id: i64,
) -> rusqlite::Result<Option<String>> {
    conn.prepare_cached("SELECT extension FROM channel WHERE provider_id = ?1 AND stream_id = ?2")?
        .query_row(params![provider_id, stream_id], |r| r.get(0))
        .optional()
}

/// Quality tags that do not change which channel it is. `+1` style timeshift
/// markers are deliberately not here.
const QUALITY: &[&str] = &["sd", "hd", "fhd", "uhd", "4k", "8k", "hevc", "h265", "lq"];

/// SPEC 5 `name_norm`: compatibility-folded (so `ᴴᴰ` reads as `HD`),
/// lowercased, whitespace-collapsed, with trailing quality tags stripped.
pub fn name_norm(name: &str) -> String {
    let folded = name.nfkc().collect::<String>().to_lowercase();
    let mut words: Vec<&str> = folded.split_whitespace().collect();
    while words.len() > 1 && is_quality(words[words.len() - 1]) {
        words.pop();
    }
    words.join(" ")
}

/// A quality tag, or a resolution such as `1080p`.
fn is_quality(word: &str) -> bool {
    QUALITY.contains(&word)
        || word
            .strip_suffix('p')
            .is_some_and(|d| d.len() >= 3 && d.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_provider_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.db");
        {
            let conn = open(&path).unwrap();
            assert_eq!(last_provider(&conn).unwrap(), None);
            set_last_provider(&conn, "a").unwrap();
            set_last_provider(&conn, "b").unwrap();
        }
        assert_eq!(
            last_provider(&open(&path).unwrap()).unwrap().as_deref(),
            Some("b")
        );
    }

    /// A row for `provider_id` in every table that has one, with a
    /// scheduled and a done recording, a queued and a done download.
    fn fill(conn: &Connection, provider_id: &str) {
        conn.execute_batch(&format!(
            "INSERT INTO provider (id, name, last_channels_refresh) VALUES ('{p}', 'P', 1);
             INSERT INTO category (provider_id, kind, category_id, name, sort) VALUES ('{p}', 'live', '1', 'News', 0);
             INSERT INTO channel (provider_id, stream_id, name, name_norm, extension) VALUES ('{p}', 7, 'Seven', 'seven', 'ts');
             INSERT INTO movie (provider_id, stream_id, name, name_norm) VALUES ('{p}', 8, 'Film', 'film');
             INSERT INTO series (provider_id, series_id, name, name_norm) VALUES ('{p}', 9, 'Show', 'show');
             INSERT INTO episode (provider_id, series_id, season, stream_id, title, sort) VALUES ('{p}', 9, 1, 90, 'E1', 0);
             INSERT INTO programme (provider_id, epg_channel_id, start, stop, title) VALUES ('{p}', 'seven', 0, 60, 'News');
             INSERT INTO favourite (provider_id, kind, stream_id, name_norm, position, name) VALUES ('{p}', 'live', 7, 'seven', 0, 'Seven');
             INSERT INTO progress (provider_id, kind, stream_id, pos, watched, at) VALUES ('{p}', 'movie', 8, 600, 0, 1);
             INSERT INTO history (provider_id, kind, stream_id, played_at) VALUES ('{p}', 'live', 7, 1);
             INSERT INTO recording (provider_id, stream_id, channel, start, stop, padded_start, padded_stop, state)
                 VALUES ('{p}', 7, 'Seven', 0, 60, 0, 60, 'scheduled'), ('{p}', 7, 'Seven', 0, 60, 0, 60, 'done');
             INSERT INTO download (provider_id, kind, stream_id, title, extension, state, added)
                 VALUES ('{p}', 'movie', 8, 'Film', 'mkv', 'queued', 1), ('{p}', 'movie', 8, 'Film', 'mkv', 'done', 1);
             INSERT INTO category_rule (provider_id, kind, pattern) VALUES ('{p}', 'live', 'uk');
             INSERT INTO category_override (provider_id, kind, category_id, name, shown) VALUES ('{p}', 'live', '1', 'News', 0);
             INSERT INTO channel_rule (provider_id, pattern) VALUES ('{p}', 'fr:');
             INSERT INTO kept_tag (provider_id, tag) VALUES ('{p}', 'uk');
             INSERT INTO app_state (key, value) VALUES ('continue_cleared:{p}', '1');",
            p = provider_id
        ))
        .unwrap();
    }

    /// Each table's rows for `provider_id`, by table name.
    fn rows(conn: &Connection, provider_id: &str) -> Vec<(&'static str, i64)> {
        let count = |sql: String| conn.query_row(&sql, [provider_id], |r| r.get(0)).unwrap();
        let mut rows: Vec<(&str, i64)> = [
            "category",
            "channel",
            "movie",
            "series",
            "episode",
            "programme",
            "favourite",
            "progress",
            "history",
            "recording",
            "download",
            "category_rule",
            "category_override",
            "channel_rule",
            "kept_tag",
        ]
        .into_iter()
        .map(|t| {
            (
                t,
                count(format!("SELECT count(*) FROM {t} WHERE provider_id = ?1")),
            )
        })
        .collect();
        rows.push((
            "provider",
            count("SELECT count(*) FROM provider WHERE id = ?1".into()),
        ));
        rows.push((
            "app_state",
            count("SELECT count(*) FROM app_state WHERE substr(key, 18) = ?1".into()),
        ));
        rows
    }

    /// Onboarding ticket 02: the lists, guide and refresh times go; the
    /// personal data stays unless asked, schedules are cancelled and queued
    /// downloads forgotten; the other provider is untouched.
    #[test]
    fn removing_a_provider_keeps_personal_data_unless_asked() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open(&dir.path().join("v.db")).unwrap();
        fill(&conn, "p");
        fill(&conn, "q");
        set_last_provider(&conn, "p").unwrap();
        let other = rows(&conn, "q");
        assert_eq!(
            provider_data(&conn, "p").unwrap(),
            ProviderData {
                channels: 1,
                movies: 1,
                series: 1,
                scheduled: 1,
                queued: 1,
                favourites: 1,
                progress: 1,
                recorded: 2,
            }
        );

        remove_provider(&conn, "p", false).unwrap();
        let kept = rows(&conn, "p");
        for (table, n) in &kept {
            let want = match *table {
                "favourite" | "progress" | "history" | "category_rule" | "category_override"
                | "channel_rule" | "kept_tag" | "app_state" | "download" => 1,
                "recording" => 2,
                _ => 0,
            };
            assert_eq!(*n, want, "{table}");
        }
        let states: Vec<String> = conn
            .prepare("SELECT state FROM recording WHERE provider_id = 'p' ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(states, ["cancelled", "done"]);
        assert_eq!(last_provider(&conn).unwrap(), None);
        assert_eq!(rows(&conn, "q"), other);
        let fts: i64 = conn
            .query_row(
                "SELECT count(*) FROM channel_fts WHERE channel_fts MATCH 'seven'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fts, 1, "q's channel still found");

        // Added again with the same id: its favourite is there, available.
        import_channels(&mut conn, &[7]);
        let available: i64 = conn
            .query_row(
                "SELECT count(*) FROM favourite JOIN channel USING (provider_id, stream_id)
                 WHERE provider_id = 'p'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(available, 1);

        remove_provider(&conn, "p", true).unwrap();
        assert!(
            rows(&conn, "p").iter().all(|(_, n)| *n == 0),
            "{:?}",
            rows(&conn, "p")
        );
        assert_eq!(rows(&conn, "q"), other);
    }

    #[test]
    fn name_norm_folds_case_and_whitespace() {
        assert_eq!(name_norm("  Harbour   One\tLondon "), "harbour one london");
        assert_eq!(name_norm("UK: Northwind Sports"), "uk: northwind sports");
    }

    #[test]
    fn name_norm_strips_quality_suffixes() {
        assert_eq!(name_norm("UK: HARBOUR ONE HD"), "uk: harbour one");
        assert_eq!(name_norm("UK: HARBOUR ONE FHD"), "uk: harbour one");
        assert_eq!(name_norm("US: SUMMIT 4K"), "us: summit");
        assert_eq!(
            name_norm("UK: COBALT CINEMA GREATS HEVC 4K"),
            "uk: cobalt cinema greats"
        );
        assert_eq!(name_norm("FR: RIVAGE 1080p"), "fr: rivage");
    }

    #[test]
    fn name_norm_folds_superscript_tags() {
        assert_eq!(name_norm("DE: KANAL VIER ᴴᴰ"), "de: kanal vier");
        assert_eq!(
            name_norm("4K: NORTHWIND ARENA ᵁᴴᴰ ³⁸⁴⁰ᴾ"),
            "4k: northwind arena"
        );
        assert_eq!(name_norm("UK: KESTREL ʰᵉᵛᶜ"), "uk: kestrel");
        assert_eq!(
            name_norm("DE: KANAL VIER HD"),
            name_norm("DE: KANAL VIER ᴴᴰ")
        );
    }

    #[test]
    fn name_norm_keeps_meaningful_suffixes() {
        assert_eq!(name_norm("UK: KESTREL +1"), "uk: kestrel +1");
        assert_eq!(name_norm("UK: KESTREL +1 HD"), "uk: kestrel +1");
        assert_eq!(name_norm("AT: SABLE MUSIC ᴿᴬᵂ"), "at: sable music raw");
        assert_eq!(name_norm("SE: MERIDIAN TV ⱽᴵᴾ"), "se: meridian tv vip");
        assert_eq!(name_norm("VANTA 2"), "vanta 2");
    }

    #[test]
    fn name_norm_never_empties_a_name() {
        assert_eq!(name_norm("HD"), "hd");
        assert_eq!(name_norm("4K HD"), "4k");
        assert_eq!(name_norm(""), "");
    }

    /// A database holding provider `p`'s channels with these stream ids.
    fn with_channels(ids: &[i64]) -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open(&dir.path().join("v.db")).unwrap();
        import_channels(&mut conn, ids);
        (dir, conn)
    }

    fn import_channels(conn: &mut Connection, ids: &[i64]) {
        let tx = conn.transaction().unwrap();
        begin_channel_import(&tx, "p", "P", []).unwrap();
        for &id in ids {
            let name = format!("Channel {id}");
            let c = NewChannel {
                stream_id: id,
                num: Some(id),
                name: &name,
                category_id: None,
                epg_channel_id: None,
                icon_url: None,
                extension: "ts",
                archive_days: 0,
            };
            insert_channel(&tx, "p", &c).unwrap();
        }
        finish_channel_import(&tx, "p", 0).unwrap();
        tx.commit().unwrap();
    }

    /// Recently played as stream ids, latest first.
    fn recent_ids(conn: &Connection, limit: usize) -> Vec<i64> {
        recent(conn, "p", limit)
            .unwrap()
            .into_iter()
            .map(|k| channel(conn, k, 0).unwrap().unwrap().stream_id)
            .collect()
    }

    fn last_id(conn: &Connection, current: Option<i64>) -> Option<i64> {
        last_channel(conn, "p", current)
            .unwrap()
            .map(|k| channel(conn, k, 0).unwrap().unwrap().stream_id)
    }

    #[test]
    fn history_is_latest_first_and_replays_move_up() {
        let (_dir, conn) = with_channels(&[1, 2, 3]);
        record_play(&conn, "p", Kind::Live, 1, 100).unwrap();
        record_play(&conn, "p", Kind::Live, 2, 200).unwrap();
        record_play(&conn, "p", Kind::Live, 3, 300).unwrap();
        assert_eq!(recent_ids(&conn, 4), [3, 2, 1]);
        record_play(&conn, "p", Kind::Live, 1, 400).unwrap();
        assert_eq!(recent_ids(&conn, 4), [1, 3, 2]);
        assert_eq!(recent_ids(&conn, 2), [1, 3]);
        // Within one second, the later play still comes first.
        record_play(&conn, "p", Kind::Live, 2, 400).unwrap();
        assert_eq!(recent_ids(&conn, 4), [2, 1, 3]);
        // Another provider's history is separate.
        record_play(&conn, "q", Kind::Live, 3, 500).unwrap();
        assert_eq!(recent_ids(&conn, 4), [2, 1, 3]);
    }

    #[test]
    fn history_is_pruned_to_fifty() {
        let ids: Vec<i64> = (1..=55).collect();
        let (_dir, conn) = with_channels(&ids);
        for &id in &ids {
            record_play(&conn, "p", Kind::Live, id, id).unwrap();
        }
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 50);
        assert_eq!(recent_ids(&conn, 60), (6..=55).rev().collect::<Vec<_>>());
    }

    #[test]
    fn clear_empties_one_providers_history() {
        let (_dir, conn) = with_channels(&[1, 2]);
        record_play(&conn, "p", Kind::Live, 1, 100).unwrap();
        record_play(&conn, "p", Kind::Live, 2, 200).unwrap();
        record_play(&conn, "q", Kind::Live, 1, 300).unwrap();
        clear_history(&conn, "p").unwrap();
        assert_eq!(recent_ids(&conn, 4), Vec::<i64>::new());
        assert_eq!(last_id(&conn, None), None);
        let left: Vec<String> = conn
            .prepare("SELECT provider_id FROM history")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(left, ["q"]);
    }

    #[test]
    fn channels_gone_after_an_import_drop_out_of_history() {
        let (_dir, mut conn) = with_channels(&[1, 2, 3]);
        for id in [1, 2, 3] {
            record_play(&conn, "p", Kind::Live, id, id).unwrap();
        }
        import_channels(&mut conn, &[1, 3, 4]);
        assert_eq!(recent_ids(&conn, 4), [3, 1]);
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2);
    }

    #[test]
    fn last_channel_skips_the_one_playing() {
        let (_dir, conn) = with_channels(&[1, 2, 3]);
        assert_eq!(last_id(&conn, None), None, "no history");
        assert_eq!(last_id(&conn, Some(1)), None, "no history, playing");

        record_play(&conn, "p", Kind::Live, 1, 100).unwrap();
        assert_eq!(last_id(&conn, Some(1)), None, "only the one playing");
        assert_eq!(last_id(&conn, None), Some(1), "stopped: the last one");

        record_play(&conn, "p", Kind::Live, 2, 200).unwrap();
        record_play(&conn, "p", Kind::Live, 3, 300).unwrap();
        assert_eq!(last_id(&conn, Some(3)), Some(2));
        // Pressed again after it plays: back to the other.
        record_play(&conn, "p", Kind::Live, 2, 400).unwrap();
        assert_eq!(last_id(&conn, Some(2)), Some(3));
        // A play not yet written: the latest written one is before it.
        assert_eq!(last_id(&conn, Some(1)), Some(2));
    }

    #[test]
    fn alternates_share_the_looser_name_shown_ones_only_in_order() {
        let cats = [("1", "Sport"), ("2", "US Sport"), ("3", "Hidden")];
        let chans = [
            (1, 4, "4K: Northwind Sport ᵁᴴᴰ", "1"),
            (2, 900, "US: Northwind Sport HD", "2"),
            (3, 50, "UK: Northwind Sport", "2"),
            (4, 60, "Northwind Sport FHD", "2"),
            (5, 70, "VIP: Northwind Sport", "1"),
            (6, 80, "Northwind Sport +1", "1"),
            (7, 10, "DE: Northwind Sport", "3"),
            (8, 20, "FR: Northwind Sport", "1"),
            (9, 30, "Juniper", "1"),
            (10, 95, "US: Northwind Sport FHD", "1"),
            (11, 1, "##### NORTHWIND SPORT #####", "1"),
            (12, 2, "#### NORTHWIND SPORT ####", "2"),
        ];
        let (_dir, conn) = with_lists(&cats, &chans);
        conn.execute_batch(
            "UPDATE category SET shown = 0 WHERE category_id = '3';
             UPDATE channel SET shown = 0 WHERE stream_id = 8;",
        )
        .unwrap();
        let of = |id: i64, limit: usize| -> Vec<i64> {
            let key = channel_key(&conn, "p", id).unwrap().unwrap();
            alternates(&conn, "p", key, limit)
                .unwrap()
                .into_iter()
                .map(|k| channel(&conn, k, 0).unwrap().unwrap().stream_id)
                .collect()
        };
        // Same `name_norm` (10), then the same category (3, 4), then by
        // number; `+1` is another channel, hidden ones and dividers are
        // left out.
        assert_eq!(of(2, 10), [10, 3, 4, 1, 5]);
        assert_eq!(of(1, 10), [5, 10, 3, 4, 2]);
        assert_eq!(of(1, 2), [5, 10]);
        assert_eq!(of(9, 10), Vec::<i64>::new());
        assert_eq!(of(6, 10), Vec::<i64>::new());
        assert_eq!(of(11, 10), Vec::<i64>::new());
    }

    #[test]
    fn migration_28_fills_the_looser_name_of_stored_channels() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.db");
        {
            let mut conn = open(&path).unwrap();
            import_lists(
                &mut conn,
                &[("1", "Sport")],
                &[(1, 1, "US: Northwind ᴴᴰ", "1")],
            );
            conn.execute_batch(
                "DROP INDEX channel_by_loose;
                 ALTER TABLE channel DROP COLUMN name_loose;
                 PRAGMA user_version = 27;",
            )
            .unwrap();
        }
        let conn = open(&path).unwrap();
        let loose: String = conn
            .query_row("SELECT name_loose FROM channel", [], |r| r.get(0))
            .unwrap();
        assert_eq!(loose, "northwind");
    }

    /// Provider `p` with categories `(id, name)` and channels
    /// `(stream_id, num, name, category_id)`, inserted in this order.
    fn with_lists(
        cats: &[(&str, &str)],
        chans: &[(i64, i64, &str, &str)],
    ) -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open(&dir.path().join("v.db")).unwrap();
        import_lists(&mut conn, cats, chans);
        (dir, conn)
    }

    /// Replaces provider `p`'s lists, as a channel import does.
    fn import_lists(
        conn: &mut Connection,
        cats: &[(&str, &str)],
        chans: &[(i64, i64, &str, &str)],
    ) {
        let tx = conn.transaction().unwrap();
        begin_channel_import(&tx, "p", "P", cats.iter().copied()).unwrap();
        for &(stream_id, num, name, cat) in chans {
            let c = NewChannel {
                stream_id,
                num: Some(num),
                name,
                category_id: Some(cat),
                epg_channel_id: None,
                icon_url: None,
                extension: "ts",
                archive_days: 0,
            };
            insert_channel(&tx, "p", &c).unwrap();
        }
        finish_channel_import(&tx, "p", 0).unwrap();
        tx.commit().unwrap();
    }

    fn ids(conn: &Connection, view: View) -> Vec<i64> {
        channel_keys(conn, "p", view)
            .unwrap()
            .into_iter()
            .map(|k| channel(conn, k, 0).unwrap().unwrap().stream_id)
            .collect()
    }

    #[test]
    fn categories_a_to_z_by_name_norm_ties_in_provider_order() {
        let cats = [
            ("1", "UK| Sport"),
            ("2", "DE| News"),
            ("3", "uk| sport"),
            ("4", "AT| Film ᴴᴰ"),
        ];
        let (_dir, conn) = with_lists(&cats, &[]);
        let order = |o| -> Vec<String> {
            categories(&conn, "p", Kind::Live, o)
                .unwrap()
                .into_iter()
                .map(|(id, _)| id)
                .collect()
        };
        assert_eq!(order(Order::Provider), ["1", "2", "3", "4"]);
        assert_eq!(order(Order::Az), ["4", "2", "1", "3"]);
    }

    #[test]
    fn channels_a_to_z_by_name_norm_ties_in_provider_order() {
        let (_dir, conn) = with_lists(
            &[("a", "A"), ("b", "B")],
            &[
                (10, 1, "Vanta 2", "a"),
                (11, 4, "Harbour One HD", "a"),
                (12, 3, "Cobalt", "b"),
                // Same name_norm as 11 but numbered before it.
                (13, 2, "HARBOUR ONE ᴴᴰ", "a"),
                (14, 5, "Harbour One +1", "a"),
            ],
        );
        assert_eq!(ids(&conn, View::All(Order::Provider)), [10, 13, 12, 11, 14]);
        assert_eq!(ids(&conn, View::All(Order::Az)), [12, 13, 11, 14, 10]);
        assert_eq!(
            ids(&conn, View::Category("a", Order::Provider)),
            [10, 13, 11, 14]
        );
        assert_eq!(ids(&conn, View::Category("a", Order::Az)), [13, 11, 14, 10]);
    }

    #[test]
    fn a_to_z_is_served_from_an_index() {
        let (_dir, conn) = with_channels(&[1]);
        for sql in [
            format!(
                "SELECT rowid FROM channel c WHERE provider_id = ?1 AND {NOT_HIDDEN} AND {NOT_RULED}
                 ORDER BY name_norm, num, category_id, rowid"
            ),
            format!(
                "SELECT rowid FROM channel c WHERE provider_id = ?1 AND category_id = 'a'
                 AND {NOT_RULED} ORDER BY name_norm, num, rowid"
            ),
        ] {
            let plan: Vec<String> = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map(["p"], |r| r.get(3))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            let plan = plan.join("; ");
            assert!(plan.contains("COVERING INDEX"), "{plan}");
            assert!(!plan.contains("TEMP B-TREE"), "{plan}");
            assert!(plan.contains("channel_hidden"), "{plan}");
            assert!(!plan.contains("SCAN channel"), "{plan}");
        }
    }

    #[test]
    fn archives_by_key_are_read_from_their_index() {
        let (_dir, conn) = with_channels(&[1, 2, 3]);
        conn.execute("UPDATE channel SET archive_days = 3 WHERE num = 2", [])
            .unwrap();
        conn.execute("UPDATE channel SET archive_days = 7 WHERE num = 3", [])
            .unwrap();
        let key = |num: i64| -> i64 {
            conn.query_row("SELECT rowid FROM channel WHERE num = ?1", [num], |r| {
                r.get(0)
            })
            .unwrap()
        };
        let got = archive_days_by_key(&conn, "p").unwrap();
        assert_eq!(got, HashMap::from([(key(2), 3), (key(3), 7)]));
        assert!(archive_days_by_key(&conn, "other").unwrap().is_empty());
        let plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN SELECT rowid, archive_days FROM channel
                 WHERE provider_id = 'p' AND archive_days > 0",
                [],
                |r| r.get(3),
            )
            .unwrap();
        assert!(plan.contains("COVERING INDEX channel_archive"), "{plan}");
    }

    /// Replaces provider `p`'s movies: `(id, name, category, added)`, in
    /// provider order, in categories `cats`.
    fn import_movie_list(
        conn: &mut Connection,
        cats: &[(&str, &str)],
        movies: &[(i64, &str, &str, Option<i64>)],
    ) {
        let tx = conn.transaction().unwrap();
        begin_import(&tx, "p", "P", Kind::Movie, cats.iter().copied()).unwrap();
        for &(id, name, cat, added) in movies {
            let m = NewMovie {
                stream_id: id,
                num: Some(id),
                name,
                category_id: Some(cat),
                rating: Some(7.25),
                poster_url: Some(""),
                extension: Some("mkv"),
                added,
            };
            insert_movie(&tx, "p", &m).unwrap();
        }
        finish_vod_import(&tx, "p", Kind::Movie, 0).unwrap();
        tx.commit().unwrap();
    }

    /// Watched past 90 % or in the last 5 minutes, whichever is first, the
    /// 5 minutes at most a quarter (VOD ticket 07).
    #[test]
    fn watched_at_90_percent_or_the_last_5_minutes() {
        // 2 h: 90 % is 12 min from the end, before the last 5.
        assert!(!is_watched(6480.0 - 1.0, 7200.0));
        assert!(is_watched(6480.0, 7200.0));
        // 22 min: the last 5 minutes come first.
        assert!(!is_watched(1020.0 - 1.0, 1320.0));
        assert!(is_watched(1020.0, 1320.0));
        // 5 min: a quarter, not the whole of it.
        assert!(!is_watched(0.0, 300.0));
        assert!(!is_watched(224.0, 300.0));
        assert!(is_watched(225.0, 300.0));
        // No length, nothing is watched.
        assert!(!is_watched(100.0, 0.0));
    }

    /// Where a movie or episode stopped is read back; the first minute and
    /// a watched one start from the beginning, and watched stays (VOD
    /// ticket 07).
    #[test]
    fn progress_is_saved_and_read_back() {
        let (_dir, conn) = with_lists(&REGIONS, &[]);
        let movie = Watchable::Movie(7);
        assert_eq!(progress(&conn, "p", &movie).unwrap(), None);
        save_progress(&conn, "p", &movie, 59.5, None, false, 1).unwrap();
        assert_eq!(progress(&conn, "p", &movie).unwrap().unwrap().pos, 0);
        save_progress(&conn, "p", &movie, 600.4, Some(5594.0), false, 2).unwrap();
        let p = Progress {
            pos: 600,
            length: Some(5594),
            watched: false,
        };
        assert_eq!(progress(&conn, "p", &movie).unwrap(), Some(p));
        // The length stays when mpv has not said it yet.
        save_progress(&conn, "p", &movie, 900.0, None, false, 3).unwrap();
        assert_eq!(
            progress(&conn, "p", &movie).unwrap().unwrap().length,
            Some(5594)
        );
        save_progress(&conn, "p", &movie, 5100.0, Some(5594.0), false, 4).unwrap();
        let p = progress(&conn, "p", &movie).unwrap().unwrap();
        assert_eq!((p.pos, p.watched), (0, true));
        // Watched again part way: it resumes, still watched.
        save_progress(&conn, "p", &movie, 1200.0, Some(5594.0), false, 5).unwrap();
        let p = progress(&conn, "p", &movie).unwrap().unwrap();
        assert_eq!((p.pos, p.watched), (1200, true));
        // The end reached, with no length known.
        let ep = Watchable::Episode {
            series_id: 1,
            stream_id: 7,
            season: 1,
            num: Some(1),
        };
        save_progress(&conn, "p", &ep, 10.0, None, true, 6).unwrap();
        assert!(progress(&conn, "p", &ep).unwrap().unwrap().watched);
        // A movie and an episode with the same stream id are apart.
        assert_eq!(progress(&conn, "p", &movie).unwrap().unwrap().pos, 1200);
        assert_eq!(progress(&conn, "q", &movie).unwrap(), None);
    }

    fn episode(season: i64, num: i64, stream_id: i64) -> Episode {
        Episode {
            season,
            num: Some(num),
            stream_id,
            title: format!("Harbour Lights - S{season:02}E{num:02}"),
            extension: Some("mkv".into()),
            length: Some(2900),
        }
    }

    /// A series carries on part way, then with the next episode, into the
    /// next season, and is finished after its last (VOD ticket 07).
    #[test]
    fn up_next_follows_seasons_to_the_last_episode() {
        let eps = [episode(1, 1, 11), episode(1, 2, 12), episode(2, 1, 21)];
        let last = |stream_id, season, num, pos| LastEpisode {
            stream_id,
            season,
            num: Some(num),
            progress: Progress {
                pos,
                length: Some(2900),
                watched: pos == 0,
            },
        };
        assert_eq!(up_next(&eps, None), UpNext::Nothing);
        let part = last(11, 1, 1, 700);
        assert_eq!(
            up_next(&eps, Some(&part)),
            UpNext::Resume(eps[0].clone(), part.progress)
        );
        assert_eq!(
            up_next(&eps, Some(&last(11, 1, 1, 0))),
            UpNext::Next(eps[1].clone())
        );
        // The end of a season: the next one's first.
        assert_eq!(
            up_next(&eps, Some(&last(12, 1, 2, 0))),
            UpNext::Next(eps[2].clone())
        );
        assert_eq!(up_next(&eps, Some(&last(21, 2, 1, 0))), UpNext::Finished);
        // Renumbered streams: found by season and number.
        assert_eq!(
            up_next(&eps, Some(&last(99, 1, 2, 0))),
            UpNext::Next(eps[2].clone())
        );
        assert_eq!(up_next(&eps, Some(&last(99, 3, 1, 0))), UpNext::Unknown);
    }

    /// Continue watching: movies part way and series to carry on with,
    /// latest first; finished ones and those the lists lost are left out.
    #[test]
    fn continue_watching_lists_what_is_part_way() {
        let (_dir, mut conn) = with_lists(&REGIONS, &[]);
        import_movie_list(
            &mut conn,
            &[("1", "EN - Drama")],
            &[(7, "Quiet Shore", "1", None), (8, "Far Fields", "1", None)],
        );
        let tx = conn.transaction().unwrap();
        begin_import(&tx, "p", "P", Kind::Series, [("1", "Crime Series")]).unwrap();
        for (series_id, name) in [(1, "Harbour Lights"), (2, "Tin Roof")] {
            let s = NewSeries {
                series_id,
                num: Some(series_id),
                name,
                category_id: Some("1"),
                year: None,
                rating: None,
                cover_url: None,
                genre: None,
                plot: None,
                actors: None,
                director: None,
                episode_run_time: None,
                last_modified: None,
            };
            insert_series(&tx, "p", &s).unwrap();
        }
        finish_vod_import(&tx, "p", Kind::Series, 0).unwrap();
        tx.commit().unwrap();
        let info = SeriesInfo {
            episodes: vec![episode(1, 1, 11), episode(1, 2, 12)],
            last_year: None,
        };
        set_series_info(&mut conn, "p", 1, &info, 0).unwrap();
        let ep = |series_id, stream_id, num| Watchable::Episode {
            series_id,
            stream_id,
            season: 1,
            num: Some(num),
        };
        let names = |conn: &Connection| -> Vec<String> {
            continue_watching(conn, "p")
                .unwrap()
                .into_iter()
                .map(|k| {
                    let (kind, key) = from_list_key(k);
                    title(conn, kind, key).unwrap().unwrap().name
                })
                .collect()
        };
        assert!(names(&conn).is_empty());
        save_progress(
            &conn,
            "p",
            &Watchable::Movie(7),
            600.0,
            Some(5594.0),
            false,
            1,
        )
        .unwrap();
        // Watched, so not part way.
        save_progress(&conn, "p", &Watchable::Movie(8), 0.0, None, true, 2).unwrap();
        // Episode 1 watched: episode 2 is next.
        save_progress(&conn, "p", &ep(1, 11, 1), 0.0, None, true, 3).unwrap();
        // Its episodes not read: listed on what was played.
        save_progress(&conn, "p", &ep(2, 31, 1), 300.0, None, false, 4).unwrap();
        // A movie the list no longer has.
        save_progress(&conn, "p", &Watchable::Movie(99), 600.0, None, false, 5).unwrap();
        assert_eq!(names(&conn), ["Tin Roof", "Harbour Lights", "Quiet Shore"]);
        assert_eq!(
            series_up_next(&conn, "p", 1).unwrap(),
            UpNext::Next(episode(1, 2, 12))
        );
        assert_eq!(series_up_next(&conn, "p", 2).unwrap(), UpNext::Unknown);
        // The last episode watched: finished, so it leaves.
        save_progress(&conn, "p", &ep(1, 12, 2), 0.0, None, true, 6).unwrap();
        assert_eq!(names(&conn), ["Tin Roof", "Quiet Shore"]);
        // Episode 1 played again for a few seconds: not counted.
        save_progress(&conn, "p", &ep(1, 11, 1), 20.0, None, false, 7).unwrap();
        assert_eq!(series_up_next(&conn, "p", 1).unwrap(), UpNext::Finished);

        // Cleared: empty, positions kept; played again, it comes back.
        clear_continue(&conn, "p", 8).unwrap();
        assert!(names(&conn).is_empty());
        let movie = progress(&conn, "p", &Watchable::Movie(7)).unwrap().unwrap();
        assert_eq!(movie.pos, 600);
        assert_eq!(continue_watching(&conn, "q").unwrap().len(), 0);
        save_progress(&conn, "p", &Watchable::Movie(7), 700.0, None, false, 9).unwrap();
        assert_eq!(names(&conn), ["Quiet Shore"]);
    }

    /// `w` marks a season watched, its last the series' latest, and
    /// unwatched forgets them, so the series carries on there (VOD ticket
    /// 07).
    #[test]
    fn marking_watched_and_unwatched_moves_up_next() {
        let (_dir, mut conn) = with_lists(&REGIONS, &[]);
        let eps = [
            episode(1, 1, 11),
            episode(1, 2, 12),
            episode(2, 1, 21),
            episode(2, 2, 22),
        ];
        let tx = conn.transaction().unwrap();
        begin_import(&tx, "p", "P", Kind::Series, [("1", "Crime Series")]).unwrap();
        let s = NewSeries {
            series_id: 1,
            num: Some(1),
            name: "Harbour Lights",
            category_id: Some("1"),
            year: None,
            rating: None,
            cover_url: None,
            genre: None,
            plot: None,
            actors: None,
            director: None,
            episode_run_time: None,
            last_modified: None,
        };
        insert_series(&tx, "p", &s).unwrap();
        finish_vod_import(&tx, "p", Kind::Series, 0).unwrap();
        tx.commit().unwrap();
        let info = SeriesInfo {
            episodes: eps.to_vec(),
            last_year: None,
        };
        set_series_info(&mut conn, "p", 1, &info, 0).unwrap();
        let w = |e: &Episode| Watchable::Episode {
            series_id: 1,
            stream_id: e.stream_id,
            season: e.season,
            num: e.num,
        };
        let season1: Vec<_> = eps[..2].iter().map(w).collect();
        // Part way through episode 1, then the season marked: watched.
        save_progress(&conn, "p", &season1[0], 600.0, Some(2900.0), false, 1).unwrap();
        set_watched(&mut conn, "p", &season1, true, 2).unwrap();
        assert_eq!(progress(&conn, "p", &season1[0]).unwrap().unwrap().pos, 0);
        assert_eq!(
            series_up_next(&conn, "p", 1).unwrap(),
            UpNext::Next(eps[2].clone())
        );
        // Season 2 too: finished. Its last unwatched: carries on there.
        let season2: Vec<_> = eps[2..].iter().map(w).collect();
        set_watched(&mut conn, "p", &season2, true, 3).unwrap();
        assert_eq!(series_up_next(&conn, "p", 1).unwrap(), UpNext::Finished);
        set_watched(&mut conn, "p", &season2[1..], false, 4).unwrap();
        assert_eq!(progress(&conn, "p", &season2[1]).unwrap(), None);
        assert_eq!(
            series_up_next(&conn, "p", 1).unwrap(),
            UpNext::Next(eps[3].clone())
        );
        // Everything unwatched: nothing played.
        set_watched(&mut conn, "p", &[season1, season2].concat(), false, 5).unwrap();
        assert_eq!(series_up_next(&conn, "p", 1).unwrap(), UpNext::Nothing);
    }

    /// A movie's info is read with its row until the list is imported
    /// again (VOD ticket 04).
    #[test]
    fn movie_info_lasts_until_the_list_refreshes() {
        let (_dir, mut conn) = with_lists(&REGIONS, &[]);
        let cats = [("1", "EN - Drama")];
        let list = [(20, "Tidewater", "1", None), (21, "Amber Road", "1", None)];
        import_movie_list(&mut conn, &cats, &list);
        let row = |conn: &Connection, id: i64| {
            let keys = title_keys(conn, "p", Kind::Movie, TitleView::All(Order::Provider)).unwrap();
            keys.iter()
                .map(|&k| title(conn, Kind::Movie, k).unwrap().unwrap())
                .find(|r| r.stream_id == id)
                .unwrap()
        };
        assert!(!row(&conn, 20).info);
        let info = MovieInfo {
            year: Some(2019),
            length: Some(5594),
            genre: Some("Drama".into()),
            plot: Some("A harbour town keeps a secret.".into()),
            actors: Some("Ada Marsh, Tom Reyes".into()),
            director: None,
        };
        set_movie_info(&conn, "p", 20, &info, 100).unwrap();
        let r = row(&conn, 20);
        assert!(r.info);
        assert_eq!(
            (r.year, r.length, r.genre.as_deref(), r.director),
            (Some(2019), Some(5594), Some("Drama"), None)
        );
        assert_eq!(r.actors.as_deref(), Some("Ada Marsh, Tom Reyes"));
        assert!(!row(&conn, 21).info, "only the one fetched");
        // A movie the list no longer has is left alone.
        set_movie_info(&conn, "p", 99, &info, 100).unwrap();

        import_movie_list(&mut conn, &cats, &list);
        let r = row(&conn, 20);
        assert!(!r.info);
        assert_eq!((r.year, r.plot), (None, None));
    }

    /// The lists hold keys only and read a title by its key (ADR 0003);
    /// each order is the channels', and a hidden category's titles leave
    /// All and Recently added (VOD ticket 03).
    #[test]
    fn movie_lists_are_keys_in_order_less_hidden_categories() {
        let (_dir, mut conn) = with_lists(&REGIONS, &[]);
        let cats = [("1", "EN - Drama"), ("2", "EN - Comedy")];
        import_movie_list(
            &mut conn,
            &cats,
            &[
                (1, "Tidewater", "1", Some(300)),
                (2, "Amber Road", "2", Some(100)),
                (3, "The Lantern Keeper", "1", None),
                (4, "Brass Harbour", "2", Some(200)),
            ],
        );
        let names = |conn: &Connection, view| -> Vec<String> {
            title_keys(conn, "p", Kind::Movie, view)
                .unwrap()
                .into_iter()
                .map(|k| title(conn, Kind::Movie, k).unwrap().unwrap().name)
                .collect()
        };
        assert_eq!(
            names(&conn, TitleView::All(Order::Provider)),
            [
                "Tidewater",
                "Amber Road",
                "The Lantern Keeper",
                "Brass Harbour"
            ]
        );
        assert_eq!(
            names(&conn, TitleView::All(Order::Az)),
            [
                "Amber Road",
                "Brass Harbour",
                "The Lantern Keeper",
                "Tidewater"
            ]
        );
        assert_eq!(
            names(&conn, TitleView::Added),
            ["Tidewater", "Brass Harbour", "Amber Road"]
        );
        assert_eq!(
            names(&conn, TitleView::Category("1", Order::Provider)),
            ["Tidewater", "The Lantern Keeper"]
        );
        let key = title_keys(&conn, "p", Kind::Movie, TitleView::All(Order::Provider)).unwrap()[0];
        let row = title(&conn, Kind::Movie, key).unwrap().unwrap();
        assert_eq!(row.category.as_deref(), Some("EN - Drama"));
        assert_eq!(row.rating, Some(7.25));
        assert_eq!(row.extension.as_deref(), Some("mkv"));
        assert_eq!(row.poster_url, None, "empty reads as missing");
        assert_eq!(row.added, Some(300));
        assert_eq!(row.year, None);
        // Live rules leave movies alone; a movie override hides its titles.
        toggle_category_rule(&mut conn, "p", Kind::Live, "UK|").unwrap();
        assert_eq!(names(&conn, TitleView::All(Order::Provider)).len(), 4);
        toggle_category(&mut conn, "p", Kind::Movie, "1").unwrap();
        assert_eq!(
            names(&conn, TitleView::All(Order::Provider)),
            ["Amber Road", "Brass Harbour"]
        );
        assert_eq!(
            names(&conn, TitleView::Added),
            ["Brass Harbour", "Amber Road"]
        );
    }

    #[test]
    fn recently_added_keeps_the_latest() {
        let (_dir, mut conn) = with_lists(&REGIONS, &[]);
        let movies: Vec<(i64, String)> = (0..RECENTLY_ADDED as i64 + 5)
            .map(|i| (i, format!("Film {i}")))
            .collect();
        let list: Vec<(i64, &str, &str, Option<i64>)> = movies
            .iter()
            .map(|(i, name)| (*i, name.as_str(), "1", Some(1000 + i)))
            .collect();
        import_movie_list(&mut conn, &[("1", "EN - Drama")], &list);
        let keys = title_keys(&conn, "p", Kind::Movie, TitleView::Added).unwrap();
        assert_eq!(keys.len(), RECENTLY_ADDED);
        let first = title(&conn, Kind::Movie, keys[0]).unwrap().unwrap();
        assert_eq!(first.name, format!("Film {}", RECENTLY_ADDED + 4));
    }

    #[test]
    fn series_rows_carry_their_list_fields() {
        let (_dir, mut conn) = with_lists(&REGIONS, &[]);
        let tx = conn.transaction().unwrap();
        begin_import(&tx, "p", "P", Kind::Series, [("1", "Crime Series")]).unwrap();
        for (id, name, modified) in [(1, "Harbour Lights", 50), (2, "Northwind", 90)] {
            let s = NewSeries {
                series_id: id,
                num: Some(id),
                name,
                category_id: Some("1"),
                year: Some(2019),
                rating: Some(8.1),
                cover_url: None,
                genre: Some("Drama"),
                plot: Some("A lifeboat crew."),
                actors: Some(" "),
                director: None,
                episode_run_time: Some(48),
                last_modified: Some(modified),
            };
            insert_series(&tx, "p", &s).unwrap();
        }
        finish_vod_import(&tx, "p", Kind::Series, 0).unwrap();
        tx.commit().unwrap();
        let keys = title_keys(&conn, "p", Kind::Series, TitleView::Added).unwrap();
        let rows: Vec<TitleRow> = keys
            .iter()
            .map(|&k| title(&conn, Kind::Series, k).unwrap().unwrap())
            .collect();
        assert_eq!(rows[0].name, "Northwind", "latest change first");
        assert_eq!(rows[1].year, Some(2019));
        assert_eq!(rows[1].genre.as_deref(), Some("Drama"));
        assert_eq!(rows[1].plot.as_deref(), Some("A lifeboat crew."));
        assert_eq!(rows[1].actors, None, "blank reads as missing");
        assert_eq!(rows[1].run_time, Some(48));
        assert_eq!(rows[1].category.as_deref(), Some("Crime Series"));
    }

    /// A series' episodes and seasons last until the series list is
    /// imported again (VOD ticket 05).
    #[test]
    fn series_info_lasts_until_the_list_refreshes() {
        let (_dir, mut conn) = with_lists(&REGIONS, &[]);
        let import = |conn: &mut Connection| {
            let tx = conn.transaction().unwrap();
            begin_import(&tx, "p", "P", Kind::Series, [("1", "Crime Series")]).unwrap();
            let s = NewSeries {
                series_id: 1,
                num: Some(1),
                name: "Harbour Lights",
                category_id: Some("1"),
                year: Some(2019),
                rating: None,
                cover_url: None,
                genre: None,
                plot: None,
                actors: None,
                director: None,
                episode_run_time: None,
                last_modified: None,
            };
            insert_series(&tx, "p", &s).unwrap();
            finish_vod_import(&tx, "p", Kind::Series, 0).unwrap();
            tx.commit().unwrap();
        };
        import(&mut conn);
        let row = |conn: &Connection| {
            let keys = title_keys(conn, "p", Kind::Series, TitleView::All(Order::Provider));
            title(conn, Kind::Series, keys.unwrap()[0])
                .unwrap()
                .unwrap()
        };
        assert!(!row(&conn).info);
        let ep = |season, num, stream_id| Episode {
            season,
            num,
            stream_id,
            title: format!("Harbour Lights - S{season:02}E{:02}", num.unwrap_or(0)),
            extension: Some("mkv".into()),
            length: Some(2900),
        };
        // As the provider lists them: season 2 first, numbers out of order.
        let info = SeriesInfo {
            episodes: vec![ep(2, Some(1), 21), ep(1, Some(2), 12), ep(1, Some(1), 11)],
            last_year: Some(2023),
        };
        set_series_info(&mut conn, "p", 1, &info, 100).unwrap();
        let r = row(&conn);
        assert!(r.info);
        assert_eq!((r.seasons, r.last_year), (Some(2), Some(2023)));
        let ids: Vec<i64> = episodes(&conn, "p", 1)
            .unwrap()
            .iter()
            .map(|e| e.stream_id)
            .collect();
        assert_eq!(ids, [11, 12, 21]);
        // Fetched again, it replaces what was there.
        let info = SeriesInfo {
            episodes: vec![ep(1, Some(1), 11)],
            last_year: None,
        };
        set_series_info(&mut conn, "p", 1, &info, 200).unwrap();
        assert_eq!(episodes(&conn, "p", 1).unwrap(), [ep(1, Some(1), 11)]);
        assert_eq!(row(&conn).seasons, Some(1));
        // A series the list no longer has is left alone.
        set_series_info(&mut conn, "p", 99, &info, 100).unwrap();
        assert!(episodes(&conn, "p", 99).unwrap().is_empty());

        import(&mut conn);
        let r = row(&conn);
        assert!(!r.info);
        assert_eq!((r.seasons, r.last_year), (None, None));
        assert!(episodes(&conn, "p", 1).unwrap().is_empty());
    }

    /// Every list order comes straight from an index, with no sort, as
    /// the channels' do (SPEC 8.2): 183k movies sorted per category switch
    /// would not be.
    #[test]
    fn title_lists_are_read_from_an_index() {
        let (_dir, conn) = with_lists(&REGIONS, &[]);
        for kind in [Kind::Movie, Kind::Series] {
            for view in [
                TitleView::All(Order::Provider),
                TitleView::All(Order::Az),
                TitleView::Added,
                TitleView::Category("1", Order::Provider),
                TitleView::Category("1", Order::Az),
            ] {
                let sql = title_query(kind, &view);
                let args = if sql.contains("?2") {
                    &["p", "1"][..]
                } else {
                    &["p"]
                };
                let plan: Vec<String> = conn
                    .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                    .unwrap()
                    .query_map(rusqlite::params_from_iter(args), |r| r.get(3))
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap();
                let plan = plan.join("; ");
                assert!(plan.contains("COVERING INDEX"), "{kind:?}: {plan}");
                assert!(!plan.contains("TEMP B-TREE"), "{kind:?}: {plan}");
            }
        }
    }

    /// Ids of the categories the library shows, in provider order.
    fn shown(conn: &Connection) -> Vec<String> {
        categories(conn, "p", Kind::Live, Order::Provider)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// Ids of the categories shown by an override.
    fn overridden(conn: &Connection) -> Vec<String> {
        managed_categories(conn, "p", Kind::Live, Order::Provider)
            .unwrap()
            .into_iter()
            .filter(|c| c.overridden)
            .map(|c| c.id)
            .collect()
    }

    const REGIONS: [(&str, &str); 4] = [
        ("1", "UK| News"),
        ("2", "uk| sport"),
        ("3", "US| News"),
        ("4", "DE| Kino ᴴᴰ"),
    ];

    #[test]
    fn category_rules_match_names_case_insensitively() {
        let (_dir, mut conn) = with_lists(&REGIONS, &[]);
        assert_eq!(shown(&conn), ["1", "2", "3", "4"], "no rules: everything");
        assert!(toggle_category_rule(&mut conn, "p", Kind::Live, "UK|").unwrap());
        assert_eq!(shown(&conn), ["1", "2"]);
        // Any rule shows a category; superscripts fold.
        assert!(toggle_category_rule(&mut conn, "p", Kind::Live, "kino hd").unwrap());
        assert_eq!(shown(&conn), ["1", "2", "4"]);
        // The same rule typed differently removes it.
        assert!(!toggle_category_rule(&mut conn, "p", Kind::Live, "uk|").unwrap());
        assert_eq!(shown(&conn), ["4"]);
        assert_eq!(category_rules(&conn, "p", Kind::Live).unwrap(), ["kino hd"]);
        assert!(!toggle_category_rule(&mut conn, "p", Kind::Live, "  ").unwrap());
        assert_eq!(category_rules(&conn, "p", Kind::Live).unwrap(), ["kino hd"]);
        // A rule nothing matches shows nothing.
        toggle_category_rule(&mut conn, "p", Kind::Live, "kino hd").unwrap();
        toggle_category_rule(&mut conn, "p", Kind::Live, "FR|").unwrap();
        assert!(shown(&conn).is_empty());
    }

    #[test]
    fn overrides_win_over_rules_both_ways() {
        let (_dir, mut conn) = with_lists(&REGIONS, &[]);
        toggle_category_rule(&mut conn, "p", Kind::Live, "UK|").unwrap();
        assert!(
            !toggle_category(&mut conn, "p", Kind::Live, "2").unwrap(),
            "hide one the rule shows"
        );
        assert!(
            toggle_category(&mut conn, "p", Kind::Live, "4").unwrap(),
            "show one no rule covers"
        );
        assert_eq!(shown(&conn), ["1", "4"]);
        assert_eq!(overridden(&conn), ["2", "4"]);
        // Toggled back, it is the rules' again.
        assert!(toggle_category(&mut conn, "p", Kind::Live, "2").unwrap());
        assert_eq!(overridden(&conn), ["4"]);
        // Overrides outlast rule changes.
        toggle_category_rule(&mut conn, "p", Kind::Live, "UK|").unwrap();
        toggle_category(&mut conn, "p", Kind::Live, "3").unwrap();
        assert_eq!(shown(&conn), ["1", "2", "4"], "no rules, one hidden");
    }

    /// Replaces provider `p`'s movie categories `(id, name)`, with a movie
    /// in each, as a movie import does.
    fn import_movies(conn: &mut Connection, cats: &[(&str, &str)]) {
        let tx = conn.transaction().unwrap();
        begin_import(&tx, "p", "P", Kind::Movie, cats.iter().copied()).unwrap();
        for (i, &(cat, name)) in (0..).zip(cats) {
            let m = NewMovie {
                stream_id: i,
                num: Some(i),
                name,
                category_id: Some(cat),
                rating: None,
                poster_url: None,
                extension: None,
                added: None,
            };
            insert_movie(&tx, "p", &m).unwrap();
        }
        finish_vod_import(&tx, "p", Kind::Movie, 0).unwrap();
        tx.commit().unwrap();
    }

    /// Ids of `kind`'s shown categories, in provider order.
    fn shown_of(conn: &Connection, kind: Kind) -> Vec<String> {
        managed_categories(conn, "p", kind, Order::Provider)
            .unwrap()
            .into_iter()
            .filter(|c| c.shown)
            .map(|c| c.id)
            .collect()
    }

    /// Live rules match no movie category (VOD ticket 01), so each kind has
    /// rules and overrides of its own; ids repeat across kinds.
    #[test]
    fn each_kind_has_its_own_rules_and_overrides() {
        let (_dir, mut conn) = with_lists(&REGIONS, &[]);
        let movies = [("1", "EN - Drama"), ("2", "EN - Comedy"), ("3", "Kino")];
        import_movies(&mut conn, &movies);
        toggle_category_rule(&mut conn, "p", Kind::Live, "UK|").unwrap();
        assert_eq!(shown(&conn), ["1", "2"]);
        assert_eq!(
            shown_of(&conn, Kind::Movie),
            ["1", "2", "3"],
            "no movie rules: every movie category"
        );

        assert!(toggle_category_rule(&mut conn, "p", Kind::Movie, "en - ").unwrap());
        assert_eq!(shown_of(&conn, Kind::Movie), ["1", "2"]);
        assert_eq!(shown(&conn), ["1", "2"], "live is the live rules'");
        assert!(!toggle_category(&mut conn, "p", Kind::Movie, "2").unwrap());
        assert_eq!(shown_of(&conn, Kind::Movie), ["1"]);
        assert_eq!(shown(&conn), ["1", "2"], "live 2 is another category");
        assert_eq!(shown_title_count(&conn, "p", Kind::Movie).unwrap(), 1);
        assert_eq!(category_rules(&conn, "p", Kind::Movie).unwrap(), ["en - "]);
        assert!(category_rules(&conn, "p", Kind::Series).unwrap().is_empty());

        // Each import replaces its own kind's categories only, and keeps
        // the rules and overrides.
        import_lists(&mut conn, &REGIONS, &[]);
        assert_eq!(shown_of(&conn, Kind::Movie), ["1"]);
        import_movies(&mut conn, &movies);
        assert_eq!(shown_of(&conn, Kind::Movie), ["1"]);
        assert_eq!(shown(&conn), ["1", "2"]);
        // A movie override follows its category to a new id, as live ones do.
        import_movies(&mut conn, &[("1", "EN - Drama"), ("20", "EN - Comedy")]);
        assert_eq!(shown_of(&conn, Kind::Movie), ["1"]);
    }

    #[test]
    fn a_new_category_matching_a_rule_shows_after_an_import() {
        let (_dir, mut conn) = with_lists(&REGIONS, &[]);
        toggle_category_rule(&mut conn, "p", Kind::Live, "UK|").unwrap();
        let mut cats = REGIONS.to_vec();
        cats.extend([("5", "UK| Film"), ("6", "FR| Film")]);
        import_lists(&mut conn, &cats, &[]);
        assert_eq!(shown(&conn), ["1", "2", "5"]);
    }

    #[test]
    fn rules_and_overrides_survive_restart_and_an_id_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.db");
        {
            let mut conn = open(&path).unwrap();
            import_lists(&mut conn, &REGIONS, &[]);
            toggle_category_rule(&mut conn, "p", Kind::Live, "News").unwrap();
            toggle_category(&mut conn, "p", Kind::Live, "3").unwrap();
            toggle_category(&mut conn, "p", Kind::Live, "4").unwrap();
        }
        let mut conn = open(&path).unwrap();
        assert_eq!(shown(&conn), ["1", "4"]);
        // The provider renumbers US| News and DE| Kino; UK| News goes.
        import_lists(
            &mut conn,
            &[
                ("2", "uk| sport"),
                ("30", "US| News"),
                ("40", "DE| KINO HD"),
            ],
            &[],
        );
        assert_eq!(shown(&conn), ["40"]);
        assert_eq!(overridden(&conn), ["30", "40"]);
        // An override whose category has gone for good is kept, unmatched.
        import_lists(&mut conn, &[("2", "uk| sport")], &[]);
        assert!(shown(&conn).is_empty());
        import_lists(&mut conn, &[("2", "uk| sport"), ("40", "DE| Kino")], &[]);
        assert_eq!(shown(&conn), ["40"]);
    }

    #[test]
    fn hidden_categories_leave_all_but_not_favourites_or_recent() {
        let (_dir, mut conn) = with_lists(
            &[("a", "UK| News"), ("b", "US| News")],
            &[
                (10, 1, "Harbour One", "a"),
                (11, 2, "Summit News", "b"),
                (12, 3, "Kestrel", "a"),
                // Not in a category the provider lists.
                (13, 4, "Vanta", "zz"),
            ],
        );
        toggle_favourite(&conn, "p", Kind::Live, 11).unwrap();
        record_play(&conn, "p", Kind::Live, 11, 100).unwrap();
        toggle_category_rule(&mut conn, "p", Kind::Live, "UK|").unwrap();
        assert_eq!(ids(&conn, View::All(Order::Provider)), [10, 12, 13]);
        assert_eq!(ids(&conn, View::All(Order::Az)), [10, 12, 13]);
        assert_eq!(shown_channel_count(&conn, "p").unwrap(), 3);
        assert_eq!(ids(&conn, View::Favourites), [11]);
        assert_eq!(ids(&conn, View::Recent), [11]);
        let counts: Vec<i64> = managed_categories(&conn, "p", Kind::Live, Order::Provider)
            .unwrap()
            .iter()
            .map(|c| c.titles)
            .collect();
        assert_eq!(counts, [2, 1]);
    }

    #[cfg(feature = "devtools")]
    #[test]
    fn alternates_are_other_shown_channels_with_the_same_looser_name() {
        let (_dir, mut conn) = with_lists(
            &[("a", "UK| News"), ("b", "US| News")],
            &[
                (10, 1, "Harbour One", "a"),
                (11, 2, "Harbour One HD", "a"),
                (12, 3, "HARBOUR ONE FHD", "b"),
                (13, 4, "Harbour One +1", "a"),
                (14, 5, "Kestrel", "a"),
                (15, 6, "Kestrel HD", "zz"),
                (16, 7, "FR: Kestrel", "a"),
            ],
        );
        toggle_favourite(&conn, "p", Kind::Live, 10).unwrap();
        toggle_favourite(&conn, "p", Kind::Live, 13).unwrap();
        record_play(&conn, "p", Kind::Live, 14, 100).unwrap();
        let spread = |conn: &Connection, of| alternate_spread(conn, "p", of).unwrap();
        // `FR: Kestrel` is one name with `Kestrel` once its tag is dropped.
        assert_eq!(spread(&conn, AlternatesOf::Shown), [1, 0, 6, 0]);
        assert_eq!(spread(&conn, AlternatesOf::Favourites), [1, 0, 1, 0]);
        assert_eq!(spread(&conn, AlternatesOf::Recent), [0, 0, 1, 0]);
        // The US category hidden: its channel is no alternate, nor counted.
        toggle_category_rule(&mut conn, "p", Kind::Live, "UK|").unwrap();
        assert_eq!(spread(&conn, AlternatesOf::Shown), [1, 2, 3, 0]);
        assert_eq!(spread(&conn, AlternatesOf::Favourites), [1, 1, 0, 0]);
        assert_eq!(spread(&conn, AlternatesOf::Recent), [0, 0, 1, 0]);
        toggle_channel_rule(&mut conn, "p", "FR:").unwrap();
        assert_eq!(spread(&conn, AlternatesOf::Shown), [1, 4, 0, 0]);
    }

    #[test]
    fn channel_rules_hide_at_the_start_of_a_word() {
        for name in ["FR: x", "x | FR: y", "BE/FR: z", "fr:", "ᶠᴿ: x", "x FR:"] {
            assert!(hides(&fold(name), &fold("FR:")), "{name}");
        }
        for name in ["AFR: x", "x2FR: y", "FR x", "ÉFR: x", ""] {
            assert!(!hides(&fold(name), &fold("FR:")), "{name}");
        }
        // A later occurrence counts when an earlier one is mid-word.
        assert!(hides("xa-a-a", "a-a"));
    }

    const NAMED: [(i64, i64, &str, &str); 5] = [
        (10, 1, "FR: Nova", "a"),
        (11, 2, "Harbour One", "a"),
        (12, 3, "Kestrel | fr: Sport", "b"),
        (13, 4, "AFR: Savanna", "b"),
        (14, 5, "BE/FR: Lumen", "a"),
    ];

    #[test]
    fn channel_rules_hide_channels_from_lists_but_not_favourites_or_recent() {
        let (_dir, mut conn) = with_lists(&[("a", "UK| News"), ("b", "US| News")], &NAMED);
        toggle_favourite(&conn, "p", Kind::Live, 10).unwrap();
        record_play(&conn, "p", Kind::Live, 12, 100).unwrap();
        assert!(!any_hidden(&conn, "p").unwrap());
        assert!(toggle_channel_rule(&mut conn, "p", "fr:").unwrap());
        assert!(any_hidden(&conn, "p").unwrap());
        assert_eq!(ids(&conn, View::All(Order::Provider)), [11, 13]);
        assert_eq!(ids(&conn, View::All(Order::Az)), [13, 11]);
        assert_eq!(ids(&conn, View::Category("a", Order::Provider)), [11]);
        assert_eq!(ids(&conn, View::Category("b", Order::Az)), [13]);
        assert_eq!(shown_channel_count(&conn, "p").unwrap(), 2);
        assert_eq!(ids(&conn, View::Favourites), [10]);
        assert_eq!(ids(&conn, View::Recent), [12]);
        let counts: Vec<(i64, i64)> = managed_categories(&conn, "p", Kind::Live, Order::Provider)
            .unwrap()
            .iter()
            .map(|c| (c.titles, c.hidden))
            .collect();
        assert_eq!(counts, [(3, 2), (2, 1)]);
        // The same rule typed differently removes it.
        assert!(!toggle_channel_rule(&mut conn, "p", "FR:").unwrap());
        assert!(channel_rules(&conn, "p").unwrap().is_empty());
        assert_eq!(ids(&conn, View::All(Order::Provider)), [10, 11, 12, 13, 14]);
        assert!(!any_hidden(&conn, "p").unwrap());
    }

    #[test]
    fn a_new_channel_matching_a_rule_is_hidden_after_an_import() {
        let (_dir, mut conn) = with_lists(&[("a", "UK| News"), ("b", "US| News")], &NAMED);
        toggle_channel_rule(&mut conn, "p", "fr:").unwrap();
        let mut chans = NAMED.to_vec();
        chans.extend([(15, 6, "FR: Ciel", "a"), (16, 7, "Vanta", "a")]);
        import_lists(&mut conn, &[("a", "UK| News"), ("b", "US| News")], &chans);
        assert_eq!(ids(&conn, View::All(Order::Provider)), [11, 13, 16]);
    }

    #[test]
    fn channel_rules_survive_restart_and_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.db");
        let cats = [("a", "UK| News"), ("b", "US| News")];
        {
            let mut conn = open(&path).unwrap();
            import_lists(&mut conn, &cats, &NAMED);
            toggle_channel_rule(&mut conn, "p", "fr:").unwrap();
        }
        let mut conn = open(&path).unwrap();
        assert_eq!(channel_rules(&conn, "p").unwrap(), ["fr:"]);
        assert_eq!(ids(&conn, View::All(Order::Provider)), [11, 13]);
        import_lists(&mut conn, &cats, &NAMED);
        assert_eq!(ids(&conn, View::All(Order::Provider)), [11, 13]);
    }

    #[test]
    fn region_tags_are_two_or_three_letters_at_the_start() {
        for (name, tag) in [
            ("FR: x", "fr"),
            (" fr | x", "fr"),
            ("BE/FR: x", "be/fr"),
            ("UK| News", "uk"),
            ("ABC: x", "abc"),
            ("ᶠᴿ: x", "fr"),
        ] {
            assert_eq!(region_tag(&fold(name)), Some(tag), "{name}");
        }
        for name in [
            "4K: x",
            "24/7: x",
            "x | FR: y",
            "Harbour One",
            "F: x",
            "ABCD: x",
            "FR x",
            "BE/: x",
            "",
        ] {
            assert_eq!(region_tag(&fold(name)), None, "{name}");
        }
        for input in ["uk", "UK:", "uk|", " Uk | "] {
            assert_eq!(kept_tag(input).as_deref(), Some("uk"), "{input}");
        }
        for input in ["", "u", "ukne", "4k", "be/fr", "u k"] {
            assert_eq!(kept_tag(input), None, "{input}");
        }
    }

    const TAGGED: [(i64, i64, &str, &str); 5] = [
        (10, 1, "FR: Nova", "a"),
        (11, 2, "Harbour One", "a"),
        (12, 3, "UK: Kestrel", "b"),
        (13, 4, "BE/FR: Lumen", "b"),
        (14, 5, "UK| Summit", "a"),
    ];

    #[test]
    fn kept_tags_hide_other_regions_but_not_untagged_channels() {
        let (_dir, mut conn) = with_lists(&[("a", "8K| UHD"), ("b", "4K| UHD")], &TAGGED);
        toggle_favourite(&conn, "p", Kind::Live, 10).unwrap();
        record_play(&conn, "p", Kind::Live, 13, 100).unwrap();
        assert!(toggle_kept_tag(&mut conn, "p", "UK:").unwrap());
        assert_eq!(ids(&conn, View::All(Order::Provider)), [11, 12, 14]);
        assert_eq!(ids(&conn, View::Category("a", Order::Az)), [11, 14]);
        assert_eq!(ids(&conn, View::Favourites), [10]);
        assert_eq!(ids(&conn, View::Recent), [13]);
        assert!(any_hidden(&conn, "p").unwrap());
        // Either of a joined tag's regions keeps it.
        assert!(toggle_kept_tag(&mut conn, "p", "be").unwrap());
        assert_eq!(ids(&conn, View::All(Order::Provider)), [11, 12, 13, 14]);
        assert!(!toggle_kept_tag(&mut conn, "p", "be").unwrap());
        assert!(toggle_kept_tag(&mut conn, "p", "fr").unwrap());
        assert_eq!(ids(&conn, View::All(Order::Provider)), [10, 11, 12, 13, 14]);
        // Typed differently, the same entry.
        assert!(!toggle_kept_tag(&mut conn, "p", "fr|").unwrap());
        assert_eq!(kept_tags(&conn, "p").unwrap(), ["uk"]);
        // Not a tag: nothing changes.
        assert!(!toggle_kept_tag(&mut conn, "p", "summit").unwrap());
        assert_eq!(kept_tags(&conn, "p").unwrap(), ["uk"]);
        // Channel rules hide on top.
        toggle_channel_rule(&mut conn, "p", "summit").unwrap();
        assert_eq!(ids(&conn, View::All(Order::Provider)), [11, 12]);
        assert!(!toggle_kept_tag(&mut conn, "p", "uk").unwrap());
        assert_eq!(ids(&conn, View::All(Order::Provider)), [10, 11, 12, 13]);
    }

    #[test]
    fn kept_tags_survive_restart_and_hide_new_channels_after_an_import() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.db");
        let cats = [("a", "8K| UHD"), ("b", "4K| UHD")];
        {
            let mut conn = open(&path).unwrap();
            import_lists(&mut conn, &cats, &TAGGED);
            toggle_kept_tag(&mut conn, "p", "uk").unwrap();
        }
        let mut conn = open(&path).unwrap();
        assert_eq!(kept_tags(&conn, "p").unwrap(), ["uk"]);
        assert_eq!(ids(&conn, View::All(Order::Provider)), [11, 12, 14]);
        let mut chans = TAGGED.to_vec();
        chans.extend([(15, 6, "DE: Granite", "a"), (16, 7, "UK: Ember", "b")]);
        import_lists(&mut conn, &cats, &chans);
        assert_eq!(ids(&conn, View::All(Order::Provider)), [11, 12, 14, 16]);
    }

    /// Provider `p` with channels `(stream_id, category_id, epg_channel_id)`,
    /// numbered in this order, and programmes `(epg_channel_id, start, stop)`.
    fn with_guide(
        chans: &[(i64, &str, Option<&str>)],
        progs: &[(&str, i64, i64)],
    ) -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open(&dir.path().join("v.db")).unwrap();
        let tx = conn.transaction().unwrap();
        begin_channel_import(&tx, "p", "P", [("a", "UK| News"), ("b", "US| News")]).unwrap();
        for (num, &(stream_id, cat, epg)) in (1..).zip(chans) {
            let name = format!("Channel {stream_id}");
            let c = NewChannel {
                stream_id,
                num: Some(num),
                name: &name,
                category_id: Some(cat),
                epg_channel_id: epg,
                icon_url: None,
                extension: "ts",
                archive_days: 0,
            };
            insert_channel(&tx, "p", &c).unwrap();
        }
        finish_channel_import(&tx, "p", 0).unwrap();
        begin_epg_import(&tx, "p", 0).unwrap();
        for &(epg, start, stop) in progs {
            let title = format!("{epg} at {start}");
            let p = NewProgramme {
                epg_channel_id: epg,
                start,
                stop,
                title: &title,
                description: None,
            };
            insert_programme(&tx, "p", &p).unwrap();
        }
        // Another provider's guide for the same id is not this one's.
        insert_programme(
            &tx,
            "q",
            &NewProgramme {
                epg_channel_id: "e11",
                start: 0,
                stop: 100,
                title: "Other",
                description: None,
            },
        )
        .unwrap();
        resolve_guides(&tx, "p").unwrap();
        tx.commit().unwrap();
        (dir, conn)
    }

    fn guide_ids(conn: &Connection, view: View) -> Vec<i64> {
        guide_keys(conn, "p", view)
            .unwrap()
            .into_iter()
            .map(|k| channel(conn, k, 0).unwrap().unwrap().stream_id)
            .collect()
    }

    #[test]
    fn guide_keys_are_the_views_channels_with_programmes() {
        let (_dir, mut conn) = with_guide(
            &[
                (10, "a", Some("e10")),
                // An id the guide does not have.
                (11, "a", Some("e11")),
                (12, "a", None),
                (13, "b", Some("e13")),
                (14, "a", Some("e14")),
                // Shares a guide id with 10.
                (15, "b", Some("e10")),
            ],
            &[
                ("e10", 0, 100),
                ("e10", 100, 200),
                ("e13", 0, 100),
                ("e14", 50, 80),
            ],
        );
        assert_eq!(
            guide_ids(&conn, View::All(Order::Provider)),
            [10, 13, 14, 15]
        );
        assert_eq!(guide_ids(&conn, View::All(Order::Az)), [10, 13, 14, 15]);
        assert_eq!(
            guide_ids(&conn, View::Category("a", Order::Provider)),
            [10, 14]
        );
        assert_eq!(guide_ids(&conn, View::Category("b", Order::Az)), [13, 15]);
        assert_eq!(guide_ids(&conn, View::Favourites), Vec::<i64>::new());

        for id in [13, 11, 10, 12] {
            toggle_favourite(&conn, "p", Kind::Live, id).unwrap();
            record_play(&conn, "p", Kind::Live, id, id).unwrap();
        }
        // Favourites in their order, recently played latest first.
        assert_eq!(guide_ids(&conn, View::Favourites), [13, 10]);
        assert_eq!(guide_ids(&conn, View::Recent), [13, 10]);

        // Hidden categories leave All, not Favourites or Recently played.
        toggle_category_rule(&mut conn, "p", Kind::Live, "UK|").unwrap();
        assert_eq!(guide_ids(&conn, View::All(Order::Provider)), [10, 14]);
        assert_eq!(guide_ids(&conn, View::Favourites), [13, 10]);
        assert_eq!(guide_ids(&conn, View::Recent), [13, 10]);

        // So do channels a channel rule hides.
        toggle_channel_rule(&mut conn, "p", "Channel 14").unwrap();
        assert_eq!(guide_ids(&conn, View::All(Order::Provider)), [10]);
        assert_eq!(guide_ids(&conn, View::Category("a", Order::Az)), [10]);
        toggle_channel_rule(&mut conn, "p", "Channel 10").unwrap();
        assert_eq!(guide_ids(&conn, View::Favourites), [13, 10]);
        toggle_channel_rule(&mut conn, "p", "Channel 14").unwrap();
        toggle_channel_rule(&mut conn, "p", "Channel 10").unwrap();

        // And channels kept tags hide.
        conn.execute(
            "UPDATE channel SET name = 'FR: Channel 14' WHERE stream_id = 14",
            [],
        )
        .unwrap();
        toggle_kept_tag(&mut conn, "p", "uk").unwrap();
        assert_eq!(guide_ids(&conn, View::All(Order::Provider)), [10]);
        toggle_kept_tag(&mut conn, "p", "uk").unwrap();

        // An unavailable favourite has no guide.
        import_lists(
            &mut conn,
            &[("a", "UK| News")],
            &[(14, 1, "Channel 14", "a")],
        );
        assert_eq!(guide_ids(&conn, View::Favourites), Vec::<i64>::new());
        assert_eq!(channel_keys(&conn, "p", View::Favourites).unwrap().len(), 4);
    }

    #[test]
    fn programmes_are_those_airing_in_the_span() {
        let (_dir, conn) = with_guide(
            &[(10, "a", Some("e10")), (11, "a", Some("e11"))],
            &[
                ("e10", 0, 100),
                ("e10", 100, 250),
                ("e10", 250, 400),
                ("e10", 400, 500),
                ("e11", 120, 300),
            ],
        );
        let spans = |from, to| -> Vec<(i64, i64)> {
            programmes(&conn, "p", "e10", from, to)
                .unwrap()
                .iter()
                .map(|p| (p.start, p.stop))
                .collect()
        };
        assert_eq!(spans(120, 300), [(100, 250), (250, 400)]);
        // Ends are exclusive: one ending at `from` or starting at `to` is out.
        assert_eq!(spans(250, 400), [(250, 400)]);
        assert_eq!(spans(0, 1000).len(), 4);
        assert_eq!(spans(500, 600), []);
        let p = &programmes(&conn, "p", "e10", 120, 130).unwrap()[0];
        assert_eq!(p.title, "e10 at 100");
        assert!(programmes(&conn, "p", "e12", 0, 1000).unwrap().is_empty());
    }

    #[test]
    fn description_is_the_programmes_at_its_start() {
        let (_dir, conn) = with_guide(&[(10, "a", Some("e10"))], &[("e10", 0, 100)]);
        let p = NewProgramme {
            epg_channel_id: "e10",
            start: 100,
            stop: 200,
            title: "Described",
            description: Some("What happens."),
        };
        insert_programme(&conn, "p", &p).unwrap();
        assert_eq!(
            description(&conn, "p", "e10", 100).unwrap().as_deref(),
            Some("What happens.")
        );
        assert_eq!(description(&conn, "p", "e10", 0).unwrap(), None);
        assert_eq!(description(&conn, "p", "e10", 50).unwrap(), None);
        assert_eq!(description(&conn, "q", "e10", 100).unwrap(), None);
    }

    #[test]
    fn order_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.db");
        {
            let conn = open(&path).unwrap();
            assert_eq!(order(&conn).unwrap(), Order::Provider);
            set_order(&conn, Order::Az).unwrap();
        }
        let conn = open(&path).unwrap();
        assert_eq!(order(&conn).unwrap(), Order::Az);
        set_order(&conn, Order::Provider).unwrap();
        assert_eq!(order(&conn).unwrap(), Order::Provider);
    }

    /// Replaces provider `p`'s guide with `(channel, start, stop, title)`s.
    fn import_guide(conn: &mut Connection, progs: &[(&str, i64, i64, &str)]) {
        let tx = conn.transaction().unwrap();
        begin_epg_import(&tx, "p", 0).unwrap();
        for &(epg_channel_id, start, stop, title) in progs {
            let p = NewProgramme {
                epg_channel_id,
                start,
                stop,
                title,
                description: None,
            };
            insert_programme(&tx, "p", &p).unwrap();
        }
        finish_epg_import(&tx, "p", 0).unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn a_guide_reimport_moves_unstarted_schedules_with_their_programmes() {
        let (_dir, mut conn) = with_channels(&[1]);
        let h = 3600;
        let schedule = |title: &str, start: i64| {
            let r = NewRecording {
                provider_id: "p".into(),
                stream_id: 1,
                channel: "Channel 1".into(),
                guide_id: Some("c1".into()),
                title: Some(title.into()),
                start,
                stop: start + h,
            };
            schedule_recording(&conn, &r, 120, 300).unwrap()
        };
        let moved = schedule("Football Tonight", 20 * h);
        let gone = schedule("Harbour News", 10 * h);
        let started = schedule("Coast Weather", 12 * h);
        let far = schedule("Late Film", 30 * h);
        recording_started(&conn, started, "/x.ts").unwrap();
        import_guide(
            &mut conn,
            &[
                // Half an hour later, and longer.
                ("c1", 20 * h + 1800, 21 * h + 2700, "Football Tonight"),
                // The same title the next day: further, not taken.
                ("c1", 44 * h, 45 * h, "Football Tonight"),
                // Same time, other channel or title: not a match.
                ("c2", 10 * h, 11 * h, "Harbour News"),
                ("c1", 10 * h, 11 * h, "Harbour News Extra"),
                ("c1", 13 * h, 14 * h, "Coast Weather"),
                // More than 3 hours off: another showing.
                ("c1", 34 * h, 35 * h, "Late Film"),
            ],
        );
        let times = |id| {
            let r = recording(&conn, id).unwrap().unwrap();
            (r.start, r.stop, r.padded_start, r.padded_stop)
        };
        assert_eq!(
            times(moved),
            (20 * h + 1800, 21 * h + 2700, 20 * h + 1680, 21 * h + 3000),
            "the padding moves with it"
        );
        assert_eq!(times(gone), (10 * h, 11 * h, 10 * h - 120, 11 * h + 300));
        assert_eq!(times(started), (12 * h, 13 * h, 12 * h - 120, 13 * h + 300));
        assert_eq!(times(far), (30 * h, 31 * h, 30 * h - 120, 31 * h + 300));
    }

    /// Migration 17 keeps the live categories, rules and overrides, and
    /// their rules' order, as live.
    #[test]
    fn migration_17_gives_categories_a_kind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.db");
        {
            let conn = Connection::open(&path).unwrap();
            for sql in &MIGRATIONS[..16] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 16).unwrap();
            conn.execute_batch(
                "INSERT INTO category VALUES ('p', '1', 'UK| News', 0, 1), ('p', '2', 'US| News', 1, 0);
                 INSERT INTO category_rule VALUES ('p', 'uk|'), ('p', 'de|');
                 INSERT INTO category_override VALUES ('p', '2', 'US| News', 0);",
            )
            .unwrap();
        }
        let conn = open(&path).unwrap();
        let cats: Vec<(String, String, bool)> = conn
            .prepare("SELECT kind, category_id, shown FROM category ORDER BY sort")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            cats,
            [
                ("live".into(), "1".into(), true),
                ("live".into(), "2".into(), false)
            ]
        );
        assert_eq!(
            category_rules(&conn, "p", Kind::Live).unwrap(),
            ["uk|", "de|"]
        );
        assert_eq!(overridden(&conn), ["2"]);
    }

    #[test]
    fn migrations_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.db");
        drop(open(&path).unwrap());
        let conn = open(&path).unwrap();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    /// Replaces provider `p`'s series with `(series_id, name)`s.
    fn import_series_list(conn: &mut Connection, list: &[(i64, &str)]) {
        let tx = conn.transaction().unwrap();
        begin_import(&tx, "p", "P", Kind::Series, [("1", "Crime Series")]).unwrap();
        for &(series_id, name) in list {
            let s = NewSeries {
                series_id,
                num: Some(series_id),
                name,
                category_id: Some("1"),
                year: None,
                rating: None,
                cover_url: None,
                genre: None,
                plot: None,
                actors: None,
                director: None,
                episode_run_time: None,
                last_modified: None,
            };
            insert_series(&tx, "p", &s).unwrap();
        }
        finish_vod_import(&tx, "p", Kind::Series, 0).unwrap();
        tx.commit().unwrap();
    }

    /// A list of several kinds as `(kind, stream id or series id, name)`,
    /// read through its keys; unavailable favourites as `(Live, id, name)`.
    fn mixed(conn: &Connection, view: View) -> Vec<(Kind, i64, String)> {
        channel_keys(conn, "p", view)
            .unwrap()
            .into_iter()
            .map(|k| match from_list_key(k) {
                (Kind::Live, key) => {
                    let c = channel(conn, key, 0).unwrap().unwrap();
                    (c.kind, c.stream_id, c.name)
                }
                (kind, key) => {
                    let t = title(conn, kind, key).unwrap().unwrap();
                    (kind, t.stream_id, t.name)
                }
            })
            .collect()
    }

    /// A database whose channel 1, movie 1 and series 1 share an id, as
    /// the provider's do (VOD ticket 01).
    fn with_colliding_ids() -> (tempfile::TempDir, Connection) {
        let (dir, mut conn) = with_channels(&[1, 2]);
        import_movie_list(
            &mut conn,
            &[("1", "EN - Drama")],
            &[(1, "Quiet Shore", "1", None), (2, "Far Fields", "1", None)],
        );
        import_series_list(&mut conn, &[(1, "Harbour Lights")]);
        (dir, conn)
    }

    /// VOD ticket 11: series ids collide with live and movie ids, so the
    /// kind keeps them apart in favourites and recently played.
    #[test]
    fn ids_that_collide_across_kinds_stay_apart() {
        let (_dir, mut conn) = with_colliding_ids();
        for kind in [Kind::Series, Kind::Live, Kind::Movie] {
            assert!(toggle_favourite(&conn, "p", kind, 1).unwrap());
        }
        let favs = || mixed(&conn, View::Favourites);
        assert_eq!(
            favs(),
            [
                (Kind::Series, 1, "Harbour Lights".into()),
                (Kind::Live, 1, "Channel 1".into()),
                (Kind::Movie, 1, "Quiet Shore".into()),
            ]
        );
        let starred = |kind| {
            let key = title_keys(&conn, "p", kind, TitleView::All(Order::Provider)).unwrap()[0];
            title(&conn, kind, key).unwrap().unwrap().favourite
        };
        assert!(starred(Kind::Movie) && starred(Kind::Series));
        assert!(
            !channel(&conn, channel_key(&conn, "p", 2).unwrap().unwrap(), 0)
                .unwrap()
                .unwrap()
                .favourite
        );
        // Removing the channel leaves the movie and series of the same id.
        assert!(!toggle_favourite(&conn, "p", Kind::Live, 1).unwrap());
        assert_eq!(favs().len(), 2);
        // J and K move across the kinds.
        assert!(move_favourite(&mut conn, "p", Kind::Movie, 1, -1).unwrap());
        let kinds: Vec<Kind> = mixed(&conn, View::Favourites).iter().map(|f| f.0).collect();
        assert_eq!(kinds, [Kind::Movie, Kind::Series]);

        record_play(&conn, "p", Kind::Live, 1, 100).unwrap();
        record_play(&conn, "p", Kind::Movie, 1, 200).unwrap();
        record_play(&conn, "p", Kind::Series, 1, 300).unwrap();
        let recent: Vec<(Kind, i64)> = mixed(&conn, View::Recent)
            .into_iter()
            .map(|(k, id, _)| (k, id))
            .collect();
        assert_eq!(
            recent,
            [(Kind::Series, 1), (Kind::Movie, 1), (Kind::Live, 1)]
        );
        // The panel's RECENTLY PLAYED and `-` are channels only.
        assert_eq!(recent_ids(&conn, 4), [1]);
        assert_eq!(last_id(&conn, None), Some(1));
        // The grid has only channels with a guide; movies and series none.
        assert_eq!(
            guide_keys(&conn, "p", View::Recent).unwrap(),
            Vec::<i64>::new()
        );
    }

    /// The name fallback (SPEC 5) runs per kind: a movie import re-points
    /// movie favourites only, by movie names only.
    #[test]
    fn the_name_fallback_works_per_kind() {
        let (_dir, mut conn) = with_colliding_ids();
        toggle_favourite(&conn, "p", Kind::Movie, 1).unwrap();
        toggle_favourite(&conn, "p", Kind::Series, 1).unwrap();
        toggle_favourite(&conn, "p", Kind::Live, 2).unwrap();
        record_play(&conn, "p", Kind::Movie, 2, 100).unwrap();
        record_play(&conn, "p", Kind::Series, 1, 200).unwrap();
        // Channel 2 goes: its favourite is kept, unavailable.
        import_channels(&mut conn, &[1]);
        // New movie ids, one named as the channel gone was.
        import_movie_list(
            &mut conn,
            &[("1", "EN - Drama")],
            &[(5, "Quiet Shore", "1", None), (6, "Channel 2", "1", None)],
        );
        assert_eq!(
            mixed(&conn, View::Favourites),
            [
                (Kind::Movie, 5, "Quiet Shore".into()),
                (Kind::Series, 1, "Harbour Lights".into()),
                (Kind::Live, 2, "Channel 2".into()),
            ],
            "the movie followed its name; the series and channel were left alone"
        );
        // The movie played is gone; the series stays.
        let recent: Vec<Kind> = mixed(&conn, View::Recent).iter().map(|r| r.0).collect();
        assert_eq!(recent, [Kind::Series]);

        // A series gone, with no name to follow: kept, unavailable, as
        // a series.
        import_series_list(&mut conn, &[(9, "Tin Roof")]);
        let favs = mixed(&conn, View::Favourites);
        assert_eq!(favs[1], (Kind::Series, 1, "Harbour Lights".into()));
        assert_eq!(favs[2].0, Kind::Live, "the channel is still a channel");
        let keys = channel_keys(&conn, "p", View::Favourites).unwrap();
        assert!(keys[1] < 0);
        assert!(!channel(&conn, keys[1], 0).unwrap().unwrap().available);
        assert_eq!(mixed(&conn, View::Recent), []);
        // Back under a new id: re-pointed by its name.
        import_series_list(&mut conn, &[(9, "Tin Roof"), (12, "Harbour Lights HD")]);
        assert_eq!(
            mixed(&conn, View::Favourites)[1],
            (Kind::Series, 12, "Harbour Lights HD".into())
        );
    }

    /// Migration 22 keeps the favourites and history from before it, as
    /// live, in their order.
    #[test]
    fn the_kinds_migration_keeps_favourites_and_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.db");
        {
            let conn = Connection::open(&path).unwrap();
            for (i, sql) in (0..21).zip(MIGRATIONS) {
                conn.execute_batch(sql).unwrap();
                conn.pragma_update(None, "user_version", i + 1).unwrap();
            }
            conn.execute_batch(
                "INSERT INTO favourite (provider_id, stream_id, name_norm, group_id, position, name)
                     VALUES ('p', 2, 'channel 2', NULL, 1, 'Channel 2'),
                            ('p', 1, 'channel 1', NULL, 2, 'Channel 1');
                 INSERT INTO history VALUES ('p', 1, 100), ('p', 2, 100);",
            )
            .unwrap();
        }
        let mut conn = open(&path).unwrap();
        import_channels(&mut conn, &[1, 2]);
        let favs: Vec<(Kind, i64)> = mixed(&conn, View::Favourites)
            .into_iter()
            .map(|(k, id, _)| (k, id))
            .collect();
        assert_eq!(favs, [(Kind::Live, 2), (Kind::Live, 1)]);
        // Played in the same second: the later row still comes first.
        assert_eq!(recent_ids(&conn, 4), [2, 1]);
    }

    #[test]
    fn downloads_are_queued_once_and_failed_ones_again() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("v.db")).unwrap();
        let film = |id: i64| NewDownload {
            provider_id: "p".into(),
            stream_id: id,
            series_id: None,
            season: None,
            episode_num: None,
            title: format!("Film {id}"),
            episode: None,
            extension: "mkv".into(),
        };
        let q = |added, already| Queued { added, already };
        assert_eq!(
            queue_downloads(&conn, &[film(1), film(2)], 10).unwrap(),
            q(2, 0)
        );
        assert_eq!(
            queue_downloads(&conn, &[film(1), film(3)], 20).unwrap(),
            q(1, 1)
        );
        // An episode with a movie's id is another title.
        let episode = NewDownload {
            series_id: Some(9),
            season: Some(1),
            episode_num: Some(1),
            ..film(1)
        };
        assert_eq!(queue_downloads(&conn, &[episode], 20).unwrap(), q(1, 0));
        let ids: Vec<i64> = downloads(&conn)
            .unwrap()
            .iter()
            .map(|d| d.stream_id)
            .collect();
        assert_eq!(ids, [1, 2, 3, 1]);
        assert_eq!(next_download(&conn, "p").unwrap().unwrap().stream_id, 1);

        // A failed one goes to the back of the queue again.
        let one = download_of(&conn, "p", false, 1).unwrap().unwrap();
        set_download_state(&conn, one.id, DlState::Failed, Some("cut"), None, 30).unwrap();
        assert_eq!(queue_downloads(&conn, &[film(1)], 40).unwrap(), q(1, 0));
        let one = download(&conn, one.id).unwrap().unwrap();
        assert_eq!(
            (one.state, one.note, one.added),
            (DlState::Queued, None, 40)
        );
        assert_eq!(next_download(&conn, "p").unwrap().unwrap().stream_id, 2);

        // A done one counts while its file is there, and is forgotten once
        // it is gone.
        let file = dir.path().join("Film 2.mkv");
        std::fs::write(&file, b"x").unwrap();
        let two = download_of(&conn, "p", false, 2).unwrap().unwrap();
        let path = file.display().to_string();
        set_download_state(&conn, two.id, DlState::Done, None, Some(&path), 50).unwrap();
        assert_eq!(queue_downloads(&conn, &[film(2)], 60).unwrap(), q(0, 1));
        std::fs::remove_file(&file).unwrap();
        assert_eq!(queue_downloads(&conn, &[film(2)], 60).unwrap(), q(1, 0));
        assert_eq!(download(&conn, two.id).unwrap(), None);

        // Left running by a crash: queued again.
        download_started(&conn, one.id, "/x/Film 1.mkv").unwrap();
        assert_eq!(settle_downloads(&conn).unwrap(), 1);
        assert_eq!(
            download(&conn, one.id).unwrap().unwrap().state,
            DlState::Queued
        );
    }
}
