//! Slint window and the Rust glue (SPEC 8): models, key handling, selection.

use crate::config::{self, EpgSource, Provider};
use crate::epg_sources;
use crate::import;
use crate::info;
use crate::logos;
use crate::player::{Behind, Control, Event, Launch, Player, Remote};
use crate::recorder::{self, Recorder, recording_name, reserve_recording};
use crate::refresh::{self, AccountInfo, Refresher, Request, Stage, StageState, Stages, Update};
use crate::search;
use crate::store::{
    self, ChannelRow, Episode, Kind, ManagedCategory, Order, Programme, Progress, TitleRow,
    TitleView, UpNext, View, Watchable,
};
use crate::tables;
use crate::xtream::{self, Blocked, Client};
use rusqlite::Connection;
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

slint::include_modules!();

pub struct Options {
    /// When the process started, for the first-frame time.
    #[cfg(feature = "devtools")]
    pub started: Instant,
    #[cfg(feature = "devtools")]
    pub screenshot: Option<std::path::PathBuf>,
    #[cfg(feature = "devtools")]
    pub keys: Option<String>,
    /// Let `--keys` start playback, e.g. with a fake `mpv_path`.
    #[cfg(feature = "devtools")]
    pub play: bool,
}

/// What a key does (SPEC 8.3). Kept separate from its effect so replayed and
/// real keys go through the same mapping.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Move(isize),
    HalfPage(isize),
    Page(isize),
    Top,
    Bottom,
    Category(isize),
    Play,
    Favourite,
    /// `J` / `K`: move the selected favourite down / up.
    MoveFavourite(isize),
    Search,
    ClearFilter,
    Refresh,
    /// `p`: the provider picker.
    Providers,
    /// Space, `s`, `m`, `F`, `L`: the Player's controls.
    Control(Control),
    /// `R`: record what plays, or stop recording.
    Record,
    /// `S`: the programme airing now from its start (catch-up ticket 04).
    StartOver,
    /// `-`: play the channel before the current one.
    LastChannel,
    /// `o`: the keys into the selected channel's other listings (stream
    /// recovery ticket 03).
    Alternates,
    /// `c`: clear recently played, on a second press.
    ClearHistory,
    /// `w`: a movie, an episode or the season shown watched or not (VOD
    /// ticket 07).
    Watched,
    /// `d`: download the selected movie or episode (VOD ticket 10).
    Download,
    /// `D`: download the season shown.
    DownloadSeason,
    /// `a`: A–Z or provider order.
    ToggleOrder,
    /// `e`: manage mode, which categories the library shows.
    Manage,
    /// `E`: the selected channel's guide sources, pins and mappings (EPG
    /// sources ticket 05).
    GuideSources,
    /// `i`: the account dashboard.
    Account,
    /// `?`: the keys panel.
    Keys,
    /// `v`: the Recordings view.
    Recordings,
    /// `,`: the Settings view (settings ticket 01).
    Settings,
    /// `t`: the guide grid, or back to the list.
    Guide,
    /// `n`, `<`, `>`, `P`: through time, in the guide grid.
    Jump(Jump),
    /// `[` / `]`: previous / next category, in the guide grid.
    GuideCategory(isize),
    /// Tab / Shift-Tab: the library's next / previous kind (VOD ticket 03);
    /// in the guide grid, as `]` / `[`.
    Cycle(isize),
    Quit,
    #[cfg(feature = "devtools")]
    Snapshot,
}

/// `text` is Slint's key text: the character, or a `Key` code for special keys.
fn action(text: &str, ctrl: bool) -> Option<Action> {
    use Action::*;
    use slint::platform::Key;
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let is = |k: Key| c == char::from(k);
    if ctrl {
        // Some backends send the control character instead of the letter.
        return match c {
            'd' | '\u{4}' => Some(HalfPage(1)),
            'u' | '\u{15}' => Some(HalfPage(-1)),
            _ => None,
        };
    }
    Some(match c {
        'j' => Move(1),
        'k' => Move(-1),
        'g' => Top,
        'G' => Bottom,
        'h' => Category(-1),
        'l' => Category(1),
        'r' => Refresh,
        'p' => Providers,
        'f' => Favourite,
        'J' => MoveFavourite(1),
        'K' => MoveFavourite(-1),
        'q' => Quit,
        ' ' => Control(crate::player::Control::Pause),
        's' => Control(crate::player::Control::Stop),
        'm' => Control(crate::player::Control::Mute),
        'F' => Control(crate::player::Control::Fullscreen),
        'L' => Control(crate::player::Control::Live),
        'R' => Record,
        'S' => StartOver,
        '-' => LastChannel,
        'o' => Alternates,
        'c' => ClearHistory,
        'w' => Watched,
        'd' => Download,
        'D' => DownloadSeason,
        'a' => ToggleOrder,
        'e' => Manage,
        'E' => GuideSources,
        'i' => Account,
        '?' => Keys,
        'v' => Recordings,
        ',' => Settings,
        't' => Guide,
        'n' => Jump(self::Jump::Now),
        '<' => Jump(self::Jump::Day(-1)),
        '>' => Jump(self::Jump::Day(1)),
        'P' => Jump(self::Jump::Prime),
        '[' => GuideCategory(-1),
        ']' => GuideCategory(1),
        _ if is(Key::Tab) => Cycle(1),
        _ if is(Key::Backtab) => Cycle(-1),
        '/' => Search,
        '\n' | '\r' => Play,
        _ if is(Key::Escape) => ClearFilter,
        _ if is(Key::DownArrow) => Move(1),
        _ if is(Key::UpArrow) => Move(-1),
        _ if is(Key::PageDown) => Page(1),
        _ if is(Key::PageUp) => Page(-1),
        _ if is(Key::Home) => Top,
        _ if is(Key::End) => Bottom,
        _ if is(Key::LeftArrow) => Category(-1),
        _ if is(Key::RightArrow) => Category(1),
        #[cfg(feature = "devtools")]
        _ if is(Key::F12) => Snapshot,
        _ => return None,
    })
}

/// A key in search mode (SPEC 8.2). The app, not a text widget, handles
/// typing, so letters go to the query and only keys that cannot be typed
/// navigate.
#[derive(Clone, Debug, PartialEq)]
enum SearchKey {
    Nav(Action),
    Type(String),
    Backspace,
    /// Enter: keep the filter, back to the three-pane view.
    Keep,
    /// Esc: drop the filter, back to the three-pane view.
    Clear,
}

fn search_key(text: &str, ctrl: bool) -> Option<SearchKey> {
    use slint::platform::Key;
    if ctrl {
        return action(text, true).map(SearchKey::Nav);
    }
    let is = |k: Key| text.chars().eq([char::from(k)]);
    if is(Key::Escape) {
        return Some(SearchKey::Clear);
    }
    if is(Key::Return) || text == "\n" {
        return Some(SearchKey::Keep);
    }
    if is(Key::Backspace) {
        return Some(SearchKey::Backspace);
    }
    // Slint sends its other special keys as private-use characters.
    let typed = |c: char| !c.is_control() && !('\u{F700}'..='\u{F8FF}').contains(&c);
    if !text.is_empty() && text.chars().all(typed) {
        return Some(SearchKey::Type(text.into()));
    }
    match action(text, false)? {
        a @ (Action::Move(_)
        | Action::Page(_)
        | Action::Top
        | Action::Bottom
        | Action::Category(_)) => Some(SearchKey::Nav(a)),
        #[cfg(feature = "devtools")]
        a @ Action::Snapshot => Some(SearchKey::Nav(a)),
        _ => None,
    }
}

type KeyRows = &'static [(&'static [&'static str], &'static str)];

/// The keys panel (ticket 24), column by column: each group's keys and what
/// they do (SPEC 8.3, 9.2). `p` is dropped with one provider, F12 without
/// devtools.
const KEY_PANEL: [&[(&str, KeyRows)]; 4] = [
    &[
        (
            "MOVE",
            &[
                (&["j", "k", "↓", "↑"], "Down / up"),
                (&["Ctrl d", "Ctrl u"], "Half page down / up"),
                (&["PgDn", "PgUp"], "Page down / up"),
                (&["g", "G", "Home", "End"], "Top / bottom"),
                (&["h", "l", "←", "→"], "Previous / next category"),
            ],
        ),
        (
            "CHANNELS",
            &[
                (&["Enter"], "Play"),
                (&["f"], "Favourite"),
                (&["J", "K"], "Move favourite down / up"),
                (&["-"], "Last channel"),
                (&["o"], "Other listings of the channel"),
            ],
        ),
        (
            "GUIDE",
            &[
                (&["h", "l", "←", "→"], "Previous / next programme"),
                (&["n"], "Back to now"),
                (&["<", ">"], "A day back / ahead"),
                (&["P"], "Prime time, 20:00"),
                (&["Enter"], "Play live, or catch up"),
                (&["[", "]", "⇧Tab", "Tab"], "Previous / next category"),
                (&["R"], "Record / cancel the programme"),
            ],
        ),
    ],
    &[
        (
            "LIBRARY",
            &[
                (&["/"], "Search"),
                (&["t"], "Guide grid, or back to the list"),
                (&["Esc"], "Clear the search filter"),
                (&["a"], "Provider or A–Z order"),
                (&["Tab", "⇧Tab"], "Next / previous kind"),
                (&["e"], "Choose categories"),
                (&["E"], "Guide sources"),
                (&["c"], "Clear Recent / Continue (twice)"),
            ],
        ),
        (
            "MOVIES AND SERIES",
            &[
                (&["Enter"], "Play, from where it stopped"),
                (&["S"], "Play from the start"),
                (&["w"], "Watched / not, or a season"),
                (&["d"], "Download"),
                (&["D"], "Download the season"),
                (&["Enter"], "Series: episodes, or its next"),
                (&["h", "l", "←", "→"], "Previous / next season"),
                (&["Esc"], "Back to the list"),
            ],
        ),
    ],
    &[
        (
            "PLAYER",
            &[
                (&["Space"], "Pause / resume"),
                (&["s"], "Stop"),
                (&["m"], "Mute"),
                (&["F"], "Fullscreen"),
                (&["L"], "Go live"),
                (&["S"], "Start over what plays"),
                (&["R"], "Record / stop recording"),
            ],
        ),
        (
            "INSIDE MPV",
            &[
                (&["PgUp", "PgDn"], "Previous / next of what plays"),
                (&["0–9"], "Channel number"),
                (&["Backspace"], "Back to the list"),
                (&["L"], "Go live"),
                (&["S"], "Start over what plays"),
                (&["R"], "Record / stop recording"),
            ],
        ),
    ],
    &[(
        "APP",
        &[
            (&["r"], "Refresh, and show its progress"),
            (&["p"], "Switch provider"),
            (&["i"], "Account"),
            (&["v"], "Recordings and downloads"),
            (&[","], "Settings"),
            (&["?"], "Keys"),
            (&["F12"], "Save a snapshot"),
            (&["q"], "Quit"),
        ],
    )],
];

/// The keys panel's other views, one line each; the picker only with more
/// than one provider.
const KEY_NOTES: [(&str, &str); 7] = [
    ("Account", "1–3 show or hide a login field · Esc closes"),
    (
        "Search",
        "In the list or the guide: type to filter · Tab all kinds · Enter keeps the filter · Esc clears it",
    ),
    (
        "Manage",
        "Type to filter · Tab kind · Enter shows or hides · Ctrl+Enter adds or removes a rule · Ctrl+H hides names with it · Ctrl+K keeps it as a tag · Esc leaves",
    ),
    (
        "Refresh",
        "Esc or × hides its progress · the refresh carries on",
    ),
    (
        "Guide sources",
        "j / k move · Enter maps to one of a source's channels, typed to search · p pins · h / l shift the pin 5 min · Esc closes",
    ),
    (
        "Provider picker",
        "j / k move · Enter or 1–9 switch · the last row adds a provider · Esc closes",
    ),
    (
        "Recordings",
        "Tab recordings or downloads · j / k move · Enter plays a finished one · o opens its folder · twice: x cancels, d moves its file to the trash, c clears the finished ones · Esc closes",
    ),
];

/// The keys panel's contents for this app: [`KEY_PANEL`] and [`KEY_NOTES`]
/// less what does not apply.
fn key_panel() -> (Vec<KeyColumn>, Vec<KeyNote>) {
    let applies = |keys: &[&str]| match keys {
        ["F12"] => cfg!(feature = "devtools"),
        _ => true,
    };
    let columns = KEY_PANEL
        .iter()
        .map(|groups| {
            let groups: Vec<KeyGroup> = groups
                .iter()
                .map(|(title, rows)| {
                    let rows: Vec<KeyRow> = rows
                        .iter()
                        .filter(|(keys, _)| applies(keys))
                        .map(|(keys, what)| {
                            let keys: Vec<slint::SharedString> =
                                keys.iter().map(|&k| k.into()).collect();
                            KeyRow {
                                keys: ModelRc::new(VecModel::from(keys)),
                                action: (*what).into(),
                            }
                        })
                        .collect();
                    KeyGroup {
                        title: (*title).into(),
                        rows: ModelRc::new(VecModel::from(rows)),
                    }
                })
                .collect();
            KeyColumn {
                groups: ModelRc::new(VecModel::from(groups)),
            }
        })
        .collect();
    let notes = KEY_NOTES
        .iter()
        .map(|(view, text)| KeyNote {
            view: (*view).into(),
            text: (*text).into(),
        })
        .collect();
    (columns, notes)
}

/// The library (SPEC 8.2), as the sidebar lists it and the guide grid's
/// `[` and `]` step through it: the views across kinds, then `kind`'s All
/// (and Recently added, for movies and series), then the provider's shown
/// categories of `kind`, `(id, name)` in the order read.
fn library(kind: Kind, provider_cats: Vec<(String, String)>) -> Vec<Category> {
    [Category::Favourites, Category::Recent, Category::Continue]
        .into_iter()
        .chain([Category::All(kind)])
        .chain((kind != Kind::Live).then_some(Category::Added(kind)))
        .chain(
            provider_cats
                .into_iter()
                .map(|(id, name)| Category::Provider { kind, id, name }),
        )
        .collect()
}

/// How many library entries sit above the kind switch.
const LIBRARY_FIXED: usize = 3;

/// The grid row to open on for the main view's channel `key` (SPEC 8.2):
/// its own, or else the first guide channel after it in the category's
/// order `base`, or the last before it. `guide` is in that order too.
fn guide_row_for(base: &[i64], guide: &[i64], key: Option<i64>) -> usize {
    let Some(pos) = key.and_then(|k| base.iter().position(|&b| b == k)) else {
        return 0;
    };
    let mut row = 0;
    for &b in &base[..pos] {
        if guide.get(row) == Some(&b) {
            row += 1;
        }
    }
    row.min(guide.len().saturating_sub(1))
}

/// Sidebar entries: the views across kinds, then one kind's (VOD ticket
/// 03).
#[derive(Debug, PartialEq)]
enum Category {
    Favourites,
    Recent,
    /// Movies and series part way through (VOD ticket 07).
    Continue,
    All(Kind),
    /// A movie or series kind's latest titles.
    Added(Kind),
    Provider {
        kind: Kind,
        id: String,
        name: String,
    },
}

impl Category {
    fn name(&self) -> &str {
        match self {
            Category::Favourites => "Favourites",
            Category::Recent => "Recently played",
            Category::Continue => "Continue watching",
            Category::All(Kind::Live) => "All channels",
            Category::All(Kind::Movie) => "All movies",
            Category::All(Kind::Series) => "All series",
            Category::Added(_) => "Recently added",
            Category::Provider { name, .. } => name,
        }
    }

    /// What its rows are. Favourites, Recently played (VOD ticket 11) and
    /// Continue watching hold several kinds, each row saying which
    /// ([`Channels::kind_of`]); here channels for the first two and movies
    /// for the last.
    fn rows(&self) -> Kind {
        match self {
            Category::Favourites | Category::Recent => Kind::Live,
            Category::Continue => Kind::Movie,
            Category::All(k) | Category::Added(k) | Category::Provider { kind: k, .. } => *k,
        }
    }

    /// Its channels, if it lists channels.
    fn view(&self, order: Order) -> Option<View<'_>> {
        match self {
            Category::Favourites => Some(View::Favourites),
            Category::Recent => Some(View::Recent),
            Category::All(Kind::Live) => Some(View::All(order)),
            Category::Provider {
                kind: Kind::Live,
                id,
                ..
            } => Some(View::Category(id, order)),
            _ => None,
        }
    }

    /// Its movies or series, if it lists them.
    fn title_view(&self, order: Order) -> Option<TitleView<'_>> {
        match self {
            Category::All(k) if *k != Kind::Live => Some(TitleView::All(order)),
            Category::Added(_) => Some(TitleView::Added),
            Category::Provider { kind, id, .. } if *kind != Kind::Live => {
                Some(TitleView::Category(id, order))
            }
            _ => None,
        }
    }

    /// Its row keys, in `order` where it has one.
    fn keys(
        &self,
        conn: &Connection,
        provider_id: &str,
        order: Order,
    ) -> rusqlite::Result<Vec<i64>> {
        if let Some(view) = self.view(order) {
            store::channel_keys(conn, provider_id, view)
        } else if *self == Category::Continue {
            store::continue_watching(conn, provider_id)
        } else if let Some(view) = self.title_view(order) {
            store::title_keys(conn, provider_id, self.rows(), view)
        } else {
            Ok(Vec::new())
        }
    }
}

/// Logo boxes in logical px, as `app.slint` draws them (SPEC 8.2).
const ROW_LOGO: f32 = 32.0;
const SEARCH_LOGO: f32 = 40.0;
const RECENT_LOGO: f32 = 28.0;
const PLAYING_LOGO: f32 = 44.0;
const DETAIL_LOGO: f32 = 88.0;
/// Poster heights in logical px (VOD ticket 06).
const ROW_POSTER: f32 = 42.0;
const SEARCH_POSTER: f32 = 54.0;
const PLAYING_POSTER: f32 = 44.0;
const DETAIL_POSTER: f32 = 120.0;
/// Rows either side of those on screen whose logos are fetched ahead.
const LOGO_MARGIN: usize = 4;
/// Logo images kept in memory.
const LOGOS_HELD: usize = 1024;

/// Logos ready to draw, by URL and pixel size, and the URLs that failed
/// this run (ticket 13). Anything else is asked of the loader.
struct Logos {
    window: slint::Weak<AppWindow>,
    tx: mpsc::UnboundedSender<logos::Request>,
    /// With when each was last used, for eviction.
    images: RefCell<HashMap<(String, u32), (slint::Image, u64)>>,
    failed: RefCell<HashSet<String>>,
    clock: Cell<u64>,
}

impl Logos {
    /// Physical pixels for a box of `size` logical px.
    fn px(&self, size: f32) -> u32 {
        let scale = self
            .window
            .upgrade()
            .map_or(1.0, |w| w.window().scale_factor());
        (size * scale).round() as u32
    }

    /// The logo for a `size` px box, or an empty image, so the tile shows,
    /// while it loads or if it failed. Asks for it if it is not held.
    fn get(&self, url: Option<&str>, size: f32) -> slint::Image {
        self.image(url, size, false)
    }

    /// A movie's or series' poster `size` px high, as [`Logos::get`].
    fn poster(&self, url: Option<&str>, size: f32) -> slint::Image {
        self.image(url, size, true)
    }

    fn image(&self, url: Option<&str>, size: f32, poster: bool) -> slint::Image {
        let Some(url) = url.filter(|u| !u.is_empty()) else {
            return slint::Image::default();
        };
        let px = self.px(size);
        let t = self.clock.get() + 1;
        self.clock.set(t);
        if let Some((image, used)) = self.images.borrow_mut().get_mut(&(url.to_owned(), px)) {
            *used = t;
            return image.clone();
        }
        if !self.failed.borrow().contains(url) {
            let _ = self.tx.send(logos::Request {
                url: url.to_owned(),
                px,
                poster,
            });
        }
        slint::Image::default()
    }

    /// A logo arrived. False if it failed.
    fn insert(&self, logo: logos::Logo) -> bool {
        let Some(pixels) = logo.pixels else {
            self.failed.borrow_mut().insert(logo.url);
            return false;
        };
        let mut images = self.images.borrow_mut();
        if images.len() >= LOGOS_HELD {
            // Drop the least recently used quarter.
            let mut used: Vec<u64> = images.values().map(|(_, u)| *u).collect();
            used.sort_unstable();
            let cut = used[LOGOS_HELD / 4];
            images.retain(|_, (_, u)| *u > cut);
        }
        images.insert(
            (logo.url, logo.px),
            (slint::Image::from_rgba8(pixels), self.clock.get()),
        );
        true
    }
}

/// The middle list's row model: channels, or a kind's movies or series
/// (VOD ticket 03). Holds only the current view's row keys; Slint's
/// `ListView` asks for the rows on screen, which are read from SQLite one
/// at a time (ADR 0003, 0005).
struct Channels {
    conn: Rc<Connection>,
    /// What the keys are keys of.
    kind: Cell<Kind>,
    /// The keys are [`store::list_key`]s, of several kinds: Favourites,
    /// Recently played (VOD ticket 11) and Continue watching (07).
    mixed: Cell<bool>,
    /// Continue watching: a series' row says its episode to carry on with.
    carry_on: Cell<bool>,
    /// A widened search: a title's row says its kind.
    labelled: Cell<bool>,
    /// Channels and titles together: a title's kind goes where a
    /// channel's number is, so the rows line up.
    with_channels: Cell<bool>,
    keys: RefCell<Vec<i64>>,
    notify: ModelNotify,
    logos: Rc<Logos>,
    /// `ROW_LOGO`, or `SEARCH_LOGO` in search mode.
    logo_size: Cell<f32>,
    /// Rows drawn without their logo, by logo URL, to redraw when it comes.
    waiting: RefCell<HashMap<String, Vec<usize>>>,
}

impl Channels {
    fn set_keys(&self, kind: Kind, mixed: bool, carry_on: bool, labelled: bool, keys: Vec<i64>) {
        self.kind.set(kind);
        self.mixed.set(mixed);
        self.carry_on.set(carry_on);
        self.labelled.set(labelled);
        let kinds = || keys.iter().map(|&k| store::from_list_key(k).0);
        self.with_channels
            .set(mixed && kinds().any(|k| k == Kind::Live) && kinds().any(|k| k != Kind::Live));
        *self.keys.borrow_mut() = keys;
        self.waiting.borrow_mut().clear();
        self.notify.reset();
    }

    /// Rows are taller in search mode, and so are their logos.
    fn set_searching(&self, searching: bool) {
        let size = if searching { SEARCH_LOGO } else { ROW_LOGO };
        if self.logo_size.replace(size) != size {
            self.refresh();
        }
    }

    /// A logo arrived: redraws the rows that were waiting for it.
    fn logo_ready(&self, url: &str) {
        let rows = self.waiting.borrow_mut().remove(url);
        for i in rows.into_iter().flatten() {
            if i < self.row_count() {
                self.notify.row_changed(i);
            }
        }
    }

    /// A poster's height in rows: taller in search mode, as logos are.
    fn poster_size(&self) -> f32 {
        match self.logo_size.get() == SEARCH_LOGO {
            true => SEARCH_POSTER,
            false => ROW_POSTER,
        }
    }

    /// Asks for the logos or posters of the rows [`LOGO_MARGIN`] either
    /// side of row `i`, so they are ready when scrolled to.
    fn fetch_ahead(&self, i: usize) {
        let around = [i.checked_sub(LOGO_MARGIN), Some(i + LOGO_MARGIN)];
        for j in around.into_iter().flatten() {
            let Some(mut key) = self.keys.borrow().get(j).copied() else {
                continue;
            };
            let kind = self.kind_of(j);
            if self.mixed.get() {
                key = store::from_list_key(key).1;
            }
            if kind == Kind::Live {
                let url = store::icon_url(&self.conn, key)
                    .inspect_err(|e| log::error!("reading channel: {e}"))
                    .ok()
                    .flatten();
                self.logos.get(url.as_deref(), self.logo_size.get());
            } else {
                let url = store::poster_url(&self.conn, kind, key)
                    .inspect_err(|e| log::error!("reading {}: {e}", kind.as_str()))
                    .ok()
                    .flatten();
                self.logos.poster(url.as_deref(), self.poster_size());
            }
        }
    }

    /// Re-reads every row Slint has built, e.g. when programmes change over.
    fn refresh(&self) {
        for i in 0..self.row_count() {
            self.notify.row_changed(i);
        }
    }

    /// Channel row `i`; `None` if it is a movie or series.
    fn row(&self, i: usize, now: i64) -> Option<ChannelRow> {
        if self.kind_of(i) != Kind::Live {
            return None;
        }
        let key = *self.keys.borrow().get(i)?;
        read_channel(&self.conn, key, now)
    }

    /// What row `i` is: the list's kind, or in a mixed list its own.
    fn kind_of(&self, i: usize) -> Kind {
        match self.keys.borrow().get(i) {
            Some(&key) if self.mixed.get() => store::from_list_key(key).0,
            _ => self.kind.get(),
        }
    }

    /// Movie or series row `i`; `None` if it is a channel.
    fn title(&self, i: usize) -> Option<TitleRow> {
        let kind = self.kind_of(i);
        if kind == Kind::Live {
            return None;
        }
        let mut key = *self.keys.borrow().get(i)?;
        if self.mixed.get() {
            key = store::from_list_key(key).1;
        }
        store::title(&self.conn, kind, key)
            .inspect_err(|e| log::error!("reading {}: {e}", kind.as_str()))
            .ok()
            .flatten()
    }

    /// A movie or series row: its poster (VOD ticket 06), the title, and
    /// what the list knows of it; a movie's progress or Watched, and in
    /// Continue watching a series' episode to carry on with (VOD ticket 07).
    fn title_item(&self, i: usize) -> Option<ChannelItem> {
        let row = self.title(i)?;
        // Ahead first: the loader takes the newest request first.
        self.fetch_ahead(i);
        let poster = self
            .logos
            .poster(row.poster_url.as_deref(), self.poster_size());
        if let Some(url) = row.poster_url.as_ref().filter(|_| poster.size().width == 0) {
            self.waiting
                .borrow_mut()
                .entry(url.clone())
                .or_default()
                .push(i);
        }
        let mut now = title_meta(&row);
        let mut part = row.progress.as_ref().and_then(part_way);
        if row.progress.is_some_and(|p| p.watched && p.pos == 0) {
            now = [now.as_str(), "✓ Watched"]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("  ·  ");
        }
        if self.carry_on.get() && self.kind_of(i) == Kind::Series {
            match store::series_up_next(&self.conn, &row.provider_id, row.stream_id) {
                Ok(UpNext::Resume(e, p)) => {
                    now = episode_line(&e);
                    part = part_way(&p);
                }
                Ok(UpNext::Next(e)) => now = format!("Next: {}", episode_line(&e)),
                Ok(_) => {}
                Err(e) => log::error!("reading series progress: {e}"),
            }
        }
        let kind = match self.kind_of(i) {
            Kind::Series => "Series",
            _ => "Movie",
        };
        let with_channels = self.with_channels.get();
        if self.labelled.get() && !with_channels {
            now = [kind, now.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("  ·  ");
        }
        Some(ChannelItem {
            num: if with_channels { kind } else { "" }.into(),
            name: row.name.as_str().into(),
            now: now.into(),
            has_now: part.is_some(),
            progress: part.unwrap_or(0.0),
            available: true,
            initials: initials(&row.name).into(),
            tile: tile(&row.name),
            logo: poster,
            poster: true,
            favourite: row.favourite,
        })
    }
}

/// A movie's or series' line under its title (VOD ticket 03): the year
/// (a series' years once its info is in), a movie's length, then the
/// rating. A movie's year and length come with its info (ticket 04).
fn title_meta(row: &TitleRow) -> String {
    [years(row), row.length.map(duration), rating(row.rating)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ·  ")
}

/// `2019`, or for a series running on, `2019 – 2023` up to its latest
/// episode's year (VOD ticket 05).
fn years(row: &TitleRow) -> Option<String> {
    let first = row.year?;
    Some(match row.last_year {
        Some(last) if last > first => format!("{first} – {last}"),
        _ => first.to_string(),
    })
}

/// A series' line in the detail panel (VOD ticket 05): its years, how many
/// seasons the provider has, the genre and the rating.
fn series_meta(row: &TitleRow) -> String {
    let seasons = row.seasons.filter(|n| *n > 0).map(|n| match n {
        1 => "1 season".to_string(),
        n => format!("{n} seasons"),
    });
    // It wraps between parts only, never inside `★ 8.1` or the years.
    [years(row), seasons, row.genre.clone(), rating(row.rating)]
        .into_iter()
        .flatten()
        .map(|part| part.replace(' ', "\u{a0}"))
        .collect::<Vec<_>>()
        .join("  ·  ")
}

/// `S2 E4`, or `S2` for an episode without a number.
fn episode_label(e: &Episode) -> String {
    match e.num {
        Some(n) => format!("S{} E{n}", e.season),
        None => format!("S{}", e.season),
    }
}

/// `S2 E4  ·  The Night Ferry`, as the NOW PLAYING card says it.
fn episode_line(e: &Episode) -> String {
    match episode_name(&e.title) {
        "" => episode_label(e),
        name => format!("{}  ·  {name}", episode_label(e)),
    }
}

/// How far through a part-watched movie or episode is, 0 to 1, for its
/// bar; none from the start or without a length (VOD ticket 07).
fn part_way(p: &Progress) -> Option<f32> {
    let len = p.length.filter(|l| *l > 0)?;
    (p.pos > 0).then(|| (p.pos as f32 / len as f32).clamp(0.0, 1.0))
}

/// The NOW card's lines for a resume point: `From 0:42:10` and
/// `51 min left`.
fn resume_lines(p: &Progress) -> (String, String) {
    let left = p
        .length
        .filter(|l| *l > p.pos)
        .map(|l| format!("{} left", duration(l - p.pos)));
    (format!("From {}", hms(p.pos)), left.unwrap_or_default())
}

/// An episode as its progress is kept.
fn watchable(series_id: i64, e: &Episode) -> Watchable {
    Watchable::Episode {
        series_id,
        stream_id: e.stream_id,
        season: e.season,
        num: e.num,
    }
}

/// An episode's own name. The provider's title is its whole one (SPEC
/// 6.1), often the series and `S01E04` before the name: what follows that
/// mark, or without one the whole title. Empty when nothing follows it.
fn episode_name(title: &str) -> &str {
    let t = title.trim();
    let b = t.as_bytes();
    let digits = |from: usize| {
        b[from.min(b.len())..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count()
    };
    for i in 0..b.len() {
        if !b[i].eq_ignore_ascii_case(&b's') || (i > 0 && b[i - 1].is_ascii_alphanumeric()) {
            continue;
        }
        let s = digits(i + 1);
        let e_at = i + 1 + s;
        if s == 0 || b.get(e_at).map(u8::to_ascii_lowercase) != Some(b'e') {
            continue;
        }
        let e = digits(e_at + 1);
        let end = e_at + 1 + e;
        if e == 0 || b.get(end).is_some_and(u8::is_ascii_alphanumeric) {
            continue;
        }
        return t[end..]
            .trim_start_matches(|c: char| c.is_whitespace() || "-\u{2013}\u{2014}|:.".contains(c))
            .trim_end();
    }
    t
}

/// An episode as its row in the detail panel shows it.
fn episode_item(e: &Episode, p: Option<&Progress>, d: Option<&store::Download>) -> EpisodeItem {
    let name = episode_name(&e.title);
    let part = p.and_then(part_way);
    EpisodeItem {
        download: download_mark(d).0.into(),
        watched: p.is_some_and(|p| p.watched),
        has_progress: part.is_some(),
        progress: part.unwrap_or(0.0),
        num: e.num.map(|n| n.to_string()).unwrap_or_default().into(),
        title: match (name, e.num) {
            ("", Some(n)) => format!("Episode {n}"),
            ("", None) => e.title.clone(),
            (name, _) => name.to_string(),
        }
        .into(),
        length: e.length.map(duration).unwrap_or_default().into(),
    }
}

/// An episode's URL, mpv title and NOW PLAYING line (SPEC 9.1, VOD ticket
/// 05): the series, `S2 E4` and the episode's name, never the URL.
fn episode_request(
    client: &Client,
    series: &str,
    e: &Episode,
) -> Result<(reqwest::Url, String, String), String> {
    let Some(ext) = &e.extension else {
        return Err(format!(
            "The provider gives no format for {series} {}",
            episode_label(e)
        ));
    };
    let url = client.vod_url("series", e.stream_id, ext)?;
    let line = episode_line(e);
    let title = format!("{series} · {}", line.replace("  ·  ", " · "));
    Ok((url, title, line))
}

/// Episode `e` of series `row` to download (VOD ticket 10); none without
/// a format.
fn episode_download(row: &TitleRow, e: &Episode) -> Option<store::NewDownload> {
    Some(store::NewDownload {
        provider_id: row.provider_id.clone(),
        stream_id: e.stream_id,
        series_id: Some(row.stream_id),
        season: Some(e.season),
        episode_num: e.num,
        title: row.name.clone(),
        episode: Some(episode_name(&e.title).to_string()).filter(|n| !n.is_empty()),
        extension: e.extension.clone()?,
    })
}

/// The detail panel's facts about a movie or series, as its list gives
/// them (SPEC 6.1).
fn title_facts(kind: Kind, row: &TitleRow) -> Vec<FactItem> {
    let added = match kind {
        Kind::Series => "UPDATED",
        _ => "ADDED",
    };
    [
        ("GENRE", row.genre.clone()),
        ("CAST", row.actors.clone()),
        ("DIRECTOR", row.director.clone()),
        (
            "EPISODES",
            row.run_time.filter(|m| *m > 0).map(|m| format!("{m} min")),
        ),
        (added, row.added.filter(|t| *t > 0).map(date)),
    ]
    .into_iter()
    .filter_map(|(label, value)| {
        Some(FactItem {
            label: label.into(),
            value: value?.into(),
        })
    })
    .collect()
}

/// A movie's URL and mpv title (SPEC 9.1): its name, never the URL.
fn movie_request(client: &Client, row: &TitleRow) -> Result<(reqwest::Url, String), String> {
    let Some(ext) = &row.extension else {
        return Err(format!("The provider gives no format for {}", row.name));
    };
    let url = client.vod_url("movie", row.stream_id, ext)?;
    Ok((url, row.name.clone()))
}

/// The NOW PLAYING card for a movie (VOD ticket 04): where it is and its
/// length, both mpv's. No go live or record; start over takes it back to
/// the start (VOD ticket 07).
fn movie_playing(m: &MoviePlaying, mpv: &MpvState) -> PlayingItem {
    let pos = m.pos as i64;
    let len = m.length.map(|l| l as i64).filter(|l| *l > 0);
    PlayingItem {
        visible: true,
        heading: if mpv.paused { "PAUSED" } else { "NOW PLAYING" }.into(),
        movie: true,
        name: m.name.as_str().into(),
        initials: initials(&m.name).into(),
        tile: tile(&m.name),
        title: if m.meta.is_empty() { "Movie" } else { &m.meta }.into(),
        has_guide: len.is_some(),
        span: match len {
            Some(len) => format!("{} / {}", hms(pos), hms(len)),
            None => hms(pos),
        }
        .into(),
        left: len
            .map(|len| format!("{} left", duration(len - pos)))
            .unwrap_or_default()
            .into(),
        progress: len.map_or(0.0, |len| pos as f32 / len as f32),
        // `S` takes it back to the start (VOD ticket 07).
        start_over: START_OVER_HINT.into(),
        paused: mpv.paused,
        muted: mpv.muted,
        fullscreen: mpv.fullscreen,
        ..Default::default()
    }
}

/// `0:12:34`, as mpv shows a position.
fn hms(secs: i64) -> String {
    let secs = secs.max(0);
    format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

/// `★ 7.4`; none for a missing or zero rating, which providers send for
/// unrated titles.
fn rating(r: Option<f64>) -> Option<String> {
    r.filter(|r| *r > 0.0).map(|r| format!("★ {r:.1}"))
}

impl Model for Channels {
    type Data = ChannelItem;

    fn row_count(&self) -> usize {
        self.keys.borrow().len()
    }

    fn row_data(&self, i: usize) -> Option<ChannelItem> {
        // Debug builds: `RUST_LOG=vidicon::ui=trace` shows that only visible rows are built.
        #[cfg(debug_assertions)]
        log::trace!("row_data({i})");
        if self.kind_of(i) != Kind::Live {
            return self.title_item(i);
        }
        let now = import::unix_now();
        let row = self.row(i, now)?;
        // Ahead first: the loader takes the newest request first.
        self.fetch_ahead(i);
        let logo = self
            .logos
            .get(row.icon_url.as_deref(), self.logo_size.get());
        if let Some(url) = row.icon_url.as_ref().filter(|_| logo.size().width == 0) {
            self.waiting
                .borrow_mut()
                .entry(url.clone())
                .or_default()
                .push(i);
        }
        let now_text = match &row.now {
            _ if !row.available => UNAVAILABLE,
            Some(p) => p.title.as_str(),
            None => NO_EPG,
        };
        Some(ChannelItem {
            num: row.num.map(|n| n.to_string()).unwrap_or_default().into(),
            name: row.name.as_str().into(),
            now: now_text.into(),
            progress: row.now.as_ref().map_or(0.0, |p| progress(p, now)),
            has_now: row.now.is_some(),
            favourite: row.favourite,
            available: row.available,
            initials: initials(&row.name).into(),
            tile: tile(&row.name),
            logo,
            // An unavailable favourite movie or series keeps its shape.
            poster: row.kind != Kind::Live,
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// The guide grid's span (SPEC 8.2): 3 h across.
const GUIDE_SPAN: i64 = 3 * 3600;

/// Where the grid's span starts for `now`: the half hour it is in, so the
/// header's ticks fall on the half hours.
fn guide_start(now: i64) -> i64 {
    now - now.rem_euclid(1800)
}

/// The grid's blocks for a channel's programmes, in order, placed from
/// `origin` (set when the grid opens): `x` and `w` in spans, `start` in
/// seconds, which the selection is matched against ([`guide_sel`]). A
/// block keeps its place while the grid is open, so re-reading a row for
/// another page changes only the blocks that differ ([`sync_blocks`]). A
/// programme the next one overlaps (SPEC 6.1) is cut where that one
/// starts, so the later one is the one on now, as in the list. `marks` are
/// the channel's scheduled and running recordings by guide start
/// ([`Guide::read_marks`]). A past programme the channel's archive keeps
/// has the archive mark; one it does not is dimmed ([`replayable`]).
fn guide_blocks(
    progs: &[Programme],
    origin: i64,
    now: i64,
    marks: &[(i64, store::RecState)],
    archive_days: i64,
) -> Vec<GuideBlock> {
    progs
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let stop = progs.get(i + 1).map_or(p.stop, |n| p.stop.min(n.start));
            let replay = replayable(p, stop, archive_days, now);
            (p.start < stop).then(|| GuideBlock {
                title: p.title.as_str().into(),
                times: span(p).into(),
                x: (p.start - origin) as f32 / GUIDE_SPAN as f32,
                w: (stop - p.start) as f32 / GUIDE_SPAN as f32,
                airing: p.start <= now && now < stop,
                start: (p.start - origin) as i32,
                rec: rec_mark(marks, p.start),
                archive: matches!(replay, Some(Ok(()))),
                gone: matches!(replay, Some(Err(_))),
            })
        })
        .collect()
}

/// Whether programme `p`, cut to end at `stop`, can be played from its
/// channel's archive at `now` (SPEC 8.2), or why not; `None` while it has
/// not ended, which Enter plays live. The archive keeps `archive_days`
/// back, 0 for none; where the channel's table said whether it has the
/// programme, that decides (catch-up ticket 06).
pub(crate) fn replayable(
    p: &Programme,
    stop: i64,
    archive_days: i64,
    now: i64,
) -> Option<Result<(), String>> {
    if stop > now {
        return None;
    }
    Some(match (archive_days, p.archive) {
        (0, _) => Err("No archive on this channel".into()),
        (_, Some(true)) => Ok(()),
        (_, Some(false)) => Err("Not in this channel's archive".into()),
        (d, None) if p.start < now - d * 86_400 => {
            Err(format!("Older than this channel's {d}-day archive"))
        }
        _ => Ok(()),
    })
}

/// The detail strip's line on a programme the archive keeps (SPEC 8.2).
const CATCH_UP_HINT: &str = "CATCH-UP";

/// The programme `airing` now on a channel whose archive keeps
/// `archive_days`, to play from its start (SPEC 8.2, catch-up ticket 04),
/// or why not.
fn start_over(airing: Option<Programme>, archive_days: i64) -> Result<Programme, &'static str> {
    if archive_days == 0 {
        return Err("No archive on this channel");
    }
    airing.ok_or("Nothing to start over: the guide has no programme airing")
}

/// The detail strip's and NOW PLAYING card's hint where `S` starts over.
const START_OVER_HINT: &str = "START OVER";

/// The programme in a grid cell `R` can record, or why not: a gap, or one
/// that has ended. One airing now can (SPEC 8.2).
fn recordable(slot: Slot, now: i64) -> Result<Programme, &'static str> {
    match slot.programme {
        None => Err("Nothing to record: the guide has no programme here"),
        Some(_) if slot.to <= now => Err("That programme has ended"),
        Some(p) => Ok(p),
    }
}

/// The record mark of the programme starting at `start`.
fn rec_mark(marks: &[(i64, store::RecState)], start: i64) -> RecMark {
    match marks.iter().find(|(s, _)| *s == start) {
        Some((_, store::RecState::Recording)) => RecMark::Recording,
        Some(_) => RecMark::Scheduled,
        None => RecMark::None,
    }
}

/// Brings a row's block model to `new` with as few changes as it takes, so
/// Slint keeps the blocks both have rather than building them again. Both
/// are ordered by `start`, which is unique in a row.
fn sync_blocks(model: &VecModel<GuideBlock>, new: Vec<GuideBlock>) {
    let mut i = 0;
    for b in new {
        loop {
            match model.row_data(i) {
                // Gone from the row.
                Some(old) if old.start < b.start => {
                    model.remove(i);
                }
                Some(old) if old.start == b.start => {
                    if old != b {
                        model.set_row_data(i, b);
                    }
                    break;
                }
                // New, before the next kept one or at the end.
                _ => {
                    model.insert(i, b);
                    break;
                }
            }
        }
        i += 1;
    }
    while model.row_count() > i {
        model.remove(i);
    }
}

/// The selected cell as the grid draws it, placed from `origin` as the
/// blocks are: the row's block whose `start` matches, or for a gap, where
/// to draw it. A selection move then sets this alone, rather than
/// re-reading two rows (ticket 06).
fn guide_sel(row: usize, slot: &Slot, origin: i64) -> GuideSel {
    GuideSel {
        row: row as i32,
        start: slot
            .programme
            .as_ref()
            .map_or(i32::MIN, |p| (p.start - origin) as i32),
        gap: slot.programme.is_none(),
        x: (slot.from - origin) as f32 / GUIDE_SPAN as f32,
        w: (slot.to - slot.from) as f32 / GUIDE_SPAN as f32,
    }
}

/// A gap in a channel's guide, selected in the grid.
const NO_INFO: &str = "No information";

/// What the guide keeps around `now` (SPEC 7.1): `back` seconds back
/// ([`guide_back`]) to 3 days ahead. The grid's selection and span stay
/// inside it.
fn guide_window(now: i64, back: i64) -> (i64, i64) {
    (now - back, now + 3 * 86_400)
}

/// How far back the grid reaches for channels whose longest archive is
/// `archive_days`: that far, or 1 h without one (catch-up ticket 03).
fn guide_back(archive_days: i64) -> i64 {
    match archive_days {
        0 => 3600,
        d => d * 86_400,
    }
}

/// Whether a grid row wants its channel's table (catch-up ticket 06): the
/// page it is read for starts in the past, before the first of its
/// programmes `progs`, and the archive reaches into the page.
fn needs_table(progs: &[Programme], page: (i64, i64), archive_days: i64, now: i64) -> bool {
    archive_days > 0
        && page.0 < now
        && page.1 > now - archive_days * 86_400
        && progs.first().is_none_or(|p| p.start > page.0)
}

/// A selectable cell of a grid row, `from..to`: a programme, cut where the
/// next one starts (SPEC 6.1), or a gap the guide says nothing about.
#[derive(Clone, Debug, PartialEq)]
struct Slot {
    from: i64,
    to: i64,
    programme: Option<Programme>,
}

/// A row's cells across `lo..hi`, in order and with no holes, so every
/// row has a cell at every time.
fn guide_slots(progs: &[Programme], lo: i64, hi: i64) -> Vec<Slot> {
    let mut slots = Vec::new();
    let mut at = lo;
    for (i, p) in progs.iter().enumerate() {
        let stop = progs.get(i + 1).map_or(p.stop, |n| p.stop.min(n.start));
        let (from, to) = (p.start.max(at), stop.min(hi));
        if from >= to {
            continue;
        }
        push_gap(&mut slots, at, from);
        slots.push(Slot {
            from,
            to,
            programme: Some(p.clone()),
        });
        at = to;
    }
    push_gap(&mut slots, at, hi);
    slots
}

/// The gap `from..to` as cells. One longer than the span, such as the days
/// before a guide that does not reach back as far as the grid (catch-up
/// ticket 03), is cut at every multiple of the span, so `h`/`l` step
/// through it a span at a time and the selection is never drawn wider.
fn push_gap(slots: &mut Vec<Slot>, mut from: i64, to: i64) {
    if to - from > GUIDE_SPAN {
        let mut cut = from - from.rem_euclid(GUIDE_SPAN) + GUIDE_SPAN;
        while cut < to {
            slots.push(Slot {
                from,
                to: cut,
                programme: None,
            });
            (from, cut) = (cut, cut + GUIDE_SPAN);
        }
    }
    if from < to {
        slots.push(Slot {
            from,
            to,
            programme: None,
        });
    }
}

/// The index of the cell at `t`; the first or last cell for a time before
/// or after the row.
fn slot_at(slots: &[Slot], t: i64) -> usize {
    slots
        .iter()
        .position(|s| t < s.to)
        .unwrap_or(slots.len().saturating_sub(1))
}

/// The time `j`/`k` keep to once `slot` is selected: now while it airs, so
/// it follows the clock, else its start.
fn guide_anchor(slot: &Slot, now: i64) -> Option<i64> {
    (!(slot.from..slot.to).contains(&now)).then_some(slot.from)
}

/// The selected time for `anchor`, inside the kept window.
fn guide_time(anchor: Option<i64>, now: i64, back: i64) -> i64 {
    let (lo, hi) = guide_window(now, back);
    anchor.unwrap_or(now).clamp(lo, hi - 1)
}

/// The page rows hold for a view starting at `view`: the half hours it
/// touches. Moving through time within it moves one offset; past it, each
/// row is read again but changes only at its ends ([`sync_blocks`]).
fn guide_page(view: i64) -> (i64, i64) {
    let from = guide_start(view);
    (from, from + GUIDE_SPAN + 1800)
}

/// The header's half-hour ticks in the span from `view`: each time and
/// where it falls, 0 to 1 across the timeline.
fn guide_ticks(view: i64) -> Vec<(i64, f32)> {
    let first = guide_start(view + 1799);
    (0..)
        .map(|i| first + i * 1800)
        .take_while(|&t| t < view + GUIDE_SPAN)
        .map(|t| (t, (t - view) as f32 / GUIDE_SPAN as f32))
        .collect()
}

/// A drag in the grid moves channels or time, never both (SPEC 8.4).
#[derive(Clone, Copy, Debug, PartialEq)]
enum DragAxis {
    Vertical,
    Horizontal,
}

/// How far, in logical pixels, a drag moves before its direction counts.
const DRAG_LOCK: f32 = 8.0;

/// The direction a drag locks to, from its movement so far: none until it
/// has moved [`DRAG_LOCK`] pixels, then whichever way it went further,
/// vertical on a tie as the list pans that way.
fn drag_axis(dx: f32, dy: f32) -> Option<DragAxis> {
    if dx.hypot(dy) < DRAG_LOCK {
        None
    } else if dx.abs() > dy.abs() {
        Some(DragAxis::Horizontal)
    } else {
        Some(DragAxis::Vertical)
    }
}

/// `start` moved so the span stays inside the kept window.
fn guide_clamp(start: i64, now: i64, back: i64) -> i64 {
    let (lo, hi) = guide_window(now, back);
    start.clamp(guide_start(lo), guide_start(hi + 1799) - GUIDE_SPAN)
}

/// The span's start once the cell `from..to` is in view, moved from `start`
/// only as far as needed: back to the cell's start, or on until its end
/// shows, but never past its start.
fn guide_follow(start: i64, from: i64, to: i64, now: i64, back: i64) -> i64 {
    let s = if from < start {
        guide_start(from)
    } else if to > start + GUIDE_SPAN {
        guide_start(from).min(guide_start(to + 1799) - GUIDE_SPAN)
    } else {
        start
    };
    guide_clamp(s, now, back)
}

/// The grid's jumps through time (SPEC 8.3).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Jump {
    /// `n`: back to now.
    Now,
    /// `<` / `>`: a day back or ahead.
    Day(i64),
    /// `P`: prime time.
    Prime,
}

/// Where a jump from the selected time `at` lands, with the span starting
/// at `start`: the time to select at and the span's new start. Both stop at
/// the kept window's ends, the time still in view.
fn guide_jump(jump: Jump, at: i64, start: i64, now: i64, back: i64) -> (i64, i64) {
    let (lo, hi) = guide_window(now, back);
    let (t, s) = match jump {
        Jump::Now => (now, guide_start(now)),
        // The span moves by the day too, so the layout stays as it was.
        Jump::Day(d) => (at + d * 86_400, start + d * 86_400),
        Jump::Prime => {
            let t = prime_time(now);
            (t, guide_start(t))
        }
    };
    let t = t.clamp(lo, hi - 1);
    let s = guide_clamp(s, now, back);
    if (s..s + GUIDE_SPAN).contains(&t) {
        (t, s)
    } else {
        (t, guide_clamp(guide_start(t), now, back))
    }
}

/// Prime time: 20:00 local today, or tomorrow once that has passed.
fn prime_time(now: i64) -> i64 {
    let at = |days: i32| {
        let mut tm = local_tm(now);
        (tm.tm_hour, tm.tm_min, tm.tm_sec) = (20, 0, 0);
        tm.tm_mday += days;
        tm.tm_isdst = -1;
        // SAFETY: `mktime` normalises the fields of a valid `tm`.
        unsafe { libc::mktime(&mut tm) as i64 }
    };
    let today = at(0);
    if now < today { today } else { at(1) }
}

/// The guide grid's row model: the guide channels' row keys, each row's
/// programmes read for the span as Slint builds it (ADR 0003, 0005).
struct Guide {
    conn: Rc<Connection>,
    provider_id: RefCell<String>,
    keys: RefCell<Vec<i64>>,
    /// The page of time rows are read for ([`guide_page`]).
    page: Cell<(i64, i64)>,
    /// Where blocks are placed from, set when the grid opens.
    origin: Cell<i64>,
    /// Each row's block model by row key, kept so a re-read row updates in
    /// place ([`sync_blocks`]).
    blocks: RefCell<HashMap<i64, Rc<VecModel<GuideBlock>>>>,
    /// The selected row and cell.
    sel: RefCell<Option<(usize, Slot)>>,
    /// Scheduled and running recordings by stream id, as guide starts and
    /// states, read on each [`Guide::refresh`] rather than per row.
    marks: RefCell<HashMap<i64, Vec<(i64, store::RecState)>>>,
    notify: ModelNotify,
    logos: Rc<Logos>,
    /// Rows drawn without their logo, by logo URL, to redraw when it comes.
    waiting: RefCell<HashMap<String, Vec<usize>>>,
    /// Where rows ask for their channel's table ([`tables`]).
    tables: mpsc::UnboundedSender<tables::Ask>,
    /// The last ask of each row read that wanted its table, by row key.
    asked: RefCell<HashMap<i64, tables::Ask>>,
    /// Each provider's server clock offset from its latest account check:
    /// tables give times in it (SPEC 6.1), so none are asked for before.
    offsets: RefCell<HashMap<String, i64>>,
}

impl Guide {
    fn set_keys(&self, provider_id: &str, keys: Vec<i64>) {
        provider_id.clone_into(&mut self.provider_id.borrow_mut());
        *self.keys.borrow_mut() = keys;
        self.waiting.borrow_mut().clear();
        self.asked.borrow_mut().clear();
        self.blocks.borrow_mut().clear();
        self.read_marks();
        self.notify.reset();
    }

    /// As [`Guide::set_keys`] for the same provider, but the rows Slint has
    /// built are re-read in place rather than rebuilt: a reset tears down
    /// and rebuilds every visible row, about 13 ms at 50k, too slow per
    /// keystroke (SPEC 10).
    fn replace_keys(&self, keys: Vec<i64>) {
        let (old, new) = (self.row_count(), keys.len());
        *self.keys.borrow_mut() = keys;
        self.waiting.borrow_mut().clear();
        if new < old {
            self.notify.row_removed(new, old - new);
        } else if new > old {
            self.notify.row_added(old, new - old);
        }
        self.refresh();
    }

    /// Asks for row `i`'s table where the page starts before what its
    /// guide has, within its archive ([`needs_table`]); the selected row
    /// first.
    fn ask_table(&self, i: usize, key: i64, row: &ChannelRow, progs: &[Programme], now: i64) {
        let mut asked = self.asked.borrow_mut();
        let provider_id = self.provider_id.borrow();
        let offset = self.offsets.borrow().get(&*provider_id).copied();
        // Tables are the provider's guide: none for a channel whose guide
        // is a source's.
        let guide_id = match row.guide_id.as_deref() {
            Some(g) if store::is_source_guide(g) => None,
            _ => row.epg_channel_id.as_ref(),
        };
        let (Some(guide_id), Some(offset)) = (
            guide_id.filter(|_| needs_table(progs, self.page.get(), row.archive_days, now)),
            offset,
        ) else {
            asked.remove(&key);
            return;
        };
        let ask = tables::Ask {
            provider_id: provider_id.clone(),
            offset,
            key,
            stream_id: row.stream_id,
            guide_id: guide_id.clone(),
            archive_days: row.archive_days,
            first: self.sel.borrow().as_ref().is_some_and(|(r, _)| *r == i),
        };
        let _ = self.tables.send(ask.clone());
        asked.insert(key, ask);
    }

    /// Row `i` was selected: its table, if it wants one, goes first.
    fn ask_first(&self, i: usize) {
        let Some(key) = self.keys.borrow().get(i).copied() else {
            return;
        };
        if let Some(ask) = self.asked.borrow().get(&key) {
            let _ = self.tables.send(tables::Ask {
                first: true,
                ..ask.clone()
            });
        }
    }

    fn read_marks(&self) {
        let mut marks: HashMap<i64, Vec<_>> = HashMap::new();
        match store::recording_marks(&self.conn, &self.provider_id.borrow()) {
            Ok(rows) => rows.into_iter().for_each(|(stream_id, start, state)| {
                marks.entry(stream_id).or_default().push((start, state));
            }),
            Err(e) => log::error!("reading recordings: {e}"),
        }
        *self.marks.borrow_mut() = marks;
    }

    fn logo_ready(&self, url: &str) {
        let rows = self.waiting.borrow_mut().remove(url);
        for i in rows.into_iter().flatten() {
            if i < self.row_count() {
                self.notify.row_changed(i);
            }
        }
    }

    /// Re-reads every row Slint has built, e.g. when the minute changes or
    /// a recording's state does.
    fn refresh(&self) {
        self.read_marks();
        for i in 0..self.row_count() {
            self.notify.row_changed(i);
        }
    }
}

impl Model for Guide {
    type Data = GuideItem;

    fn row_count(&self) -> usize {
        self.keys.borrow().len()
    }

    fn row_data(&self, i: usize) -> Option<GuideItem> {
        let key = *self.keys.borrow().get(i)?;
        let now = import::unix_now();
        let row = read_channel(&self.conn, key, now)?;
        let page = self.page.get();
        let progs = match row.guide_id.as_deref() {
            Some(epg_id) => store::programmes(
                &self.conn,
                &self.provider_id.borrow(),
                epg_id,
                page.0,
                page.1,
            )
            .unwrap_or_else(|e| {
                log::error!("reading programmes: {e}");
                Vec::new()
            }),
            None => Vec::new(),
        };
        let logo = self.logos.get(row.icon_url.as_deref(), ROW_LOGO);
        if let Some(url) = row.icon_url.as_ref().filter(|_| logo.size().width == 0) {
            self.waiting
                .borrow_mut()
                .entry(url.clone())
                .or_default()
                .push(i);
        }
        self.ask_table(i, key, &row, &progs, now);
        let marks = self.marks.borrow();
        let marks = marks.get(&row.stream_id).map_or(&[][..], Vec::as_slice);
        let blocks = guide_blocks(&progs, self.origin.get(), now, marks, row.archive_days);
        let model = {
            let mut models = self.blocks.borrow_mut();
            // Held for the rows scrolled through; a full cache starts over.
            if models.len() > 1024 && !models.contains_key(&key) {
                models.clear();
            }
            models.entry(key).or_default().clone()
        };
        sync_blocks(&model, blocks);
        Some(GuideItem {
            num: row.num.map(|n| n.to_string()).unwrap_or_default().into(),
            initials: initials(&row.name).into(),
            tile: tile(&row.name),
            name: row.name.into(),
            favourite: row.favourite,
            logo,
            blocks: ModelRc::from(model as Rc<dyn Model<Data = GuideBlock>>),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// The logo fallback (SPEC 8.2): the initials of the first two words that
/// start with an ASCII letter, after a `UK:` or `4K|` style prefix. Other
/// scripts and superscript tags are skipped rather than drawn.
fn initials(name: &str) -> String {
    let of = |s: &str| -> String {
        s.split_whitespace()
            .filter_map(|w| {
                w.trim_start_matches(|c: char| c.is_ascii_punctuation())
                    .chars()
                    .next()
                    .filter(char::is_ascii_alphabetic)
            })
            .take(2)
            .map(|c| c.to_ascii_uppercase())
            .collect()
    };
    let rest = name.split_once([':', '|']).map_or("", |(_, r)| r);
    [of(rest), of(name)]
        .into_iter()
        .find(|i| !i.is_empty())
        .or_else(|| {
            let c = name.chars().find(char::is_ascii_alphabetic)?;
            Some(c.to_ascii_uppercase().to_string())
        })
        .unwrap_or_default()
}

/// The logo tile's colour: one of a muted set, stable per name.
fn tile(name: &str) -> slint::Color {
    const TILES: [(u8, u8, u8); 8] = [
        (0x3b, 0x5b, 0xa9),
        (0x2f, 0x7d, 0x6d),
        (0x8a, 0x4b, 0x9c),
        (0xa6, 0x5a, 0x3a),
        (0x3f, 0x7f, 0x9f),
        (0x9c, 0x3f, 0x5a),
        (0x5c, 0x6b, 0x2f),
        (0x6a, 0x5a, 0xb0),
    ];
    // FNV-1a.
    let h = name.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    });
    let (r, g, b) = TILES[h as usize % TILES.len()];
    slint::Color::from_rgb_u8(r, g, b)
}

/// How long after the last digit typed in mpv the number plays (SPEC 9.2).
const NUMBER_IDLE: Duration = Duration::from_millis(1500);

/// A channel number typed in mpv, one digit at a time. It plays once
/// [`NUMBER_IDLE`] passes without another digit.
#[derive(Default)]
struct NumberEntry {
    digits: String,
    last: Option<Instant>,
}

impl NumberEntry {
    /// Longer than any provider's numbering, short enough to fit an `i64`.
    const MAX_DIGITS: usize = 6;

    /// Adds a digit and returns the number so far. A digit after the entry
    /// went idle starts a new number.
    fn push(&mut self, digit: u8, now: Instant) -> &str {
        if self.due(now) || self.digits.len() == Self::MAX_DIGITS {
            self.digits.clear();
        }
        self.digits.push(char::from(b'0' + digit));
        self.last = Some(now);
        &self.digits
    }

    fn due(&self, now: Instant) -> bool {
        self.last
            .is_some_and(|t| now.duration_since(t) >= NUMBER_IDLE)
    }

    /// The typed number, once idle; clears the entry.
    fn take_due(&mut self, now: Instant) -> Option<i64> {
        if !self.due(now) {
            return None;
        }
        self.last = None;
        std::mem::take(&mut self.digits).parse().ok()
    }
}

/// A favourite whose channel the provider no longer lists (SPEC 5).
const UNAVAILABLE: &str = "No longer in the provider's list";

/// A favourites change. Channels are named by `stream_id`, which, unlike a
/// row key, survives until the next import.
#[derive(Clone, Copy, Debug)]
enum FavCommand {
    Toggle(Kind, i64),
    Move(Kind, i64, isize),
}

/// UI to the writes task, which owns no state but uses the worker's single
/// writer connection (SPEC 3).
enum Write {
    Favourite {
        provider_id: String,
        cmd: FavCommand,
    },
    /// Empty the provider's recently played.
    ClearHistory(String),
    /// Empty the provider's Continue watching (VOD ticket 07).
    ClearContinue(String),
    /// Remember the provider switched to, for the next launch.
    LastProvider(String),
    /// Remember the list order, for the next launch.
    Order(Order),
    /// Change which of the provider's categories show.
    Library {
        provider_id: String,
        edit: LibraryEdit,
    },
    /// Pin a channel's guide, map it, or forget a source's mappings (EPG
    /// sources ticket 05).
    Guide {
        provider_id: String,
        edit: GuideEdit,
    },
    /// A channel, movie or series was played (recently played, `-`).
    Played {
        provider_id: String,
        kind: Kind,
        stream_id: i64,
        at: i64,
    },
    /// Where a movie or episode is (VOD ticket 07).
    Progress(ProgressSave),
    /// Movies or episodes marked watched, or not (`w`).
    Watched {
        provider_id: String,
        items: Vec<Watchable>,
        watched: bool,
    },
}

/// A category allowlist change (ticket 20).
enum LibraryEdit {
    /// Show a hidden category or hide a shown one, by kind and `category_id`.
    Category(Kind, String),
    /// Add the pattern as a rule for the kind, or remove the rule it
    /// already is.
    Rule(Kind, String),
    /// The same for a channel name rule (ticket 25).
    NameRule(String),
    /// Keep a region tag, or stop keeping it (ticket 26).
    KeptTag(String),
}

/// A write the UI redraws after.
enum Written {
    Favourite(FavCommand, Result<bool, String>),
    Played,
    Progress,
    HistoryCleared(Result<(), String>),
    ContinueCleared(Result<(), String>),
    Library(Result<(), String>),
    Guide(Result<(), String>),
}

/// A change to how a channel gets its guide (EPG sources ticket 05).
enum GuideEdit {
    /// Pin `stream_id` to a source with a shift in minutes, or unpin it.
    Pin(i64, Option<(i64, i64)>),
    /// Map `stream_id` to a channel of `source`, or forget its mapping.
    Map {
        source: i64,
        stream_id: i64,
        channel: Option<String>,
    },
    /// Forget the mappings to the source with this ident.
    Forget(String),
}

/// The channel last sent to mpv. Named by provider and `stream_id`, which
/// survive an import; `key` is its row key as of the last read.
struct NowPlaying {
    key: i64,
    provider_id: String,
    stream_id: i64,
    name: String,
    /// When it was played, for "Watching for N min".
    since: i64,
    /// Set while a past programme plays from the archive.
    catch_up: Option<CatchUp>,
}

/// A past programme playing from the archive (catch-up ticket 03).
struct CatchUp {
    programme: Programme,
    /// Where the archive stream starts, Unix seconds: mpv's `time-pos` 0.
    from: i64,
    /// mpv's `time-pos` as last asked ([`tick_position`]).
    pos: f64,
}

impl CatchUp {
    /// How far into the programme the picture is, in seconds.
    fn elapsed(&self) -> i64 {
        let len = self.programme.stop - self.programme.start;
        (self.from + self.pos as i64 - self.programme.start).clamp(0, len)
    }
}

/// The selected channel's other listings in the detail panel (stream
/// recovery ticket 03), for the user to play one instead.
#[derive(Default)]
struct AlternatesPanel {
    /// The channel's row key they are of.
    of: Option<i64>,
    /// Their row keys, as shown.
    keys: Vec<i64>,
    selected: usize,
    /// Keys move in them, not in the list.
    focused: bool,
}

/// The selected series' panel (VOD ticket 05): its episodes, all seasons,
/// read whole as one series' worth is small.
#[derive(Default)]
struct SeriesPanel {
    /// The series' row key; the rest is kept while it stays selected.
    key: Option<i64>,
    /// Whether its info was in when `episodes` was read.
    loaded: bool,
    /// By season, then number.
    episodes: Vec<Episode>,
    /// Those played, by stream id, and where it carries on (VOD ticket
    /// 07).
    progress: HashMap<i64, Progress>,
    /// Its episodes' downloads, by stream id (VOD ticket 10).
    downloads: HashMap<i64, store::Download>,
    up_next: UpNext,
    /// The seasons it has episodes in, ascending, and the one shown.
    seasons: Vec<i64>,
    season: usize,
    /// The selected episode among the season's.
    selected: usize,
    /// Keys move in the episodes rather than the list.
    focused: bool,
}

impl SeriesPanel {
    /// The episodes of the season shown.
    fn in_season(&self) -> impl Iterator<Item = &Episode> {
        let season = self.seasons.get(self.season).copied();
        self.episodes
            .iter()
            .filter(move |e| Some(e.season) == season)
    }
}

/// A movie playing (VOD ticket 04): its position and length are mpv's,
/// asked every second ([`tick_position`]). Where it is is kept every
/// [`SAVE_EVERY`], and when it stops (VOD ticket 07).
struct MoviePlaying {
    name: String,
    /// Its row's year and rating, when played.
    meta: String,
    pos: f64,
    /// mpv's `duration`, once known.
    length: Option<f64>,
    provider_id: String,
    item: Watchable,
    /// Its [`PlayRequest::id`]: mpv's answers count once it is handed on.
    play: u64,
    /// mpv has said where it is since it was played.
    heard: bool,
    /// The position last saved.
    saved: f64,
    /// Played to its end and saved so: nothing more to keep.
    done: bool,
}

/// How often a movie's or episode's position is kept while it plays.
const SAVE_EVERY: f64 = 5.0;

/// The episode to play when the countdown after one ends runs out (VOD
/// ticket 08).
struct NextEpisode {
    series_id: i64,
    /// The series' name.
    name: String,
    episode: Episode,
    due: Instant,
}

/// How long after an episode ends the next one plays.
const NEXT_IN: Duration = Duration::from_secs(10);

/// How long a countdown line stays on mpv's OSD, a little over the second
/// before the next one replaces it.
const OSD_SHOW: u64 = 1500;

/// The episode `d` along from `stream_id` in `episodes` (as
/// [`store::episodes`] orders them): into the next season at a season's
/// end, none past either end.
fn neighbour(episodes: &[Episode], stream_id: i64, d: isize) -> Option<&Episode> {
    let at = episodes.iter().position(|e| e.stream_id == stream_id)?;
    episodes.get(at.checked_add_signed(d)?)
}

/// Where a movie or episode was, for the writes task.
struct ProgressSave {
    provider_id: String,
    item: Watchable,
    pos: f64,
    length: Option<f64>,
    ended: bool,
    at: i64,
}

impl ProgressSave {
    fn write(&self, conn: &Connection) -> rusqlite::Result<()> {
        store::save_progress(
            conn,
            &self.provider_id,
            &self.item,
            self.pos,
            self.length,
            self.ended,
            self.at,
        )
    }
}

/// mpv's state while it runs, as it reports it (SPEC 9.1).
#[derive(Default)]
struct MpvState {
    paused: bool,
    muted: bool,
    fullscreen: bool,
    behind: Behind,
    /// Recording what plays: since when, and to which file.
    recording: Option<(Instant, PathBuf)>,
}

/// Manage mode (ticket 20): every category of one kind (VOD ticket 02),
/// with a filter over them, and for live the channel name rules (ticket 25)
/// and the kept tags (ticket 26).
#[derive(Default)]
struct Manage {
    kind: Kind,
    cats: Vec<ManagedCategory>,
    /// The names in `cats`, folded as rules fold them, for the filter.
    folded: Vec<String>,
    rules: Vec<String>,
    /// Channel name rules, with how many channels each hides.
    name_rules: Vec<(String, usize)>,
    /// Kept region tags, and how many channels they hide in all.
    kept: Vec<String>,
    kept_hidden: usize,
    /// Every channel's category and folded name. Read once, as names change
    /// only with an import, which clears `names_read`.
    names: Vec<(Option<String>, String)>,
    names_read: bool,
    /// Indices into `names` of the channels in shown categories: what a
    /// name rule or kept tags are counted against.
    listed: Vec<usize>,
    /// How many channels All shows, or titles of the kind in shown
    /// categories.
    titles: i64,
    filter: String,
    /// Indices into `cats` of the rows the filter lets through.
    rows: Vec<usize>,
    selected: usize,
}

impl Manage {
    /// Re-applies the filter: a substring, as a rule matches, so the rows are
    /// what the filter would show as a rule.
    fn filter_rows(&mut self) {
        let filter = store::fold(&self.filter);
        self.rows = (0..self.cats.len())
            .filter(|&i| self.folded[i].contains(filter.as_str()))
            .collect();
    }

    /// The selected category's id.
    fn selected_id(&self) -> Option<&str> {
        let &i = self.rows.get(self.selected)?;
        Some(&self.cats[i].id)
    }

    /// What Ctrl+Enter does with the filter; empty if nothing.
    fn rule_action(&self) -> &'static str {
        if self.filter.trim().is_empty() {
            ""
        } else if self
            .rules
            .iter()
            .any(|r| store::fold(r) == store::fold(&self.filter))
        {
            "Remove rule"
        } else if self.rules.is_empty() {
            "Show only matching"
        } else {
            "Also show matching"
        }
    }

    /// How many channels in shown categories `pattern` hides as a name rule.
    fn hidden_by(&self, pattern: &str) -> usize {
        let rule = store::fold(pattern);
        self.listed
            .iter()
            .filter(|&&i| store::hides(&self.names[i].1, &rule))
            .count()
    }

    /// How many channels in shown categories `kept` hides as kept tags.
    fn hidden_by_kept(&self, kept: &[String]) -> usize {
        self.listed
            .iter()
            .filter(|&&i| !store::tag_shown(store::region_tag(&self.names[i].1), kept))
            .count()
    }

    /// What Ctrl+K does with the filter; empty unless it is a tag.
    fn kept_action(&self) -> String {
        let Some(tag) = store::kept_tag(&self.filter) else {
            return String::new();
        };
        if self.kept.contains(&tag) {
            return format!("Stop keeping “{tag}”");
        }
        let mut kept = self.kept.clone();
        kept.push(tag.clone());
        let hidden = self.hidden_by_kept(&kept);
        if self.kept.is_empty() {
            format!("Keep tag “{tag}” · hides {hidden}")
        } else {
            format!("Also keep “{tag}” · shows {}", self.kept_hidden - hidden)
        }
    }

    /// What Ctrl+H does with the filter; empty if nothing.
    fn name_rule_action(&self) -> String {
        if self.filter.trim().is_empty() {
            String::new()
        } else if self
            .name_rules
            .iter()
            .any(|(r, _)| store::fold(r) == store::fold(&self.filter))
        {
            format!("Unhide names with “{}”", self.filter)
        } else {
            format!(
                "Hide names with “{}” · {}",
                self.filter,
                self.hidden_by(&self.filter)
            )
        }
    }
}

/// `1 channel`, `12 channels`.
/// What a kind's rows are called, plural.
fn noun(kind: Kind) -> &'static str {
    match kind {
        Kind::Live => "channels",
        Kind::Movie => "movies",
        Kind::Series => "series",
    }
}

/// `1 movie`, `2 series`.
fn count_of(n: usize, kind: Kind) -> String {
    match (n, kind) {
        (1, Kind::Live) => "1 channel".into(),
        (1, Kind::Movie) => "1 movie".into(),
        _ => format!("{n} {}", noun(kind)),
    }
}

/// The kind as the switch names it.
fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Live => "Live",
        Kind::Movie => "Movies",
        Kind::Series => "Series",
    }
}

/// The kind switch's position for `kind`, as `app.slint` numbers it.
fn kind_index(kind: Kind) -> i32 {
    Kind::ALL.iter().position(|&k| k == kind).unwrap_or(0) as i32
}

fn count(n: i64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Each kind's best matches a widened search lists (VOD ticket 09).
const WIDE_CAP: usize = 50;

/// How many recently played channels the detail panel lists.
const RECENT_SHOWN: usize = 4;
/// Other listings the detail panel lists at most.
const ALTERNATES_MAX: usize = 50;

/// How long the first press of clear waits for the second (SPEC 8.3).
const CLEAR_CONFIRM: Duration = Duration::from_secs(4);

/// One configured provider (ticket 10): the client for its stream URLs, its
/// refresh scheduler's `r` channel, and its latest status for the footer.
/// Each provider refreshes on its own schedule, active or not.
struct Source {
    provider: Provider,
    client: Arc<Client>,
    refresh_tx: mpsc::UnboundedSender<Request>,
    /// The latest account check, for the status bar and the dashboard.
    info: Option<AccountInfo>,
    refresh: String,
    refresh_error: bool,
    /// A refresh is running.
    busy: bool,
    /// The stages of the refresh running or last run, for the refresh
    /// panel.
    stages: Stages,
    /// Its channels were never imported: the first-run screen shows in
    /// place of the main screen (SPEC 7.2).
    first_run: bool,
    /// Its refresh scheduler, stopped when it is removed (onboarding
    /// ticket 02).
    refresher: tokio::task::AbortHandle,
}

/// What starting a provider's background work needs: kept, so a provider
/// added in Settings starts as those read at launch do (onboarding ticket
/// 01).
struct Starter {
    rt: Handle,
    writer: import::Writer,
    /// Shared with the recorder, guide tables, info and logos.
    clients: xtream::Clients,
    /// Every provider's refresh updates, by provider id.
    updates: mpsc::UnboundedSender<(String, Update)>,
}

impl Starter {
    /// Starts `provider`'s refresh scheduler on `client` and makes the
    /// client known to the other tasks.
    fn start(
        &self,
        provider: Provider,
        client: Arc<Client>,
        max_age: i64,
        first_run: bool,
    ) -> Source {
        let (refresh_tx, refresh_rx) = mpsc::unbounded_channel();
        let (updates_tx, mut updates_rx) = mpsc::unbounded_channel();
        let refresher = self
            .rt
            .spawn(
                Refresher {
                    client: client.clone(),
                    writer: self.writer.clone(),
                    provider: provider.clone(),
                    max_age,
                    check_every: refresh::CHECK_EVERY,
                    updates: updates_tx,
                }
                .run(refresh_rx),
            )
            .abort_handle();
        let (id, all) = (provider.id.clone(), self.updates.clone());
        self.rt.spawn(async move {
            while let Some(update) = updates_rx.recv().await {
                if all.send((id.clone(), update)).is_err() {
                    break;
                }
            }
        });
        self.clients.insert(provider.id.clone(), client.clone());
        Source {
            provider,
            client,
            refresh_tx,
            info: None,
            refresh: String::new(),
            refresh_error: false,
            busy: false,
            stages: Stages::new(),
            first_run,
            refresher,
        }
    }
}

/// Whether a provider opens on the first-run screen: its channels were
/// never imported (SPEC 7.2).
fn first_run(conn: &rusqlite::Connection, provider_id: &str) -> rusqlite::Result<bool> {
    Ok(store::last_refreshes(conn, provider_id)?.0.is_none())
}

/// The first-run screen after `update`: whether it still shows, and
/// whether it hands over, opening the refresh panel on the stages left
/// (SPEC 7.2). It shows until the channels stage is done; a failure
/// before then leaves it showing.
fn first_run_after(showing: bool, update: &Update) -> (bool, bool) {
    match update {
        Update::Channels(_) if showing => (false, true),
        _ => (showing, false),
    }
}

/// How long the refresh panel stays after a refresh succeeds (SPEC 8.2).
const PANEL_CLOSE: Duration = Duration::from_secs(4);

/// What the refresh panel hears of (SPEC 8.2).
#[derive(Debug)]
enum PanelEvent<'a> {
    /// `r`, or a click on the sweep line.
    Asked,
    /// Esc or its ×.
    Dismissed,
    Update(&'a Update),
}

/// The refresh panel after `event`, for the provider shown: whether it is
/// open, and whether it closes by itself after [`PANEL_CLOSE`]. `None`
/// leaves both as they are. It opens when asked and on a failure with
/// stages to show, so a background refresh stays on the sweep line. Its
/// state never reaches the refresher: dismissing it leaves the refresh
/// running.
fn refresh_panel(open: bool, event: PanelEvent, has_stages: bool) -> Option<(bool, bool)> {
    match event {
        PanelEvent::Asked => Some((true, false)),
        PanelEvent::Dismissed => Some((false, false)),
        PanelEvent::Update(Update::Idle { error: Some(_), .. }) => {
            Some((open || has_stages, false))
        }
        PanelEvent::Update(Update::Idle { error: None, .. }) => Some((open, open)),
        // Another refresh started: stay for it.
        PanelEvent::Update(Update::Stages(stages))
            if stages
                .iter()
                .any(|(_, s)| matches!(s, StageState::Waiting | StageState::Running { .. })) =>
        {
            Some((open, false))
        }
        PanelEvent::Update(_) => None,
    }
}

/// `58k`, `1.2k`, `640`.
fn count_short(n: usize) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..9950 => format!("{:.1}k", n as f64 / 1000.0),
        _ => format!("{}k", (n + 500) / 1000),
    }
}

/// The refresh panel's rows at `now`. `provider` is stripped from errors,
/// as the panel names the provider.
fn stage_items(stages: &Stages, provider: &str, now: i64) -> Vec<StageItem> {
    let prefix = format!("{provider}: ");
    stages
        .iter()
        .map(|(stage, state)| {
            let (state, tone, reason) = match state {
                StageState::Waiting | StageState::Running { .. }
                    if matches!(stage, Stage::Source { reimport: true, .. }) =>
                {
                    let (state, tone) = match state {
                        StageState::Running { mb, .. } if *mb > 0 => {
                            (format!("▸ {mb} MB"), StageTone::Running)
                        }
                        StageState::Running { .. } => ("▸".into(), StageTone::Running),
                        _ => ("waiting".into(), StageTone::Waiting),
                    };
                    (state, tone, "re-importing for new mappings".to_owned())
                }
                StageState::Waiting => ("waiting".into(), StageTone::Waiting, String::new()),
                StageState::Running {
                    percent: Some(p), ..
                } => (format!("▸ {p} %"), StageTone::Running, String::new()),
                StageState::Running { mb: 0, .. } => {
                    ("▸".into(), StageTone::Running, String::new())
                }
                StageState::Running { mb, .. } => {
                    (format!("▸ {mb} MB"), StageTone::Running, String::new())
                }
                StageState::Done(n) => (
                    format!("✓ {}", count_short(*n)),
                    StageTone::Done,
                    String::new(),
                ),
                StageState::Unchanged => ("✓ unchanged".into(), StageTone::Done, String::new()),
                StageState::AtCap { count, max, next } => (
                    "at its cap".into(),
                    StageTone::Warn,
                    format!("{count}/{max} in 24 h, next {}", clock(*next)),
                ),
                // A source's `Retry-After` can be hours off.
                StageState::Backoff { retry_at } if matches!(stage, Stage::Source { .. }) => (
                    format!("busy until {}", clock(*retry_at)),
                    StageTone::Warn,
                    String::new(),
                ),
                StageState::Backoff { retry_at } => {
                    let left = (retry_at - now).max(0);
                    let text = format!("provider busy, retry in {}:{:02}", left / 60, left % 60);
                    (text, StageTone::Warn, String::new())
                }
                StageState::Failed(e) => {
                    let e = e.strip_prefix(&prefix).unwrap_or(e);
                    ("failed".into(), StageTone::Warn, e.to_owned())
                }
            };
            StageItem {
                name: stage.name().into(),
                state: state.into(),
                tone,
                reason: reason.into(),
                source: matches!(stage, Stage::Source { .. }),
            }
        })
        .collect()
}

/// Shown instead of a programme for channels the guide does not cover.
const NO_EPG: &str = "No programme information";

/// How far through `p` the time `now` is, 0 to 1.
fn progress(p: &Programme, now: i64) -> f32 {
    ((now - p.start) as f32 / (p.stop - p.start).max(1) as f32).clamp(0.0, 1.0)
}

/// `14:05 – 15:30 · 42 min left`, in local time.
fn now_times(p: &Programme, now: i64) -> String {
    format!("{} · {}", span(p), time_left(p, now))
}

/// `14:05 – 15:30`, in local time.
fn span(p: &Programme) -> String {
    format!("{} – {}", clock(p.start), clock(p.stop))
}

/// `42 min left`, `1 h 5 min left`.
fn time_left(p: &Programme, now: i64) -> String {
    format!("{} left", duration(p.stop - now))
}

/// `42 min`, `1 h 5 min`, `2 days 3 h`, rounded up to the minute.
fn duration(secs: i64) -> String {
    let m = (secs + 59) / 60;
    match (m / 1440, m / 60 % 24, m % 60) {
        (0, 0, m) => format!("{m} min"),
        (0, h, 0) => format!("{h} h"),
        (0, h, m) => format!("{h} h {m} min"),
        (1, 0, _) => "1 day".into(),
        (1, h, _) => format!("1 day {h} h"),
        (d, 0, _) => format!("{d} days"),
        (d, h, _) => format!("{d} days {h} h"),
    }
}

/// The guide grid's detail strip times (SPEC 8.2): the day unless today,
/// the times, then time left, when it starts, or when it ended.
/// `Tomorrow · 20:00 – 21:00 · starts in 1 day 2 h`.
fn guide_times(p: &Programme, now: i64) -> String {
    let day = day_prefix(p.start, now);
    let when = if p.stop <= now {
        format!("ended {} ago", duration(now - p.stop))
    } else if p.start <= now {
        time_left(p, now)
    } else {
        format!("starts in {}", duration(p.start - now))
    };
    format!("{day}{} · {when}", span(p))
}

/// The day of `t` before its times, unless today: `Tomorrow · `,
/// `Yesterday · `, `Sat 3 Oct · `.
fn day_prefix(t: i64, now: i64) -> String {
    match days_between(t, now) {
        0 => String::new(),
        1 => "Tomorrow · ".into(),
        -1 => "Yesterday · ".into(),
        _ => {
            let tm = local_tm(t);
            let wday = WEEKDAYS[tm.tm_wday.clamp(0, 6) as usize];
            let month = MONTHS[tm.tm_mon.clamp(0, 11) as usize];
            format!("{wday} {} {month} · ", tm.tm_mday)
        }
    }
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// mpv's title (SPEC 9.1): `<num> <channel name>`, with ` · <programme>`
/// while catch-up plays it. Never the URL.
fn media_title(num: Option<i64>, name: &str, replayed: Option<&str>) -> String {
    let mut title = match num {
        Some(n) => format!("{n} {name}"),
        None => name.to_string(),
    };
    if let Some(p) = replayed {
        title = format!("{title} · {p}");
    }
    title
}

/// The NOW PLAYING card's heading while catch-up plays (SPEC 8.2).
fn catch_up_heading(paused: bool) -> &'static str {
    if paused {
        "PAUSED · CATCH-UP"
    } else {
        "CATCH-UP"
    }
}

/// The NOW PLAYING card's heading (SPEC 8.2): behind live in whole minutes.
/// Paused under a minute behind is plain `PAUSED`, as every pause is behind.
fn playing_heading(paused: bool, behind: Duration) -> String {
    let by = match behind.as_secs() / 60 {
        0 if paused => return "PAUSED".into(),
        _ if behind.is_zero() => return "NOW PLAYING".into(),
        0 => "<1 min".to_string(),
        m => format!("{m} min"),
    };
    if paused {
        format!("PAUSED · {by} behind")
    } else {
        format!("BEHIND LIVE · {by}")
    }
}

/// The NOW PLAYING card's line for a channel without a guide.
fn watching(since: i64, now: i64) -> String {
    match (now - since) / 60 {
        ..=0 => "Just started".into(),
        m => format!("Watching for {m} min"),
    }
}

/// The grid's detail strip on a scheduled or recording programme (SPEC
/// 8.2): `● Recording 19:58 – 21:05`, `○ Scheduled to record 19:58 – 21:05`,
/// the padded times.
fn record_detail(r: &store::Recording) -> String {
    let what = match r.state {
        store::RecState::Recording => "● Recording",
        _ => "○ Scheduled to record",
    };
    format!(
        "{what} {} – {}",
        clock(r.padded_start),
        clock(r.padded_stop)
    )
}

/// The recording indicator (SPEC 8.2): `● REC 12:04`, `● REC 1:05:30`.
fn rec_elapsed(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    match s / 3600 {
        0 => format!("● REC {}:{:02}", s / 60, s % 60),
        h => format!("● REC {h}:{:02}:{:02}", s / 60 % 60, s % 60),
    }
}

/// A recording as the status bar names it: `Football Tonight on
/// Northwind Sport 1`, or the channel alone without a guide.
fn rec_what(r: &store::Recording) -> String {
    match &r.title {
        Some(title) => format!("{title} on {}", r.channel),
        None => r.channel.clone(),
    }
}

/// A line of the Recordings view (recording ticket 04): a group's heading,
/// or a recording.
#[derive(Debug, PartialEq)]
enum RecLine {
    Heading(&'static str),
    Rec(store::Recording),
    /// On the Downloads tab (VOD ticket 10).
    Dl(store::Download),
}

/// The Recordings view's last group, which `c` clears.
const HISTORY: &str = "HISTORY";
/// The Downloads tab's, likewise.
const FINISHED: &str = "FINISHED";

/// Heights of the Recordings view's lines, as `app.slint` draws them.
const REC_HEADING_HEIGHT: f32 = 36.0;
const REC_ROW_HEIGHT: f32 = 56.0;

impl RecLine {
    fn height(&self) -> f32 {
        match self {
            Self::Heading(_) => REC_HEADING_HEIGHT,
            Self::Rec(_) | Self::Dl(_) => REC_ROW_HEIGHT,
        }
    }
}

/// The Recordings view's groups: now recording, by start; scheduled,
/// soonest first; finished (done, failed, missed, cancelled), latest
/// first. An empty group is left out, heading and all, as are trashed
/// recordings whose file is gone.
fn recording_lines(rows: Vec<store::Recording>) -> Vec<RecLine> {
    use store::RecState;
    let (mut recording, rest): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .filter(store::recording_shown)
        .partition(|r| r.state == RecState::Recording);
    let (mut scheduled, mut finished): (Vec<_>, Vec<_>) = rest
        .into_iter()
        .partition(|r| r.state == RecState::Scheduled);
    recording.sort_by_key(|r| (r.padded_start, r.id));
    scheduled.sort_by_key(|r| (r.padded_start, r.id));
    finished.sort_by_key(|r| std::cmp::Reverse((r.padded_start, r.id)));
    let mut lines = Vec::new();
    for (heading, group) in [
        ("NOW RECORDING", recording),
        ("SCHEDULED", scheduled),
        (HISTORY, finished),
    ] {
        if !group.is_empty() {
            lines.push(RecLine::Heading(heading));
            lines.extend(group.into_iter().map(RecLine::Rec));
        }
    }
    lines
}

/// When a recording is or was: `Today 20:00 – 21:00`, `Sat 3 Oct 20:00 –
/// 21:00`, the guide's times rather than the padded ones. One of what plays
/// has no stop until it ends: `Today from 20:04`.
fn rec_times(r: &store::Recording, now: i64) -> String {
    let day = match days_between(r.start, now) {
        0 => "Today".to_string(),
        1 => "Tomorrow".into(),
        -1 => "Yesterday".into(),
        _ => {
            let tm = local_tm(r.start);
            let wday = WEEKDAYS[tm.tm_wday.clamp(0, 6) as usize];
            let month = MONTHS[tm.tm_mon.clamp(0, 11) as usize];
            format!("{wday} {} {month}", tm.tm_mday)
        }
    };
    if r.stop <= r.start && r.state == store::RecState::Recording {
        format!("{day} from {}", clock(r.start))
    } else {
        format!("{day} {} – {}", clock(r.start), clock(r.stop))
    }
}

/// A file's size: `640 KB`, `850 MB`, `1.2 GB`.
fn file_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} KB", (b / 1e3).ceil())
    }
}

/// The status bar after Clear in the Recordings view: how many recordings
/// (or downloads) left the list, and whether files stay on disk, only when some had one.
fn cleared_message(n: usize, with_file: usize, noun: &str) -> String {
    let cleared = format!("Cleared {} from the list", count(n as i64, noun));
    match with_file {
        0 => cleared,
        1 if n == 1 => format!("{cleared}; its file is kept"),
        k if k == n => format!("{cleared}; their files are kept"),
        1 => format!("{cleared}; the 1 file is kept"),
        k => format!("{cleared}; the {k} files are kept"),
    }
}

/// The status bar once `d` or `D` has queued what it asked for, `what`,
/// a season's name with `season`.
fn queued_message(what: &str, season: bool, q: store::Queued) -> String {
    match (season, q.added, q.already) {
        (false, 0, _) => format!("{what} is downloaded or queued already"),
        (false, _, _) => format!("Queued {what} to download"),
        (true, 0, _) => format!("Every episode of {what} is downloaded or queued already"),
        (true, n, 0) => format!(
            "Queued {} of {what} to download",
            count(n as i64, "episode")
        ),
        (true, n, k) => format!(
            "Queued {} of {what} to download; {k} already were",
            count(n as i64, "episode")
        ),
    }
}

/// The last part of a recording's file path.
fn file_name(file: &str) -> std::borrow::Cow<'_, str> {
    std::path::Path::new(file)
        .file_name()
        .map_or(file.into(), |n| n.to_string_lossy())
}

/// The status bar after `d` in the Recordings view (recording ticket 05),
/// and whether it is a problem.
fn deleted_message(
    r: &store::Recording,
    deleted: &recorder::Deleted,
    dir: &std::path::Path,
) -> (String, bool) {
    use recorder::Deleted::*;
    let name = || file_name(r.file.as_deref().unwrap_or_default()).into_owned();
    match deleted {
        Trashed => (format!("Moved {} to the trash", name()), false),
        NoFile if r.file.is_some() => (
            format!(
                "Removed {} from the list; its file was already gone",
                rec_what(r)
            ),
            false,
        ),
        NoFile => (
            format!("Removed {} from the list; it had no file", rec_what(r)),
            false,
        ),
        Elsewhere => (
            format!(
                "{} is not in {}, so d leaves it alone",
                name(),
                dir.display()
            ),
            false,
        ),
        NotFinished => (
            "d deletes a finished recording; x cancels this one".into(),
            false,
        ),
        Failed(e) => (format!("Can't move {} to the trash: {e}", name()), true),
    }
}

/// A recording's line in the Recordings view, its file's size read now.
fn recording_item(r: &store::Recording, now: i64) -> RecordingItem {
    use store::RecState::*;
    let size = || {
        r.file
            .as_ref()
            .map(|f| std::fs::metadata(f).map(|m| file_size(m.len())))
    };
    let (state, tone) = match r.state {
        Recording => ("● Recording".to_string(), RecTone::Recording),
        Scheduled => ("Scheduled".into(), RecTone::None),
        Done => match size() {
            Some(Ok(size)) => (size, RecTone::None),
            Some(Err(_)) => ("File missing".into(), RecTone::Warn),
            None => ("Done".into(), RecTone::None),
        },
        Failed => ("Failed".into(), RecTone::Warn),
        Missed => ("Missed".into(), RecTone::Warn),
        Cancelled => match size() {
            Some(Ok(size)) => (format!("Cancelled · {size}"), RecTone::Dim),
            _ => ("Cancelled".into(), RecTone::Dim),
        },
    };
    let note = match (r.state, &r.note) {
        (_, Some(note)) => note.clone(),
        (Missed, None) => "the app was not running".into(),
        _ => String::new(),
    };
    let sub = match &r.title {
        Some(_) => format!("{} · {}", r.channel, rec_times(r, now)),
        None => rec_times(r, now),
    };
    RecordingItem {
        id: r.id as i32,
        heading: "".into(),
        title: r.title.as_deref().unwrap_or(&r.channel).into(),
        sub: sub.into(),
        note: note.into(),
        state: state.into(),
        tone,
        progress: -1.0,
    }
}

/// A download's mark in the detail panel (VOD ticket 10), and whether it
/// is a problem: `↓ Downloaded`, `↓ 41 %` while it runs, `↓ Queued`,
/// `↓ Failed`; nothing without one, or for one done whose file is gone.
fn download_mark(d: Option<&store::Download>) -> (String, bool) {
    use store::DlState::*;
    let Some(d) = d else {
        return (String::new(), false);
    };
    match d.state {
        Done if d
            .file
            .as_deref()
            .is_some_and(|f| std::path::Path::new(f).exists()) =>
        {
            ("↓ Downloaded".into(), false)
        }
        Done => (String::new(), false),
        Downloading => match download_part(d) {
            Some(part) => (format!("↓ {} %", (part * 100.0) as u32), false),
            None => ("↓ Downloading".into(), false),
        },
        Queued => ("↓ Queued".into(), false),
        Failed => ("↓ Failed".into(), true),
    }
}

/// How far download `d` is, 0 to 1, once its size is known.
fn download_part(d: &store::Download) -> Option<f32> {
    let total = d.total.filter(|t| *t > 0)?;
    Some((d.have() as f32 / total as f32).clamp(0.0, 1.0))
}

/// A download's episode, as the NOW PLAYING card says it: `S2 E4  ·  The
/// Night Ferry`; empty for a movie.
fn download_episode(d: &store::Download) -> String {
    let Some(season) = d.season.filter(|_| d.series_id.is_some()) else {
        return String::new();
    };
    let label = match d.episode_num {
        Some(n) => format!("S{season} E{n}"),
        None => format!("S{season}"),
    };
    match d.episode.as_deref().filter(|e| !e.is_empty()) {
        Some(name) => format!("{label}  ·  {name}"),
        None => label,
    }
}

/// A download as the status bar names it: the movie, or `Glass Kings S2
/// E4`.
fn dl_what(d: &store::Download) -> String {
    match d.season.filter(|_| d.series_id.is_some()) {
        Some(season) => match d.episode_num {
            Some(n) => format!("{} S{season} E{n}", d.title),
            None => format!("{} S{season}", d.title),
        },
        None => d.title.clone(),
    }
}

/// The Downloads tab's groups (VOD ticket 10): the one running, the queue
/// in its order, then done and failed ones, latest first. An empty group
/// is left out, heading and all, as are trashed downloads whose file is
/// gone.
fn download_lines(rows: Vec<store::Download>) -> Vec<RecLine> {
    use store::DlState;
    let (running, rest): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .filter(store::download_shown)
        .partition(|d| d.state == DlState::Downloading);
    let (queued, mut finished): (Vec<_>, Vec<_>) =
        rest.into_iter().partition(|d| d.state == DlState::Queued);
    finished.sort_by_key(|d| std::cmp::Reverse((d.ended, d.id)));
    let mut lines = Vec::new();
    for (heading, group) in [
        ("DOWNLOADING", running),
        ("QUEUED", queued),
        (FINISHED, finished),
    ] {
        if !group.is_empty() {
            lines.push(RecLine::Heading(heading));
            lines.extend(group.into_iter().map(RecLine::Dl));
        }
    }
    lines
}

/// A download's line on the Downloads tab, its part's size read now. The
/// first queued one, with none running, says why it waits.
fn download_item(d: &store::Download, waits: bool) -> RecordingItem {
    use store::DlState::*;
    let have = d.have();
    let of = |have: u64| match d.total.filter(|t| *t > 0) {
        Some(total) => format!("{} of {}", file_size(have), file_size(total as u64)),
        None => file_size(have),
    };
    let (state, tone, note) = match d.state {
        Downloading => match download_part(d) {
            Some(part) => (
                format!("↓ {} %", (part * 100.0) as u32),
                RecTone::Active,
                of(have),
            ),
            None => ("↓ Downloading".into(), RecTone::Active, of(have)),
        },
        Queued => {
            let note = match (have, waits) {
                (0, true) => "waiting for a free connection".into(),
                (0, false) => String::new(),
                (have, true) => format!("{} · waiting for a free connection", of(have)),
                (have, false) => of(have),
            };
            ("Queued".into(), RecTone::None, note)
        }
        Done => match d.file.as_ref().map(std::fs::metadata) {
            Some(Ok(m)) => (file_size(m.len()), RecTone::None, String::new()),
            _ => ("File missing".into(), RecTone::Warn, String::new()),
        },
        Failed => (
            "Failed".into(),
            RecTone::Warn,
            d.note.clone().unwrap_or_default(),
        ),
    };
    let episode = download_episode(d);
    RecordingItem {
        id: d.id as i32,
        heading: "".into(),
        title: d.title.as_str().into(),
        sub: if episode.is_empty() {
            "Movie".into()
        } else {
            episode.into()
        },
        note: note.into(),
        state: state.into(),
        tone,
        progress: match d.state {
            Downloading => download_part(d).unwrap_or(0.0),
            _ => -1.0,
        },
    }
}

/// The status bar after `d` on the Downloads tab, and whether it is a
/// problem.
fn dl_deleted_message(
    d: &store::Download,
    deleted: &recorder::Deleted,
    dir: &std::path::Path,
) -> (String, bool) {
    use recorder::Deleted::*;
    let name = || file_name(d.file.as_deref().unwrap_or_default()).into_owned();
    match deleted {
        Trashed => (format!("Moved {} to the trash", name()), false),
        NoFile if d.state == store::DlState::Failed => (
            format!("Removed {} from the list, and its partial file", dl_what(d)),
            false,
        ),
        NoFile => (
            format!(
                "Removed {} from the list; its file was already gone",
                dl_what(d)
            ),
            false,
        ),
        Elsewhere => (
            format!(
                "{} is not in {}, so d leaves it alone",
                name(),
                dir.display()
            ),
            false,
        ),
        NotFinished => (
            "d deletes a finished download; x cancels this one".into(),
            false,
        ),
        Failed(e) => (format!("Can't move {} to the trash: {e}", name()), true),
    }
}

pub(crate) fn local_tm(t: i64) -> libc::tm {
    let t = t as libc::time_t;
    // SAFETY: `tm` is plain data, and `localtime_r` fills it in.
    unsafe {
        let mut tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    }
}

/// `HH:MM` in local time.
fn clock(t: i64) -> String {
    let tm = local_tm(t);
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `3 Mar 2027` in local time.
fn date(t: i64) -> String {
    let tm = local_tm(t);
    let month = MONTHS[tm.tm_mon.clamp(0, 11) as usize];
    format!("{} {month} {}", tm.tm_mday, tm.tm_year + 1900)
}

/// `14:05` today, else `28 Sep 14:05`, in local time.
fn when(t: i64, now: i64) -> String {
    let (tm, today) = (local_tm(t), local_tm(now));
    if (tm.tm_year, tm.tm_yday) == (today.tm_year, today.tm_yday) {
        clock(t)
    } else {
        let month = MONTHS[tm.tm_mon.clamp(0, 11) as usize];
        format!("{} {month} {}", tm.tm_mday, clock(t))
    }
}

/// The account's expiry (SPEC 6.3) and how urgent it is. Within 7 days the
/// expiry is relative; 4 to 7 days warn, closer is danger. An expiry that
/// passes while running counts as expired before the next check. The status
/// bar shows it only when urgent.
fn account_text(a: &AccountInfo, now: i64) -> (String, Urgency) {
    match (&a.blocked, a.expires()) {
        (Some(Blocked::Disabled(status)), _) => (
            format!("Account {}", status.to_lowercase()),
            Urgency::Danger,
        ),
        (Some(Blocked::Expired), None) => ("Account expired".into(), Urgency::Danger),
        (_, Some(t)) if a.blocked.is_some() || t <= now => {
            (format!("Expired {}", date(t)), Urgency::Danger)
        }
        (_, Some(t)) => match days_between(t, now) {
            0 => (format!("Expires today {}", clock(t)), Urgency::Danger),
            1 => (format!("Expires tomorrow {}", clock(t)), Urgency::Danger),
            d @ 2..=3 => (format!("Expires in {d} days"), Urgency::Danger),
            d @ 4..=7 => (format!("Expires in {d} days"), Urgency::Warn),
            _ => (format!("Expires {}", date(t)), Urgency::None),
        },
        (None, None) => ("No expiry".into(), Urgency::None),
    }
}

/// The account dashboard's contents (ticket 21), from the last check if
/// there was one. Never the credentials: those are revealed one by one.
fn account_view(name: &str, a: Option<&AccountInfo>, checking: bool, now: i64) -> AccountView {
    let checked = match (checking, a) {
        (true, _) => "Checking…".to_string(),
        (false, Some(a)) => format!("Checked {}", when(a.checked, now)),
        (false, None) => "Not checked yet".into(),
    };
    let mut view = AccountView {
        title: name.into(),
        note: format!("{checked} · Esc to close").into(),
        ..Default::default()
    };
    let Some(a) = a else { return view };
    let (u, s) = (&a.user, &a.server);
    // Like the status bar, an expiry passed since the check counts.
    let passed = a.expires().is_some_and(|t| t <= now);
    let status = match &a.blocked {
        Some(Blocked::Disabled(s)) => Some(s.clone()),
        Some(Blocked::Expired) => Some("Expired".to_string()),
        None if passed => Some("Expired".to_string()),
        None => u.status.clone(),
    };
    view.status = status.unwrap_or_default().into();
    view.ok = a.blocked.is_none() && !passed;
    view.message = u.message.as_deref().unwrap_or("").trim().into();
    view.expires = match a.expires() {
        Some(t) => format!("{} · {}", date(t), days_from(t, now)),
        None => "Never".into(),
    }
    .into();
    view.created = u
        .created_at
        .filter(|&t| t > 0)
        .map(date)
        .unwrap_or_default()
        .into();
    view.trial = match u.is_trial {
        Some(true) => "Yes",
        Some(false) => "No",
        None => "",
    }
    .into();
    view.connections = match (u.active_cons, u.max_connections) {
        (Some(n), Some(max)) => format!("{n} of {max} in use"),
        (None, Some(max)) => format!("{max} allowed"),
        (Some(n), None) => format!("{n} in use"),
        (None, None) => String::new(),
    }
    .into();
    if !u.allowed_output_formats.is_empty() {
        view.formats = format!(
            "{} (using {})",
            u.allowed_output_formats.join(", "),
            u.stream_extension()
        )
        .into();
    }
    view.timezone = s.timezone.clone().unwrap_or_default().into();
    view.server_time = s.time_now.clone().unwrap_or_default().into();
    view.ports = [
        ("HTTP", &s.port),
        ("HTTPS", &s.https_port),
        ("RTMP", &s.rtmp_port),
    ]
    .into_iter()
    .filter_map(|(name, port)| Some(format!("{name} {}", port.as_deref()?)))
    .collect::<Vec<_>>()
    .join(" · ")
    .into();
    view.protocol = s.server_protocol.clone().unwrap_or_default().into();
    view
}

/// `in 160 days`, `today`, `3 days ago`, by local calendar day.
fn days_from(t: i64, now: i64) -> String {
    match days_between(t, now) {
        0 => "today".into(),
        1 => "tomorrow".into(),
        -1 => "yesterday".into(),
        d if d > 0 => format!("in {d} days"),
        d => format!("{} days ago", -d),
    }
}

/// Local calendar days from `now` to `t`: 0 today, 1 tomorrow, -1 yesterday.
fn days_between(t: i64, now: i64) -> i64 {
    // Rounded, as a DST change makes a day 23 or 25 hours.
    ((end_of_day(t) - end_of_day(now)) as f64 / 86_400.0).round() as i64
}

/// The sidebar's refresh line when nothing is running (SPEC 7.2).
fn idle_text(updated: Option<i64>, error: Option<&str>, retry_at: Option<i64>, now: i64) -> String {
    let updated = match updated {
        Some(t) => format!("Updated {}", when(t, now)),
        None => "Not updated yet".into(),
    };
    match (error, retry_at) {
        (None, _) => updated,
        (Some(e), None) => format!("{e} · {updated}"),
        (Some(e), Some(t)) => format!("{e} · retrying at {} · {updated}", clock(t)),
    }
}

/// The next local midnight after `t`.
fn end_of_day(t: i64) -> i64 {
    let mut tm = local_tm(t);
    (tm.tm_hour, tm.tm_min, tm.tm_sec) = (0, 0, 0);
    tm.tm_mday += 1;
    tm.tm_isdst = -1;
    // SAFETY: `mktime` normalises the fields of a valid `tm`.
    unsafe { libc::mktime(&mut tm) as i64 }
}

/// Redraws what depends on the clock just after each minute boundary, and
/// not otherwise, so the progress bars advance without per-frame work.
fn tick_every_minute(screen: Rc<RefCell<Screen>>) {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let wait = Duration::from_secs(60 - since.as_secs() % 60)
        - Duration::from_nanos(since.subsec_nanos().into());
    slint::Timer::single_shot(wait + Duration::from_millis(50), move || {
        let mut s = screen.borrow_mut();
        if s.setup.is_none() {
            s.refresh_times();
        }
        drop(s);
        tick_every_minute(screen);
    });
}

/// While paused, redraws the NOW PLAYING heading just after behind live
/// crosses each whole minute; otherwise nothing runs.
fn tick_behind(screen: &Rc<RefCell<Screen>>) {
    let s = screen.borrow();
    let behind = s.mpv.as_ref().map(|m| m.behind).filter(Behind::paused);
    let Some(behind) = behind else {
        s.behind_timer.stop();
        return;
    };
    let past = behind.at(Instant::now()).as_millis() % 60_000;
    let wait = Duration::from_millis((60_000 - past) as u64 + 50);
    let weak = Rc::downgrade(screen);
    s.behind_timer
        .start(slint::TimerMode::SingleShot, wait, move || {
            if let Some(screen) = weak.upgrade() {
                screen.borrow_mut().show_playing();
                tick_behind(&screen);
            }
        });
}

/// While catch-up or a movie plays, asks mpv where the picture is every second,
/// for the NOW PLAYING card; otherwise nothing runs. Each answer redraws
/// the card ([`Screen::on_player`]).
fn tick_position(screen: &Rc<RefCell<Screen>>) {
    let s = screen.borrow();
    let catching_up = s.mpv.is_some()
        && (s.movie.is_some()
            || s.now_playing
                .as_ref()
                .is_some_and(|np| np.catch_up.is_some()));
    if !catching_up {
        s.pos_timer.stop();
        return;
    }
    if s.pos_timer.running() {
        return;
    }
    let player_tx = s.player_tx.clone();
    s.pos_timer
        .start(slint::TimerMode::Repeated, POSITION_EVERY, move || {
            let _ = player_tx.send(PlayerCommand::Position);
        });
}

/// While the next episode counts down, counts each second (VOD ticket
/// 08); otherwise nothing runs.
fn tick_next(screen: &Rc<RefCell<Screen>>) {
    let s = screen.borrow();
    if s.next.is_none() {
        s.next_timer.stop();
        return;
    }
    if s.next_timer.running() {
        return;
    }
    let weak = Rc::downgrade(screen);
    s.next_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(1),
        move || {
            let Some(screen) = weak.upgrade() else {
                return;
            };
            let mut s = screen.borrow_mut();
            if !s.count_down() {
                s.next_timer.stop();
            }
        },
    );
}

/// How often [`tick_position`] asks.
const POSITION_EVERY: Duration = Duration::from_secs(1);

/// While recording, redraws the REC indicator just after each second
/// recorded; otherwise nothing runs.
fn tick_recording(screen: &Rc<RefCell<Screen>>) {
    let s = screen.borrow();
    let since = s
        .mpv
        .as_ref()
        .and_then(|m| m.recording.as_ref())
        .map(|r| r.0);
    let Some(since) = since else {
        s.rec_timer.stop();
        return;
    };
    let past = since.elapsed().subsec_millis();
    let wait = Duration::from_millis(u64::from(1000 - past) + 20);
    let weak = Rc::downgrade(screen);
    s.rec_timer
        .start(slint::TimerMode::SingleShot, wait, move || {
            if let Some(screen) = weak.upgrade() {
                screen.borrow().show_recording();
                tick_recording(&screen);
            }
        });
}

/// The open account dashboard (ticket 21).
struct Dashboard {
    /// Server, username and password shown instead of masked. Forgotten on
    /// close, so it always opens masked.
    revealed: [bool; 3],
    /// Its account check has not answered yet.
    checking: bool,
}

/// The Recordings view (recording ticket 04): its lines, and the selected
/// one, always a recording while there are any.
#[derive(Default)]
struct RecView {
    lines: Vec<RecLine>,
    selected: usize,
    /// On the Downloads tab (VOD ticket 10).
    downloads: bool,
}

impl RecView {
    fn selected(&self) -> Option<&store::Recording> {
        match self.lines.get(self.selected) {
            Some(RecLine::Rec(r)) => Some(r),
            _ => None,
        }
    }

    fn selected_dl(&self) -> Option<&store::Download> {
        match self.lines.get(self.selected) {
            Some(RecLine::Dl(d)) => Some(d),
            _ => None,
        }
    }

    /// Moves `d` recordings down (up if negative) from the selection, or
    /// from the first after it if it is not on one, stopping at the ends.
    fn step(&mut self, d: isize) {
        let recs: Vec<usize> = (0..self.lines.len())
            .filter(|&i| !matches!(self.lines[i], RecLine::Heading(_)))
            .collect();
        let Some(&last) = recs.last() else {
            self.selected = 0;
            return;
        };
        let at = recs
            .iter()
            .position(|&i| i >= self.selected)
            .unwrap_or(recs.len() - 1);
        let to = (at as isize + d).clamp(0, recs.len() as isize - 1) as usize;
        self.selected = recs.get(to).copied().unwrap_or(last);
    }
}

/// The Settings view's sections (settings ticket 01), in order.
const SETTINGS_SECTIONS: [&str; 6] = [
    "Providers",
    "General",
    "Player",
    "Recording",
    "Downloads",
    "EPG",
];

/// The Settings view (settings ticket 01): the section and field
/// selected, and a picker open on the field.
#[derive(Default)]
struct SettingsView {
    section: usize,
    /// Keys move between the fields rather than the sections.
    in_fields: bool,
    field: usize,
    /// A picker is open on the selected field, this choice highlighted.
    choosing: Option<usize>,
    /// Why the selected field's last save failed.
    error: Option<String>,
    /// Why nothing saves: `config.toml` is managed (SPEC 4.2).
    managed: Option<String>,
    /// Why the Providers section saves nothing: `credentials.toml` is
    /// managed (onboarding ticket 01).
    providers_managed: Option<String>,
    /// The add-provider form is open in the Providers section.
    form: Option<ProviderForm>,
    /// The selected field is being typed into, this its text (settings
    /// ticket 02).
    editing: Option<String>,
    /// The typed directory does not exist: `Create it`, or Enter again,
    /// makes it.
    create: Option<PathBuf>,
    /// Settings › EPG is shown (EPG sources ticket 04).
    epg: Option<EpgView>,
}

impl SettingsView {
    /// Why the selected section saves nothing, if it does not. Guide
    /// sources are in `credentials.toml`.
    fn locked(&self) -> Option<&String> {
        match SETTINGS_SECTIONS[self.section] {
            "Providers" | "EPG" => self.providers_managed.as_ref(),
            _ => self.managed.as_ref(),
        }
    }
}

/// The add-provider form's fields, in order, with what each note says;
/// the check-and-save row follows them (onboarding ticket 01).
const FORM_FIELDS: [(&str, &str); 4] = [
    (
        "Name",
        "Shown in the switcher · the provider's id is made from it",
    ),
    ("Server URL", "http://host:port · only the origin is kept"),
    ("Username", ""),
    ("Password", "Ctrl r shows it"),
];

/// The add-provider form in Settings › Providers (onboarding ticket 01):
/// what is typed, and where its account check is.
#[derive(Default)]
struct ProviderForm {
    /// Name, server URL, username and password, as typed; the URL cut to
    /// its origin once confirmed.
    values: [String; 4],
    /// The URL typed had a path, query or login, which was dropped.
    cut: bool,
    /// The password shows as typed rather than as dots.
    revealed: bool,
    check: FormCheck,
    /// The provider being edited, by id; `None` adds one (onboarding
    /// ticket 02).
    id: Option<String>,
    /// Its removal is being confirmed, in place of the form.
    remove: Option<RemoveConfirm>,
}

/// Removing the provider being edited (onboarding ticket 02): its name,
/// what goes and what stays, and whether the personal data goes too.
struct RemoveConfirm {
    name: String,
    data: store::ProviderData,
    personal: bool,
}

#[derive(Default)]
enum FormCheck {
    #[default]
    None,
    /// The account check is running; its answer carries this number.
    Checking(u64),
    /// The check warned, saying why; `Save anyway` saves this.
    Warned(String, Box<Checked>),
}

/// A provider from the form, checked and ready to save: its client, which
/// it keeps, and the check's account details if it logged in.
struct Checked {
    provider: Provider,
    client: Arc<Client>,
    info: Option<AccountInfo>,
}

/// The Providers section's fields: each provider, with its id and the
/// last account check, then "Add provider…"; or, while it is open, the
/// provider form and its check-and-save row, editing adding "Remove
/// provider…"; or the removal's confirm. `providers` is each provider's
/// name and note.
fn provider_items(providers: &[(&str, String)], form: Option<&ProviderForm>) -> Vec<SettingItem> {
    let row = |label: &str, note: &str| SettingItem {
        label: label.into(),
        note: note.into(),
        kind: SettingKind::Row,
        ..Default::default()
    };
    let Some(form) = form else {
        return providers
            .iter()
            .map(|(name, note)| row(name, note))
            .chain(std::iter::once(row(
                "Add provider…",
                "Checks the account, then saves it to credentials.toml",
            )))
            .collect();
    };
    if let Some(r) = &form.remove {
        let d = &r.data;
        let options: Vec<slint::SharedString> = vec!["Keep".into(), "Delete".into()];
        let personal = SettingItem {
            label: "Also delete favourites, progress and history".into(),
            note: format!(
                "{}, {} with progress, {} recorded or downloaded · files on disk stay",
                count(d.favourites, "favourite"),
                d.progress,
                d.recorded
            )
            .into(),
            kind: SettingKind::Toggle,
            options: ModelRc::new(VecModel::from(options)),
            chosen: r.personal as i32,
            ..Default::default()
        };
        let bulk = format!(
            "Deletes {}, {} and {} series and their guide · cancels {} · forgets {}",
            count(d.channels, "channel"),
            count(d.movies, "movie"),
            d.series,
            count(d.scheduled, "scheduled recording"),
            count(d.queued, "queued download")
        );
        return vec![personal, row(&format!("Remove {}", r.name), &bulk)];
    }
    let mut items: Vec<SettingItem> = FORM_FIELDS
        .iter()
        .zip(&form.values)
        .map(|((label, note), value)| SettingItem {
            label: (*label).into(),
            note: (*note).into(),
            value: value.as_str().into(),
            kind: SettingKind::Text,
            ..Default::default()
        })
        .collect();
    if let Some(id) = &form.id {
        items[0].note = format!("Shown in the switcher · its id, {id}, never changes").into();
    }
    if form.cut {
        items[1].note = "Only the origin is kept: the path or login typed was dropped".into();
    }
    let password = &mut items[3];
    if !form.revealed {
        password.value = "•".repeat(form.values[3].chars().count()).into();
    }
    password.reveal = if form.revealed { "Hide" } else { "Show" }.into();
    let editing = form.id.is_some();
    let mut save = row(
        if editing {
            "Save changes"
        } else {
            "Check and save"
        },
        match form.check {
            FormCheck::None if editing => {
                "Checks the account if the URL or login changed, then saves"
            }
            FormCheck::None => "Checks the account, then saves it to credentials.toml",
            FormCheck::Checking(_) => "Checking the account…",
            FormCheck::Warned(..) => "Not saved",
        },
    );
    if let FormCheck::Warned(why, _) = &form.check {
        save.error = why.as_str().into();
        save.button = "Save anyway".into();
    }
    items.push(save);
    if editing {
        items.push(row(
            "Remove provider…",
            "Asks first, with what it deletes and what it keeps",
        ));
    }
    items
}

/// The guide card (EPG sources ticket 05), each layer over the one
/// before: `E`'s sources, Settings › EPG's ambiguous list, the mapping
/// picker.
#[derive(Default)]
struct Card {
    sources: Option<SourcesCard>,
    ambiguous: Option<AmbiguousCard>,
    picker: Option<Picker>,
}

impl Card {
    fn is_open(&self) -> bool {
        self.sources.is_some() || self.ambiguous.is_some() || self.picker.is_some()
    }
}

/// `E` on a live channel: its guide sources in order, and what each gives
/// it.
struct SourcesCard {
    stream_id: i64,
    name: String,
    sources: Vec<store::ChannelSource>,
    selected: usize,
}

/// Settings › EPG's ambiguous list: the provider's channels that matched
/// several of one source's.
struct AmbiguousCard {
    source: i64,
    label: String,
    all: Vec<(i64, String)>,
    query: String,
    /// `all`'s entries whose name holds the query.
    shown: Vec<usize>,
    selected: usize,
}

impl AmbiguousCard {
    fn filter(&mut self) {
        let q = self.query.to_lowercase();
        self.shown = (0..self.all.len())
            .filter(|&i| self.all[i].1.to_lowercase().contains(&q))
            .collect();
        self.selected = self.selected.min(self.shown.len().saturating_sub(1));
    }
}

/// The mapping picker: one source's channels, searched, for one channel.
struct Picker {
    source: i64,
    label: String,
    stream_id: i64,
    name: String,
    /// Its mapping now: a first row then forgets it.
    mapped: Option<String>,
    query: String,
    found: Vec<store::SourceChannel>,
    selected: usize,
}

impl Picker {
    fn len(&self) -> usize {
        self.mapped.is_some() as usize + self.found.len()
    }
}

/// How many of a source's channels the picker lists for a search.
const PICKER_ROWS: usize = 200;

/// The `E` card's row for source `s`, named `label`: what it gives the
/// channel, whether it is pinned there, and whether the channel's guide
/// comes from it now.
fn source_card_row(s: &store::ChannelSource, label: &str) -> CardRow {
    let channel = s
        .channel_name
        .clone()
        .or_else(|| s.channel.clone())
        .unwrap_or_default();
    let found = |how: &str| {
        let none = if !s.programmes && !s.reimport {
            " · no programmes for it"
        } else {
            ""
        };
        format!("{how}: {channel}{none}")
    };
    let mut note = if s.ident.is_empty() {
        match (&s.channel, s.programmes) {
            (Some(_), true) => "Matched by its guide id".to_owned(),
            (Some(id), false) => format!("No programmes for its guide id {id}"),
            (None, _) => "The provider gives it no guide id".to_owned(),
        }
    } else if let Some(m) = &s.mapped {
        if s.channel.as_ref() == Some(m) {
            found("Mapped")
        } else {
            format!("Mapped to {m}, which the feed no longer has")
        }
    } else if s.ambiguous {
        "Ambiguous: several of its channels match · Enter picks one".to_owned()
    } else if s.channel.is_some() {
        found("Matched")
    } else {
        "No match · Enter picks a channel".to_owned()
    };
    if s.reimport {
        note += " · re-import at the next refresh";
    }
    if !s.enabled {
        note = format!("Off · {note}");
    }
    let mut mark = Vec::new();
    match s.pin {
        Some(0) => mark.push("PINNED".to_owned()),
        Some(m) => mark.push(format!("PINNED {m:+} min")),
        None => {}
    }
    if s.guiding {
        mark.push("IN USE".to_owned());
    }
    CardRow {
        label: label.into(),
        note: note.into(),
        mark: mark.join(" · ").into(),
        warn: s.ambiguous,
        action: if s.pin.is_some() { "Unpin" } else { "Pin" }.into(),
    }
}

/// The picker's rows: "Automatic" while the channel is mapped, then the
/// channels found, by first name, with their id and other names.
fn picker_rows(p: &Picker) -> Vec<CardRow> {
    let auto = p.mapped.as_ref().map(|_| CardRow {
        label: "Automatic".into(),
        note: "Forgets the mapping: matched by id or name again at the next refresh".into(),
        ..Default::default()
    });
    auto.into_iter()
        .chain(p.found.iter().map(|c| {
            let mut names = c.names.iter();
            let label = names.next().cloned().unwrap_or_else(|| c.id.clone());
            let note = std::iter::once(c.id.as_str())
                .chain(names.map(String::as_str))
                .collect::<Vec<_>>()
                .join(" · ");
            CardRow {
                label: label.into(),
                note: note.into(),
                mark: if p.mapped.as_ref() == Some(&c.id) {
                    "MAPPED"
                } else {
                    ""
                }
                .into(),
                ..Default::default()
            }
        }))
        .collect()
}

/// The name Settings › EPG gives the provider's source `ident`: the
/// provider's for its own guide (`""`).
fn guide_label(provider: &Provider, ident: &str) -> String {
    if ident.is_empty() {
        return provider.name.clone();
    }
    let order = config::epg_order(provider);
    order
        .iter()
        .position(|s| !s.provider_guide && s.ident() == ident)
        .map_or_else(
            || config::source_label(ident),
            |i| config::epg_labels(provider)[i].clone(),
        )
}

/// Settings › EPG (EPG sources ticket 04), for the provider shown: its
/// channel count and each source's counts, read on entering the section
/// and after each import.
#[derive(Default)]
struct EpgView {
    channels: i64,
    stats: HashMap<String, store::SourceStats>,
    /// The counts have been read once; until then the rows say so.
    counted: bool,
    /// URLs show whole rather than with their query strings masked.
    revealed: bool,
    /// The source just added, by its place in the order, offered a move
    /// above the provider's guide.
    offer: Option<usize>,
    form: Option<SourceForm>,
}

/// The provider's channel count and its guide sources' counts by ident,
/// as [`store::source_stats`] reads them.
type EpgCounts = (i64, HashMap<String, store::SourceStats>);

/// The source form's text fields, with what each note says.
const SOURCE_FIELDS: [(&str, &str); 5] = [
    (
        "URL or path",
        "An http(s) address or an absolute path · XMLTV, plain or gzipped",
    ),
    ("Name", "Optional · else its host or file name"),
    (
        "Shift",
        "Minutes added to every programme's times, e.g. -60 · from its next fetch",
    ),
    (
        "Fetch every",
        "Hours between scheduled fetches of a URL · 12 when blank",
    ),
    (
        "At most per day",
        "Requests to a URL in 24 h, at least 3, the last 2 kept for r · none when blank",
    ),
];

/// One row of the source form.
#[derive(Clone, Copy, Debug, PartialEq)]
enum SourceField {
    Enabled,
    /// [`SOURCE_FIELDS`]' field.
    Text(usize),
    Save,
    /// "Ambiguous channels…": the list that opens the mapping picker (EPG
    /// sources ticket 05).
    Ambiguous,
    /// The confirm's "Also delete N manual mappings" switch.
    Forget,
    /// "Remove source…", or the confirm's Remove row.
    Remove,
}

/// The source form in Settings › EPG: a new source, an external one to
/// edit, disable or remove, or the provider's guide, which has only its
/// switch and shift.
#[derive(Default)]
struct SourceForm {
    /// Its place in the order; `None` adds one.
    index: Option<usize>,
    guide: bool,
    /// [`SOURCE_FIELDS`], as typed.
    values: [String; 5],
    enabled: bool,
    /// Turning the provider's guide off leaves this many favourites with
    /// no guide.
    orphans: Option<i64>,
    /// Its removal is being confirmed, in place of the form.
    remove: bool,
    /// The saved source's ambiguous channels and manual mappings, from
    /// its stats (EPG sources ticket 05).
    ambiguous: i64,
    mappings: i64,
    /// The confirm deletes its manual mappings too.
    forget: bool,
}

impl SourceForm {
    /// The form for `source`, at `index` in the order, filled in.
    fn edit(index: usize, source: &EpgSource) -> Self {
        let number = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
        let place = match (&source.url, &source.path) {
            (Some(url), _) => url.clone(),
            (None, Some(path)) => path.display().to_string(),
            (None, None) => String::new(),
        };
        SourceForm {
            index: Some(index),
            guide: source.provider_guide,
            values: [
                place,
                source.name.clone().unwrap_or_default(),
                match source.shift_minutes {
                    0 => String::new(),
                    n => n.to_string(),
                },
                number(source.every_hours),
                number(source.max_per_day),
            ],
            enabled: source.enabled,
            ..Default::default()
        }
    }

    fn fields(&self) -> Vec<SourceField> {
        if self.remove && self.mappings > 0 {
            return vec![SourceField::Forget, SourceField::Remove];
        }
        if self.remove {
            return vec![SourceField::Remove];
        }
        let mut fields = Vec::new();
        if self.index.is_some() {
            fields.push(SourceField::Enabled);
        }
        if self.guide {
            fields.push(SourceField::Text(2));
        } else {
            fields.extend((0..SOURCE_FIELDS.len()).map(SourceField::Text));
        }
        fields.push(SourceField::Save);
        if self.index.is_some() && !self.guide && self.ambiguous > 0 {
            fields.push(SourceField::Ambiguous);
        }
        if self.index.is_some() && !self.guide {
            fields.push(SourceField::Remove);
        }
        fields
    }

    /// Takes the saved source's counts from `stats`.
    fn counts(&mut self, stats: Option<&store::SourceStats>) {
        let stats = stats.cloned().unwrap_or_default();
        (self.ambiguous, self.mappings) = (stats.ambiguous, stats.mappings);
    }

    /// The source as typed, each value checked: the field to go back to
    /// and why, if one is not right.
    fn source(&self) -> Result<EpgSource, (usize, String)> {
        if self.guide {
            let shift =
                config::check_number(&self.values[2], i64::MIN, "Shift").map_err(|e| (2, e))?;
            return Ok(EpgSource {
                enabled: self.enabled,
                shift_minutes: shift.unwrap_or(0),
                ..EpgSource::provider_guide()
            });
        }
        let (url, path) = config::check_source(&self.values[0]).map_err(|e| (0, e))?;
        let shift = config::check_number(&self.values[2], i64::MIN, "Shift").map_err(|e| (2, e))?;
        let every = config::check_number(&self.values[3], 1, "Fetch every").map_err(|e| (3, e))?;
        let max =
            config::check_number(&self.values[4], 3, "At most per day").map_err(|e| (4, e))?;
        if path.is_some() && (every.is_some() || max.is_some()) {
            let field = if every.is_some() { 3 } else { 4 };
            return Err((field, "Only a URL is fetched by the hour or the day".into()));
        }
        let name = self.values[1].trim();
        let small = |n: Option<i64>| n.map(|n| u32::try_from(n).unwrap_or(u32::MAX));
        Ok(EpgSource {
            url,
            path,
            provider_guide: false,
            name: (!name.is_empty()).then(|| name.to_owned()),
            shift_minutes: shift.unwrap_or(0),
            enabled: self.enabled,
            every_hours: small(every),
            max_per_day: small(max),
        })
    }
}

/// `1,204`.
fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// A source's URL with its query string, and any password in it, masked,
/// unless `revealed`; a path as it is (SPEC 4.1).
fn shown_url(text: &str, revealed: bool) -> String {
    if revealed {
        return text.to_owned();
    }
    match reqwest::Url::parse(text) {
        Ok(mut url) if url.has_host() => {
            let query = url.query().is_some();
            if url.password().is_some() {
                let _ = url.set_password(Some("•••"));
            }
            url.set_query(None);
            url.set_fragment(None);
            let url = url.to_string().replace("%E2%80%A2", "•");
            if query {
                format!("{url}?•••")
            } else {
                url
            }
        }
        // While typed: all after a `?` masked.
        _ => match text.split_once('?') {
            Some((head, _)) => format!("{head}?•••"),
            None => text.to_owned(),
        },
    }
}

/// Settings › EPG's rows: each of the provider's sources in order, its
/// place, match counts, requests and last result; then the offer to move a
/// new source above the provider's guide, while it is below it; then "Add
/// source…". `busy` are the labels of sources a refresh is importing.
fn source_items(
    sources: &[EpgSource],
    labels: &[String],
    provider: &str,
    epg: &EpgView,
    busy: &[&str],
    now: i64,
) -> Vec<SettingItem> {
    let row = |label: &str, note: &str| SettingItem {
        label: label.into(),
        note: note.into(),
        kind: SettingKind::Row,
        ..Default::default()
    };
    let mut items: Vec<SettingItem> = sources
        .iter()
        .zip(labels)
        .map(|(s, label)| {
            let stats = epg.stats.get(&s.ident()).cloned().unwrap_or_default();
            let place = match (&s.url, &s.path) {
                _ if s.provider_guide => "The provider's own guide (xmltv.php)".to_owned(),
                (Some(url), _) => shown_url(url, epg.revealed),
                (None, Some(path)) => path.display().to_string(),
                (None, None) => String::new(),
            };
            if !epg.counted {
                let mut item = row(label, &format!("{place}\ncounting…"));
                item.value = if s.enabled { "" } else { "Off" }.into();
                return item;
            }
            let none = (epg.channels - stats.matched - stats.ambiguous).max(0);
            let counts = if s.provider_guide {
                format!(
                    "matched {} · none {}",
                    thousands(stats.matched),
                    thousands(none)
                )
            } else {
                format!(
                    "matched {} · ambiguous {} · none {}",
                    thousands(stats.matched),
                    thousands(stats.ambiguous),
                    thousands(none)
                )
            };
            let mut last = Vec::new();
            if s.url.is_some() {
                last.push(match s.max_per_day {
                    Some(max) => format!("{}/{max} in 24 h", stats.requests),
                    None => format!("{} in 24 h", count(stats.requests, "request")),
                });
            }
            let mut error = String::new();
            if !s.enabled {
                last.push("off: skipped, its data kept".into());
            } else if busy.contains(&label.as_str()) && !s.provider_guide {
                last.push("importing…".into());
            } else if let Some(t) = stats.retry_at.filter(|&t| t > now) {
                last.push(format!("busy, asked again after {}", when(t, now)));
            } else if let Some(e) = &stats.error {
                last.push("failing".into());
                error = format!("Last fetch failed: {e}");
            } else if let Some(t) = stats.imported {
                last.push(format!("imported {}", when(t, now)));
            } else {
                last.push("not fetched yet".into());
            }
            if stats.reimport && s.enabled && !busy.contains(&label.as_str()) {
                last.push("re-import at the next refresh".into());
            }
            let mut item = row(label, &format!("{place}\n{counts}\n{}", last.join(" · ")));
            item.value = if s.enabled { "" } else { "Off" }.into();
            item.error = error.into();
            item
        })
        .collect();
    let guide = sources.iter().position(|s| s.provider_guide);
    if let Some(i) = epg.offer
        && guide.is_some_and(|g| g < i)
        && let Some(label) = labels.get(i)
    {
        items.push(row(
            &format!("Move {label} above {provider}'s guide"),
            "Its programmes then win wherever both have a channel's",
        ));
    }
    items.push(row(
        "Add source…",
        "Checked without fetching, saved last in the order, then imported",
    ));
    items
}

/// The source form's rows (EPG sources ticket 04): for the provider's
/// guide its switch and shift; for another its switch when it is saved,
/// URL or path, name, shift and limits, then save, then "Remove source…";
/// or the removal's confirm. `stats` is the saved source's.
fn source_form_items(
    form: &SourceForm,
    label: &str,
    revealed: bool,
    stats: &store::SourceStats,
) -> Vec<SettingItem> {
    let row = |label: &str, note: &str| SettingItem {
        label: label.into(),
        note: note.into(),
        kind: SettingKind::Row,
        ..Default::default()
    };
    form.fields()
        .into_iter()
        .map(|field| match field {
            SourceField::Remove if form.remove => row(
                &format!("Remove {label}"),
                &format!(
                    "Deletes its channel list, matches, programmes and pins · {} back to the next source in the order{}",
                    match stats.guiding {
                        1 => "1 channel goes".to_owned(),
                        n => format!("{n} channels go"),
                    },
                    match stats.pins {
                        0 => String::new(),
                        1 => " · 1 pinned channel goes back to the source order".to_owned(),
                        n => format!(" · {n} pinned channels go back to the source order"),
                    }
                ),
            ),
            SourceField::Forget => {
                let options: Vec<slint::SharedString> = vec!["Keep".into(), "Delete".into()];
                SettingItem {
                    label: format!(
                        "Also delete {}",
                        count(stats.mappings, "manual mapping")
                    )
                    .into(),
                    note: "Kept, they come back if a source with this address is added again"
                        .into(),
                    kind: SettingKind::Toggle,
                    options: ModelRc::new(VecModel::from(options)),
                    chosen: form.forget as i32,
                    ..Default::default()
                }
            }
            SourceField::Ambiguous => row(
                "Ambiguous channels…",
                &format!(
                    "{} matched several of its channels, so none · Enter maps them",
                    count(stats.ambiguous, "channel")
                ),
            ),
            SourceField::Remove => row(
                "Remove source…",
                "Asks first, with what it deletes",
            ),
            SourceField::Save if form.index.is_none() => row(
                "Add and import",
                "Saves it last in the order, then fetches it once",
            ),
            SourceField::Save => row("Save changes", "Saves it to credentials.toml"),
            SourceField::Enabled => {
                let options: Vec<slint::SharedString> = vec!["On".into(), "Off".into()];
                let mut item = SettingItem {
                    label: "Enabled".into(),
                    note: if form.guide {
                        "Off stops xmltv.php and deletes its programmes"
                    } else {
                        "Off skips it and keeps its data, so on again is instant"
                    }
                    .into(),
                    kind: SettingKind::Toggle,
                    options: ModelRc::new(VecModel::from(options)),
                    chosen: (!form.enabled) as i32,
                    ..Default::default()
                };
                if let Some(n) = form.orphans.filter(|&n| n > 0 && !form.enabled) {
                    item.error = format!(
                        "Provider guide off: {} of your favourites {} no other source",
                        n,
                        if n == 1 { "has" } else { "have" }
                    )
                    .into();
                }
                item
            }
            SourceField::Text(i) => {
                let (label, note) = SOURCE_FIELDS[i];
                let value = if i == 0 {
                    shown_url(&form.values[0], revealed)
                } else {
                    form.values[i].clone()
                };
                SettingItem {
                    label: label.into(),
                    note: note.into(),
                    value: value.into(),
                    kind: SettingKind::Text,
                    reveal: match i {
                        0 if form.values[0].contains('?') => {
                            if revealed { "Hide" } else { "Show" }
                        }
                        _ => "",
                    }
                    .into(),
                    ..Default::default()
                }
            }
        })
        .collect()
}

/// The setup screen's title and what it says (onboarding ticket 03): no
/// text over the form, which the title explains; else the reason, paths
/// under `home` shortened to `~`.
fn setup_text(setup: &config::Setup, home: &Path) -> (&'static str, String) {
    let tilde = |text: &str| match home.to_str() {
        Some(h) if !h.trim_end_matches('/').is_empty() => {
            text.replace(&format!("{}/", h.trim_end_matches('/')), "~/")
        }
        _ => text.to_owned(),
    };
    match setup {
        config::Setup::Missing | config::Setup::Empty => ("Add a provider", String::new()),
        config::Setup::Loose { why, fix } => (
            "Credentials open to others",
            format!(
                "{}\n\nNothing is read from it until this is fixed{}.",
                tilde(why),
                if *fix { "" } else { " by hand" }
            ),
        ),
        config::Setup::Invalid(why) => ("Credentials not valid", tilde(why)),
    }
}

/// The setup screen's rows when it shows no form: `Fix permissions` only
/// for a file the user owns outside the Nix store, then `Check again`.
fn setup_items(setup: &config::Setup) -> Vec<SettingItem> {
    let row = |label: &str, note: &str| SettingItem {
        label: label.into(),
        note: note.into(),
        kind: SettingKind::Row,
        ..Default::default()
    };
    let mut items = Vec::new();
    if let config::Setup::Loose { fix: true, .. } = setup {
        items.push(row(
            "Fix permissions",
            "Sets 0600 on the file and 0700 on its folder, then carries on",
        ));
    }
    items.push(row(
        "Check again",
        "Reads the file again, once changed by hand",
    ));
    items
}

/// Each Settings section's keys in `config.toml`, field by field.
const SETTING_FIELDS: [&[&str]; 6] = [
    &[],
    &["active_provider", "refresh_hours"],
    &["mpv_path", "extra_mpv_args"],
    &[
        "recordings_dir",
        "ffmpeg_path",
        "record_before",
        "record_after",
    ],
    &["downloads_dir"],
    &[],
];

/// What a Settings text field shows and is edited as: the value as typed,
/// `extra_mpv_args` as a shell line.
fn setting_text(settings: &config::Settings, key: &str) -> String {
    match key {
        "mpv_path" => settings.mpv_path.clone(),
        "extra_mpv_args" => config::join_args(&settings.extra_mpv_args),
        "recordings_dir" => settings.recordings_dir.display().to_string(),
        "ffmpeg_path" => settings.ffmpeg_path.clone(),
        "downloads_dir" => settings.downloads_dir.display().to_string(),
        _ => String::new(),
    }
}

/// The fields of Settings section `section`, from `settings`: General's
/// switch and picker; Player, Recording and Downloads text and steppers
/// (settings ticket 02); Providers and EPG are empty. `providers` is each
/// provider's id and name.
fn setting_items(
    section: usize,
    settings: &config::Settings,
    providers: &[(&str, &str)],
) -> Vec<SettingItem> {
    let text = |label: &str, note: &str, key: &str| SettingItem {
        label: label.into(),
        note: note.into(),
        value: setting_text(settings, key).into(),
        kind: SettingKind::Text,
        ..Default::default()
    };
    let stepper = |label: &str, note: &str, minutes: u32| SettingItem {
        label: label.into(),
        note: note.into(),
        value: format!("{minutes} min").into(),
        kind: SettingKind::Stepper,
        chosen: minutes as i32,
        ..Default::default()
    };
    let options = |o: Vec<&str>| {
        let o: Vec<slint::SharedString> = o.into_iter().map(Into::into).collect();
        ModelRc::new(VecModel::from(o))
    };
    match SETTINGS_SECTIONS[section] {
        "General" => {
            let pinned = settings
                .active_provider
                .as_deref()
                .and_then(|id| providers.iter().position(|(p, _)| *p == id));
            let names = std::iter::once("Last used").chain(providers.iter().map(|(_, n)| *n));
            let hours = [12, 6];
            let refresh = hours
                .iter()
                .position(|&h| h == settings.refresh_hours)
                .unwrap_or(0);
            vec![
                SettingItem {
                    label: "Start with".into(),
                    note: "active_provider · used from the next start".into(),
                    value: names
                        .clone()
                        .nth(pinned.map_or(0, |i| i + 1))
                        .unwrap_or("")
                        .into(),
                    kind: SettingKind::Choice,
                    options: options(names.collect()),
                    chosen: pinned.map_or(0, |i| i as i32 + 1),
                    ..Default::default()
                },
                SettingItem {
                    label: "Refresh when older than".into(),
                    note: "refresh_hours · channels and guide, checked every 15 min".into(),
                    kind: SettingKind::Toggle,
                    options: options(vec!["12 h", "6 h"]),
                    chosen: refresh as i32,
                    ..Default::default()
                },
            ]
        }
        "Player" => vec![
            text("mpv", "mpv_path · on PATH, or a full path", "mpv_path"),
            SettingItem {
                // Shown as the array it writes, edited as a shell line.
                value: settings
                    .extra_mpv_args
                    .iter()
                    .collect::<toml_edit::Array>()
                    .to_string()
                    .into(),
                ..text(
                    "Extra mpv arguments",
                    "extra_mpv_args · quoted as in a shell",
                    "extra_mpv_args",
                )
            },
        ],
        "Recording" => vec![
            text(
                "Recordings folder",
                "recordings_dir · R and scheduled ones",
                "recordings_dir",
            ),
            text(
                "ffmpeg",
                "ffmpeg_path · records scheduled programmes",
                "ffmpeg_path",
            ),
            stepper(
                "Start early by",
                "record_before · before the guide's start, 0 to 60",
                settings.record_before,
            ),
            stepper(
                "Run late by",
                "record_after · past the guide's stop, 0 to 60",
                settings.record_after,
            ),
        ],
        "Downloads" => vec![text(
            "Downloads folder",
            "downloads_dir · where d and D save",
            "downloads_dir",
        )],
        _ => Vec::new(),
    }
}

/// UI-thread state behind the window.
struct Screen {
    window: slint::Weak<AppWindow>,
    conn: Rc<Connection>,
    sources: Vec<Source>,
    /// Index into `sources` of the provider shown.
    active: usize,
    /// `credentials.toml` gives no provider, saying why: the setup screen
    /// shows over everything, and `sources` is empty (onboarding ticket
    /// 03).
    setup: Option<config::Setup>,
    /// The provider picker is open, with this entry highlighted.
    picking: Option<usize>,
    /// The guide card (EPG sources ticket 05).
    card: Card,
    /// The account dashboard is open.
    dashboard: Option<Dashboard>,
    /// The keys panel is open.
    keys_open: bool,
    /// The Recordings view is open.
    recordings: Option<RecView>,
    /// The Settings view is open.
    settings_view: Option<SettingsView>,
    /// `config.toml` as read at start or last saved, and where it is.
    settings: config::Settings,
    settings_path: PathBuf,
    /// Where `credentials.toml` is, which the add-provider form saves to.
    credentials_path: PathBuf,
    /// Starts a provider added in Settings (onboarding ticket 01).
    starter: Starter,
    /// The add-provider form's account checks answer here, numbered.
    checked_tx: mpsc::UnboundedSender<(u64, Checked, xtream::Check)>,
    checks: u64,
    /// Settings › EPG's counts are read off the UI thread from the
    /// database at `db_path` and answer here, numbered; only the last
    /// asked is kept.
    db_path: PathBuf,
    epg_tx: mpsc::UnboundedSender<(u64, String, EpgCounts)>,
    epg_reads: u64,
    /// Lapses a first `x` or `c` in the Recordings view; the window's
    /// `recordings-armed` says which recording `x` would cancel, and
    /// `recordings-clear-armed` whether `c` would clear.
    cancel_timer: slint::Timer,
    player_tx: mpsc::UnboundedSender<PlayerCommand>,
    recorder_tx: mpsc::UnboundedSender<recorder::Command>,
    write_tx: mpsc::UnboundedSender<Write>,
    /// The library's kind switch (VOD ticket 03): whose categories it lists.
    kind: Kind,
    categories: Vec<Category>,
    category: usize,
    /// Of the categories and of All and each category; shared by providers.
    order: Order,
    /// Some of the provider's categories are hidden (ticket 20).
    hidden: bool,
    /// Set while manage mode is open.
    manage: Option<Manage>,
    manage_rows: Rc<VecModel<ManageItem>>,
    /// The Settings view's fields, changed row by row so the selected one
    /// stays the same element and scrolls into view.
    settings_rows: Rc<VecModel<SettingItem>>,
    /// The current category's row keys, before the search filter.
    base: Vec<i64>,
    channels: Rc<Channels>,
    selected: usize,
    /// `t`: the guide grid is open in place of the three panes.
    guide_open: bool,
    guide: Rc<Guide>,
    guide_selected: usize,
    /// The time `j`/`k` keep to in the grid; `None` is now, following the
    /// clock ([`guide_anchor`]).
    guide_anchor: Option<i64>,
    /// The grid's span starts at the current half hour, and moves on with
    /// it.
    guide_on_now: bool,
    /// Where the grid's span starts: a half hour from keys, any second
    /// from the mouse.
    guide_view: i64,
    /// The span's start when a drag in the grid began.
    guide_drag_from: i64,
    /// The grid's category's guide channels, unfiltered.
    guide_base: Vec<i64>,
    /// How far back the grid reaches in its category ([`guide_back`]).
    guide_back: i64,
    /// `/` in the grid: its own filter, kept until a category switch or
    /// leaving the grid.
    guide_query: String,
    guide_searching: bool,
    /// The channel last sent to mpv; PageUp/PageDown in mpv step from it.
    now_playing: Option<NowPlaying>,
    /// The movie last sent to mpv, if it was one; then `now_playing` is
    /// `None` (VOD ticket 04).
    movie: Option<MoviePlaying>,
    /// Plays sent to the player task, and the last one it took up
    /// ([`Event::Handed`]).
    plays: u64,
    handed: u64,
    /// Where a selected movie or series asks for its info ([`info`]).
    info_tx: mpsc::UnboundedSender<info::Ask>,
    /// Titles whose info was asked for, by kind and id: their row keys, to
    /// redraw the row when it comes.
    info_asked: HashMap<(Kind, i64), i64>,
    /// The selected series' seasons and episodes (VOD ticket 05).
    series: SeriesPanel,
    /// The selected channel's other listings.
    alternates: AlternatesPanel,
    /// Set while mpv runs.
    mpv: Option<MpvState>,
    /// Where `R` records to.
    recordings_dir: PathBuf,
    /// Where `d` downloads to (VOD ticket 10).
    downloads_dir: PathBuf,
    /// The Recordings view opens on its Downloads tab, the one last shown.
    downloads_tab: bool,
    /// What the last `d` or `D` asked for, for the status bar: its name,
    /// and whether it was a season.
    dl_asked: (String, bool),
    /// `record_after` in seconds: catch-up also asks for this much past a
    /// programme's guide stop, for a guide running late.
    record_after: i64,
    /// See [`tick_behind`].
    behind_timer: slint::Timer,
    /// See [`tick_recording`].
    rec_timer: slint::Timer,
    /// See [`tick_position`].
    pos_timer: slint::Timer,
    /// The episode the countdown after one ends plays (VOD ticket 08).
    next: Option<NextEpisode>,
    next_timer: slint::Timer,
    /// Row keys of the recently played channels shown.
    recent: Vec<i64>,
    /// Lapses a first press of clear; the window's `clear-armed` says
    /// whether one is pending.
    clear_timer: slint::Timer,
    /// Closes the refresh panel after a success; the window's
    /// `refresh-panel-open` says whether it is open.
    panel_timer: slint::Timer,
    /// Counts a backoff down in the refresh panel.
    stage_timer: slint::Timer,
    /// The detail panel's LATER rows.
    later: Rc<VecModel<LaterItem>>,
    logos: Rc<Logos>,
    /// The logo or poster URL the detail panel is waiting for, to show it
    /// when it arrives, and whether it is a poster. One at most, so logos
    /// of channels scrolled past are dropped.
    detail_logo: Option<(String, bool)>,
    /// Logo URLs of recently played and NOW PLAYING, to redraw them when one
    /// arrives.
    shown_logos: HashSet<String>,
    number: NumberEntry,
    /// The search filter, kept after Enter until Esc.
    query: String,
    /// In search mode: typing edits `query`.
    searching: bool,
    /// Tab in search mode (VOD ticket 09): the filter is over each kind's
    /// All, not the category; their [`store::list_key`]s while it is.
    wide: Option<Vec<i64>>,
    /// Names for search, a kind's index from when it is built until an
    /// import of that kind, which replaces the row keys.
    search: search::Search,
    /// Builds a provider's index of a kind in the background; it arrives
    /// via [`Screen::set_index`].
    rebuild_index: Box<dyn Fn(String, Kind)>,
    /// Set while `--keys` replays, unless `--play`: playback is suppressed.
    replaying: bool,
}

impl Screen {
    fn window(&self) -> AppWindow {
        self.window.unwrap()
    }

    fn provider(&self) -> &Provider {
        &self.sources[self.active].provider
    }

    /// Re-reads categories and the current view, e.g. after an import. The
    /// category and selection stay put if the category still exists.
    fn reload(&mut self) -> Result<(), String> {
        let provider_cats =
            store::categories(&self.conn, &self.provider().id, self.kind, self.order)
                .map_err(|e| format!("reading categories: {e}"))?;
        let none_shown = provider_cats.is_empty();
        let old = std::mem::take(&mut self.categories);
        self.categories = library(self.kind, provider_cats);
        // The sidebar draws the views across kinds itself, above the switch.
        let items: Vec<CategoryItem> = self.categories[LIBRARY_FIXED..]
            .iter()
            .map(|c| CategoryItem {
                name: c.name().into(),
            })
            .collect();
        self.hidden = store::any_hidden(&self.conn, &self.provider().id)
            .map_err(|e| format!("reading categories: {e}"))?;
        let w = self.window();
        w.set_library_hint(
            if self.hidden && none_shown {
                "No categories shown. Press e to choose which."
            } else {
                ""
            }
            .into(),
        );
        w.set_filtering(self.hidden);
        w.set_library_kind(kind_index(self.kind));
        w.set_categories(ModelRc::new(VecModel::from(items)));
        w.set_order_az(self.order == Order::Az);
        let kept = old
            .get(self.category)
            .and_then(|c| self.categories.iter().position(|n| n == c));
        // Otherwise the kind's All.
        self.category = kept.unwrap_or(LIBRARY_FIXED);
        self.load_view(kept.is_none())?;
        if self.guide_open {
            if kept.is_none() {
                self.drop_guide_filter();
            }
            self.load_guide(kept.is_none());
        }
        // Row keys change with an import.
        self.show_recent();
        self.show_playing();
        if self.manage.is_some() {
            self.load_manage();
        }
        Ok(())
    }

    fn set_category(&mut self, i: usize) -> Result<(), String> {
        self.category = i.min(self.categories.len().saturating_sub(1));
        self.wide = None;
        self.load_view(true)
    }

    /// Each kind's All, which a widened search filters (VOD ticket 09).
    fn wide_views(&self) -> Result<Vec<i64>, String> {
        let t = Instant::now();
        let mut keys = Vec::new();
        for kind in Kind::ALL {
            let all = Category::All(kind)
                .keys(&self.conn, &self.provider().id, self.order)
                .map_err(|e| format!("reading {}: {e}", noun(kind)))?;
            keys.extend(all.into_iter().map(|k| store::list_key(kind, k)));
        }
        log::debug!("all kinds' keys in {} µs", t.elapsed().as_micros());
        Ok(keys)
    }

    /// Tab in search mode: the filter over all kinds, or back to the
    /// category.
    fn toggle_wide(&mut self) {
        self.wide = match self.wide {
            Some(_) => None,
            None => match self.wide_views() {
                Ok(views) => Some(views),
                Err(e) => {
                    log::error!("{e}");
                    self.set_error(&e);
                    return;
                }
            },
        };
        self.show_keys(true);
    }

    /// What the middle list's rows are; in a list of several kinds, what
    /// the selected one is.
    fn listing(&self) -> Kind {
        if self.wide.is_some() && !self.query.is_empty() {
            return self.channels.kind_of(self.selected);
        }
        match self.categories.get(self.category) {
            Some(Category::Favourites | Category::Recent | Category::Continue) => {
                self.channels.kind_of(self.selected)
            }
            cat => cat.map_or(Kind::Live, Category::rows),
        }
    }

    /// Where channel `key` is in the list, if the list holds channels.
    fn channel_position(&self, key: i64) -> Option<usize> {
        if self.listing() != Kind::Live {
            return None;
        }
        self.channels.keys.borrow().iter().position(|&k| k == key)
    }

    /// Tab / Shift-Tab: the next or previous kind, wrapping.
    fn cycle_kind(&mut self, d: isize) {
        let i = kind_index(self.kind) as isize + d;
        let kind = Kind::ALL[i.rem_euclid(Kind::ALL.len() as isize) as usize];
        if let Err(e) = self.set_kind(kind) {
            log::error!("{e}");
            self.set_error(&e);
        }
    }

    /// The library's kind switch (VOD ticket 03): lists `kind`'s categories
    /// and opens on its All, from the views above the switch too, and when
    /// `kind` is the one chosen already.
    fn set_kind(&mut self, kind: Kind) -> Result<(), String> {
        if kind == self.kind {
            if self.category < LIBRARY_FIXED {
                self.set_category(LIBRARY_FIXED)?;
                self.window().invoke_show_category();
            }
            return Ok(());
        }
        self.kind = kind;
        // The old kind's All, which the new library lacks, so it opens on its own.
        self.category = LIBRARY_FIXED;
        self.reload()?;
        self.window().invoke_show_category();
        Ok(())
    }

    /// Live, All channels: where a typed number finds its channel.
    fn all_channels(&mut self) -> Result<(), String> {
        self.set_kind(Kind::Live)?;
        self.set_category(LIBRARY_FIXED)
    }

    /// Reads the current category's channels. `reset` returns the list to
    /// the top; otherwise the selection and scroll position are kept.
    fn load_view(&mut self, reset: bool) -> Result<(), String> {
        let cat = &self.categories[self.category];
        let t = Instant::now();
        self.base = cat
            .keys(&self.conn, &self.provider().id, self.order)
            .map_err(|e| format!("reading {}: {e}", noun(cat.rows())))?;
        log::debug!(
            "category {:?}: {} {} in {} µs",
            cat.name(),
            noun(cat.rows()),
            self.base.len(),
            t.elapsed().as_micros()
        );
        if self.wide.is_some() {
            // Each kind's keys change with an import or the order.
            self.wide = Some(self.wide_views()?);
        }
        let w = self.window();
        w.set_category(self.category as i32);
        w.set_category_name(cat.name().into());
        self.show_keys(reset);
        Ok(())
    }

    /// Shows the current category, filtered by the query if there is one:
    /// within its kind, or each kind's All when widened (VOD ticket 09).
    fn show_keys(&mut self, reset: bool) {
        let w = self.window();
        let cat = &self.categories[self.category];
        let kind = cat.rows();
        let mixed = matches!(
            cat,
            Category::Favourites | Category::Recent | Category::Continue
        );
        let scope = if mixed { cat.name() } else { kind_name(kind) };
        let wide = self.wide.is_some() && !self.query.is_empty();
        // In a widened search, how many each kind found.
        let mut found_by_kind = Vec::new();
        let keys = if self.query.is_empty() {
            let noun = noun(kind);
            w.set_empty_hint(
                match cat {
                    Category::Favourites => "No favourites yet.".into(),
                    Category::Recent => "Nothing played yet.".into(),
                    Category::Continue => "Nothing part way through yet.".into(),
                    Category::All(_) if self.hidden => {
                        format!("No {noun} shown. Press e to choose which.")
                    }
                    Category::All(_) | Category::Added(_) => format!("No {noun} yet."),
                    _ => format!("No {noun} in this category."),
                }
                .into(),
            );
            self.base.clone()
        } else {
            let t = Instant::now();
            let (found, of) = match &self.wide {
                Some(all) => (
                    self.search.widen(all, &self.query, WIDE_CAP).map(|groups| {
                        groups
                            .into_iter()
                            .flat_map(|g| {
                                found_by_kind.push(match g.total {
                                    n if n > g.keys.len() => {
                                        format!("{} of {}", g.keys.len(), count_of(n, g.kind))
                                    }
                                    n => count_of(n, g.kind),
                                });
                                g.keys
                            })
                            .collect()
                    }),
                    all.len(),
                ),
                None if mixed => (
                    self.search.filter_mixed(&self.base, &self.query),
                    self.base.len(),
                ),
                None => (
                    self.search.filter(kind, &self.base, &self.query),
                    self.base.len(),
                ),
            };
            match found {
                Some(keys) => {
                    log::debug!(
                        "search {:?}: {} of {} in {} µs",
                        self.query,
                        keys.len(),
                        of,
                        t.elapsed().as_micros()
                    );
                    let hint = match wide {
                        true => "No matches in any kind.".to_string(),
                        false => format!("No matches in {}.", cat.name()),
                    };
                    w.set_empty_hint(hint.into());
                    keys
                }
                None => {
                    let hint = match (wide || mixed, kind) {
                        (true, _) => "Indexing names…".to_string(),
                        (false, kind) => format!("Indexing {} names…", noun(kind)),
                    };
                    w.set_empty_hint(hint.into());
                    Vec::new()
                }
            }
        };
        w.set_channel_count(keys.len() as i32);
        // A widened search lists several kinds, as these do.
        let mixed = mixed || wide;
        // A list of several kinds counts titles, unless all are channels.
        let all_live = keys
            .iter()
            .all(|&k| store::from_list_key(k).0 == Kind::Live);
        let count = match cat {
            Category::Continue => count(keys.len() as i64, "title"),
            _ if mixed && !all_live => count(keys.len() as i64, "title"),
            _ => count_of(keys.len(), kind),
        };
        w.set_count_label(count.into());
        let keep = "Enter keeps the filter  ·  Esc clears it";
        w.set_search_scope(
            if self.wide.is_some() {
                "All kinds"
            } else {
                scope
            }
            .into(),
        );
        w.set_search_wide(self.wide.is_some());
        w.set_search_hint(
            match (&self.wide, found_by_kind.is_empty()) {
                (Some(_), false) => format!(
                    "{}  ·  Tab: {scope} only  ·  {keep}",
                    found_by_kind.join("  ·  ")
                ),
                (Some(_), true) => {
                    format!("in Live, Movies and Series  ·  Tab: {scope} only  ·  {keep}")
                }
                (None, _) => format!("in {}  ·  Tab: all kinds  ·  {keep}", cat.name()),
            }
            .into(),
        );
        // The guide grid is live only, so its hint goes with the kind;
        // Continue watching holds titles whichever kind is chosen.
        w.set_listing_titles(self.kind != Kind::Live || *cat == Category::Continue);
        w.set_can_clear(
            matches!(cat, Category::Recent | Category::Continue) && !self.base.is_empty(),
        );
        w.set_query(self.query.as_str().into());
        w.set_searching(self.searching);
        self.channels.set_searching(self.searching);
        self.channels
            .set_keys(kind, mixed, *cat == Category::Continue, wide, keys);
        w.set_mixed_kinds(self.channels.with_channels.get());
        if reset {
            w.invoke_show_category();
            self.select(0);
        } else {
            self.select(self.selected);
        }
    }

    /// `a` or the header's indicator: switches the order, staying on the same
    /// category and channel.
    fn toggle_order(&mut self) {
        self.order = match self.order {
            Order::Provider => Order::Az,
            Order::Az => Order::Provider,
        };
        let _ = self.write_tx.send(Write::Order(self.order));
        let key = self.channels.keys.borrow().get(self.selected).copied();
        if let Err(e) = self.reload() {
            log::error!("{e}");
            self.set_error(&e);
            return;
        }
        self.window().invoke_show_category();
        let position = self
            .channels
            .keys
            .borrow()
            .iter()
            .position(|&k| Some(k) == key);
        self.select(position.unwrap_or(0));
    }

    /// `e` or the library's funnel: opens manage mode.
    fn open_manage(&mut self) {
        if self.manage.is_some() {
            return;
        }
        self.manage = Some(Manage {
            kind: self.kind,
            ..Manage::default()
        });
        self.load_manage();
        let w = self.window();
        w.set_managing(true);
        w.invoke_manage_to_top();
    }

    /// Esc or Done: back to the library, which each edit has already
    /// re-read.
    fn close_manage(&mut self) {
        self.manage = None;
        self.manage_rows.set_vec(Vec::new());
        self.window().set_managing(false);
        self.select(self.selected);
    }

    /// Tab, Shift-Tab or a click on the kind switch: manage another kind's
    /// categories (VOD ticket 02). The filter stays.
    fn manage_kind(&mut self, kind: Kind) {
        let Some(m) = &mut self.manage else { return };
        if m.kind == kind {
            return;
        }
        m.kind = kind;
        m.selected = 0;
        self.load_manage();
        self.window().invoke_manage_to_top();
    }

    /// (Re-)reads manage mode's categories and rules, keeping the filter
    /// and the selected category.
    fn load_manage(&mut self) {
        let id = &self.sources[self.active].provider.id;
        let t = Instant::now();
        let Some((kind, names_read)) = self.manage.as_ref().map(|m| (m.kind, m.names_read)) else {
            return;
        };
        let live = kind == Kind::Live;
        let read = store::managed_categories(&self.conn, id, kind, self.order).and_then(|cats| {
            Ok((
                cats,
                store::category_rules(&self.conn, id, kind)?,
                // Name rules and kept tags are the channels' alone.
                if live {
                    store::channel_rules(&self.conn, id)?
                } else {
                    Vec::new()
                },
                if live {
                    store::kept_tags(&self.conn, id)?
                } else {
                    Vec::new()
                },
                if live {
                    store::shown_channel_count(&self.conn, id)?
                } else {
                    store::shown_title_count(&self.conn, id, kind)?
                },
                if names_read || !live {
                    None
                } else {
                    Some(store::channel_names_by_category(&self.conn, id)?)
                },
            ))
        });
        let (cats, rules, name_rules, kept, titles, names) = match read {
            Ok(read) => read,
            Err(e) => {
                log::error!("reading categories: {e}");
                self.set_error(&format!("reading categories: {e}"));
                return;
            }
        };
        let Some(m) = &mut self.manage else { return };
        let selected = m.selected_id().map(str::to_owned);
        m.folded = cats.iter().map(|c| store::fold(&c.name)).collect();
        if let Some(names) = names {
            m.names = names
                .into_iter()
                .map(|(cat, name)| (cat, store::fold(&name)))
                .collect();
            m.names_read = true;
        }
        let hidden: HashSet<&str> = cats
            .iter()
            .filter(|c| !c.shown)
            .map(|c| c.id.as_str())
            .collect();
        m.listed = if live {
            (0..m.names.len())
                .filter(|&i| !m.names[i].0.as_deref().is_some_and(|c| hidden.contains(c)))
                .collect()
        } else {
            Vec::new()
        };
        m.cats = cats;
        m.rules = rules;
        m.titles = titles;
        m.kept_hidden = m.hidden_by_kept(&kept);
        m.kept = kept;
        m.name_rules = name_rules
            .into_iter()
            .map(|r| {
                let n = m.hidden_by(&r);
                (r, n)
            })
            .collect();
        m.filter_rows();
        m.selected = selected
            .and_then(|s| m.rows.iter().position(|&i| m.cats[i].id == s))
            .unwrap_or(0);
        self.show_manage();
        log::debug!("manage mode read in {} µs", t.elapsed().as_micros());
    }

    /// Shows manage mode's rows, rules and counts. Rows are changed in place
    /// where the filter left their number alone, so a toggle keeps the
    /// scroll position.
    fn show_manage(&mut self) {
        let Some(m) = &self.manage else { return };
        let items: Vec<ManageItem> = m
            .rows
            .iter()
            .map(|&i| {
                let c = &m.cats[i];
                ManageItem {
                    name: c.name.as_str().into(),
                    shown: c.shown,
                    overridden: c.overridden,
                    channels: if c.hidden == 0 {
                        c.titles.to_string()
                    } else {
                        format!("{} of {}", c.titles - c.hidden, c.titles)
                    }
                    .into(),
                }
            })
            .collect();
        if items.len() == self.manage_rows.row_count() {
            for (i, item) in items.into_iter().enumerate() {
                if self.manage_rows.row_data(i).as_ref() != Some(&item) {
                    self.manage_rows.set_row_data(i, item);
                }
            }
        } else {
            self.manage_rows.set_vec(items);
        }
        let shown = m.cats.iter().filter(|c| c.shown).count();
        let titles = match m.kind {
            Kind::Live => count(m.titles, "channel"),
            Kind::Movie => count(m.titles, "movie"),
            Kind::Series => format!("{} series", m.titles),
        };
        let w = self.window();
        w.set_manage_kind(Kind::ALL.iter().position(|&k| k == m.kind).unwrap_or(0) as i32);
        w.set_manage_summary(format!("{shown} of {} shown · {titles}", m.cats.len()).into());
        w.set_manage_matching(if m.filter.is_empty() {
            "".into()
        } else {
            format!("{} matching", m.rows.len()).into()
        });
        w.set_manage_filter(m.filter.as_str().into());
        let rules: Vec<slint::SharedString> = m.rules.iter().map(|r| r.into()).collect();
        w.set_manage_rules(ModelRc::new(VecModel::from(rules)));
        w.set_manage_rule_action(m.rule_action().into());
        let name_rules: Vec<NameRuleItem> = m
            .name_rules
            .iter()
            .map(|(pattern, n)| NameRuleItem {
                pattern: pattern.into(),
                hidden: *n as i32,
            })
            .collect();
        w.set_manage_name_rules(ModelRc::new(VecModel::from(name_rules)));
        w.set_manage_name_action(m.name_rule_action().into());
        let kept: Vec<slint::SharedString> = m.kept.iter().map(|t| t.into()).collect();
        w.set_manage_kept_tags(ModelRc::new(VecModel::from(kept)));
        w.set_manage_kept_hidden(if m.kept.is_empty() {
            "".into()
        } else {
            format!("hides {}", m.kept_hidden).into()
        });
        w.set_manage_kept_action(m.kept_action().into());
        w.set_manage_selected(m.selected as i32);
        w.invoke_show_manage_selected();
    }

    fn manage_select(&mut self, i: usize) {
        let Some(m) = &mut self.manage else { return };
        m.selected = i.min(m.rows.len().saturating_sub(1));
        let selected = m.selected;
        let w = self.window();
        w.set_manage_selected(selected as i32);
        w.invoke_show_manage_selected();
    }

    /// Enter, a double click or the row's pill: shows or hides a category.
    fn manage_toggle(&mut self, i: usize) {
        self.manage_select(i);
        let Some(m) = &self.manage else { return };
        let Some(id) = m.selected_id() else {
            return;
        };
        let edit = LibraryEdit::Category(m.kind, id.to_owned());
        self.save_library(edit);
    }

    /// Ctrl+Enter or the button: the filter becomes a rule, or stops being
    /// one.
    fn manage_rule(&mut self) {
        let Some(m) = &self.manage else { return };
        if !m.filter.trim().is_empty() {
            let edit = LibraryEdit::Rule(m.kind, m.filter.clone());
            self.save_library(edit);
        }
    }

    /// Ctrl+H or the button: hide channels whose names have the filter, or
    /// stop hiding them.
    fn manage_name_rule(&mut self) {
        let Some(m) = &self.manage else { return };
        if m.kind == Kind::Live && !m.filter.trim().is_empty() {
            let edit = LibraryEdit::NameRule(m.filter.clone());
            self.save_library(edit);
        }
    }

    /// Ctrl+K or the button: keep the filter as a region tag, or stop
    /// keeping it.
    fn manage_kept_tag(&mut self) {
        let Some(m) = &self.manage else { return };
        if m.kind == Kind::Live && store::kept_tag(&m.filter).is_some() {
            let edit = LibraryEdit::KeptTag(m.filter.clone());
            self.save_library(edit);
        }
    }

    /// A kept tag's ×.
    fn remove_kept_tag(&mut self, i: usize) {
        let Some(tag) = self.manage.as_ref().and_then(|m| m.kept.get(i)) else {
            return;
        };
        let edit = LibraryEdit::KeptTag(tag.clone());
        self.save_library(edit);
    }

    /// A name rule's ×.
    fn remove_name_rule(&mut self, i: usize) {
        let Some((rule, _)) = self.manage.as_ref().and_then(|m| m.name_rules.get(i)) else {
            return;
        };
        let edit = LibraryEdit::NameRule(rule.clone());
        self.save_library(edit);
    }

    /// A rule's ×.
    fn remove_rule(&mut self, i: usize) {
        let Some(m) = &self.manage else { return };
        let Some(rule) = m.rules.get(i) else {
            return;
        };
        let edit = LibraryEdit::Rule(m.kind, rule.clone());
        self.save_library(edit);
    }

    fn save_library(&self, edit: LibraryEdit) {
        let _ = self.write_tx.send(Write::Library {
            provider_id: self.provider().id.clone(),
            edit,
        });
    }

    /// An allowlist change was saved: the library, All and manage mode are
    /// re-read.
    fn on_library(&mut self, result: Result<(), String>) {
        if let Err(e) = result.and_then(|()| self.reload()) {
            log::error!("{e}");
            self.set_error(&e);
        }
    }

    /// The filter changed: the list is re-filtered, back at the top.
    fn edit_manage_filter(&mut self, edit: impl FnOnce(&mut String)) {
        let Some(m) = &mut self.manage else { return };
        let t = Instant::now();
        edit(&mut m.filter);
        m.filter_rows();
        m.selected = 0;
        self.window().invoke_manage_to_top();
        self.show_manage();
        log::debug!("manage filter in {} µs", t.elapsed().as_micros());
    }

    /// A key in manage mode: typing edits the filter, as in search mode.
    fn manage_key(&mut self, text: &str, ctrl: bool) -> bool {
        use slint::platform::Key;
        let enter = text == "\n" || text.chars().eq([char::from(Key::Return)]);
        if ctrl && enter {
            self.manage_rule();
            return true;
        }
        if ctrl && text.eq_ignore_ascii_case("h") {
            self.manage_name_rule();
            return true;
        }
        // Some backends send the control character instead of the letter.
        if ctrl && matches!(text, "k" | "K" | "\u{b}") {
            self.manage_kept_tag();
            return true;
        }
        let is = |k: Key| text.chars().eq([char::from(k)]);
        let step = if is(Key::Tab) {
            Some(1)
        } else if is(Key::Backtab) {
            Some(-1)
        } else {
            None
        };
        if let Some(d) = step {
            let Some(m) = &self.manage else { return false };
            let i = Kind::ALL.iter().position(|&k| k == m.kind).unwrap_or(0) as isize;
            self.manage_kind(Kind::ALL[(i + d).rem_euclid(Kind::ALL.len() as isize) as usize]);
            return true;
        }
        let Some(key) = search_key(text, ctrl) else {
            return false;
        };
        let Some(sel) = self.manage.as_ref().map(|m| m.selected) else {
            return false;
        };
        let page = self.window().get_page_rows().max(1) as isize;
        let sel = sel as isize;
        let clamp = |i: isize| i.max(0) as usize;
        match key {
            SearchKey::Type(t) => self.edit_manage_filter(|f| f.push_str(&t)),
            SearchKey::Backspace => self.edit_manage_filter(|f| {
                f.pop();
            }),
            SearchKey::Keep => self.manage_toggle(sel as usize),
            SearchKey::Clear => self.close_manage(),
            SearchKey::Nav(a) => match a {
                Action::Move(d) => self.manage_select(clamp(sel + d)),
                Action::HalfPage(d) => self.manage_select(clamp(sel + d * (page / 2).max(1))),
                Action::Page(d) => self.manage_select(clamp(sel + d * page)),
                Action::Top => self.manage_select(0),
                Action::Bottom => self.manage_select(usize::MAX),
                #[cfg(feature = "devtools")]
                Action::Snapshot => crate::devtools::snapshot(&self.window()),
                _ => {}
            },
        }
        true
    }

    /// Drops `kind`'s names and builds them again, for an import of that
    /// kind or another provider.
    fn reindex(&mut self, kind: Kind) {
        self.search.drop_kind(kind);
        (self.rebuild_index)(self.provider().id.clone(), kind);
    }

    fn set_index(&mut self, provider_id: &str, index: search::Index) {
        if provider_id != self.provider().id {
            return;
        }
        self.search.set(index);
        if !self.query.is_empty() {
            self.show_keys(true);
        }
        if self.guide_open && !self.guide_query.is_empty() {
            self.show_guide_keys(true);
        }
    }

    /// `t`: opens the guide grid on the main view's category and channel,
    /// at now, or goes back to the list on the grid's (SPEC 8.2).
    fn toggle_guide(&mut self) {
        self.guide_open = !self.guide_open;
        self.window().set_guide_open(self.guide_open);
        if self.guide_open {
            let key = self.channels.keys.borrow().get(self.selected).copied();
            let now = import::unix_now();
            self.guide_anchor = None;
            self.guide
                .origin
                .set(guide_start(guide_window(now, self.guide_back).0));
            self.set_guide_view(guide_start(now));
            self.load_guide(true);
            let row = guide_row_for(&self.base, &self.guide.keys.borrow(), key);
            self.guide_select(row);
            return;
        }
        let key = self.guide.keys.borrow().get(self.guide_selected).copied();
        self.drop_guide_filter();
        self.guide.set_keys("", Vec::new());
        let position = |keys: &[i64]| key.and_then(|k| keys.iter().position(|&c| c == k));
        let mut at = position(&self.channels.keys.borrow());
        if at.is_none() && !self.query.is_empty() && position(&self.base).is_some() {
            // A kept filter hides it: drop the filter rather than lose it.
            self.query.clear();
            self.show_keys(false);
            at = position(&self.channels.keys.borrow());
        }
        // The list is drawn again, the detail panel rebuilt.
        self.select(at.unwrap_or(self.selected));
        self.window().invoke_show_selected();
    }

    /// `[` / `]` in the grid: the previous or next category of the library,
    /// which the main view follows. The grid starts at its top, at the same
    /// time.
    fn guide_category(&mut self, d: isize) {
        let i = self.category.saturating_add_signed(d);
        if i >= self.categories.len() || i == self.category {
            return;
        }
        if let Err(e) = self.set_category(i) {
            log::error!("{e}");
            self.set_error(&e);
        }
        self.drop_guide_filter();
        self.load_guide(true);
    }

    /// Leaving the grid or its category drops its filter (SPEC 8.2).
    fn drop_guide_filter(&mut self) {
        self.guide_query.clear();
        self.guide_searching = false;
        let w = self.window();
        w.set_guide_query("".into());
        w.set_guide_searching(false);
    }

    /// A key while typing in the grid's search field.
    fn edit_guide_search(&mut self, key: SearchKey) {
        match key {
            SearchKey::Type(text) => self.guide_query.push_str(&text),
            SearchKey::Backspace => {
                self.guide_query.pop();
            }
            SearchKey::Keep => {
                self.guide_searching = false;
                self.window().set_guide_searching(false);
                return;
            }
            SearchKey::Clear => {
                // Stay on the selected channel among them all.
                self.drop_guide_filter();
                self.show_guide_keys(false);
                return;
            }
            SearchKey::Nav(_) => unreachable!("handled by handle_key"),
        }
        self.show_guide_keys(true);
    }

    /// Reads the current category's guide channels. `reset` returns the
    /// grid to the top; otherwise the selection stays on its channel.
    fn load_guide(&mut self, reset: bool) {
        let id = self.provider().id.clone();
        let cat = &self.categories[self.category];
        let t = Instant::now();
        let keys = match cat.view(self.order) {
            Some(view) => store::guide_keys(&self.conn, &id, view),
            None => Ok(Vec::new()),
        };
        self.guide_base = keys.unwrap_or_else(|e| {
            log::error!("reading channels: {e}");
            self.set_error(&format!("reading channels: {e}"));
            Vec::new()
        });
        let archives = store::archive_days_by_key(&self.conn, &id).unwrap_or_else(|e| {
            log::error!("reading archives: {e}");
            HashMap::new()
        });
        let days = self.guide_base.iter().filter_map(|k| archives.get(k)).max();
        self.guide_back = guide_back(days.copied().unwrap_or(0));
        log::debug!(
            "guide {:?}: {} channels in {} µs",
            cat.name(),
            self.guide_base.len(),
            t.elapsed().as_micros()
        );
        self.show_guide_keys(reset);
    }

    /// Shows the grid's channels: its category's guide channels, filtered
    /// by its query if there is one, best match first (SPEC 8.5). `reset`
    /// returns the grid to the top; otherwise the selection stays on its
    /// channel.
    fn show_guide_keys(&mut self, reset: bool) {
        let started = Instant::now();
        let id = self.provider().id.clone();
        let name = self.categories[self.category].name().to_string();
        let w = self.window();
        let total = count(self.guide_base.len() as i64, "channel");
        let (keys, hint, counted) = if self.guide_query.is_empty() {
            let hint = format!("No channel in {name} has a guide.");
            (
                self.guide_base.clone(),
                hint,
                format!("{total} with a guide"),
            )
        } else if let Some(keys) =
            self.search
                .filter(Kind::Live, &self.guide_base, &self.guide_query)
        {
            let counted = format!("{} of {total} with a guide", keys.len());
            (keys, format!("No matches in {name}."), counted)
        } else {
            let counted = format!("{total} with a guide");
            (Vec::new(), "Indexing channel names…".into(), counted)
        };
        w.set_guide_empty_hint(hint.into());
        w.set_guide_count(counted.into());
        w.set_guide_query(self.guide_query.as_str().into());
        w.set_guide_searching(self.guide_searching);
        let key = match reset {
            true => None,
            false => self.guide.keys.borrow().get(self.guide_selected).copied(),
        };
        let now = import::unix_now();
        self.set_guide_view(guide_clamp(self.guide_view, now, self.guide_back));
        self.guide.sel.replace(None);
        if self.guide.provider_id.borrow().as_str() == id {
            self.guide.replace_keys(keys);
        } else {
            self.guide.set_keys(&id, keys);
        }
        if reset {
            self.window().invoke_guide_to_top();
        }
        let at = key.and_then(|k| self.guide.keys.borrow().iter().position(|&g| g == k));
        self.guide_select(at.unwrap_or(if reset { 0 } else { self.guide_selected }));
        if !self.guide_query.is_empty() {
            // The match and the new selection; the rows are built in the
            // next frame.
            log::debug!(
                "guide search {:?}: {} of {} in {} µs",
                self.guide_query,
                self.guide.row_count(),
                self.guide_base.len(),
                started.elapsed().as_micros()
            );
        }
    }

    /// The span from `view`: the header's ticks, the now line, and the
    /// offset every row's blocks move by. Rows hold a page around the span
    /// ([`guide_page`]), so they are read again only when the span leaves
    /// it. Returns whether they were.
    fn set_guide_view(&mut self, view: i64) -> bool {
        let now = import::unix_now();
        self.guide_on_now = view == guide_start(now);
        self.guide_view = view;
        let (from, to) = self.guide.page.get();
        let moved = view < from || view + GUIDE_SPAN > to;
        if moved {
            self.guide.page.set(guide_page(view));
        }
        let origin = self.guide.origin.get();
        let ticks: Vec<GuideTick> = guide_ticks(view)
            .into_iter()
            .map(|(t, x)| GuideTick {
                label: clock(t).into(),
                x,
            })
            .collect();
        let w = self.window();
        w.set_guide_ticks(ModelRc::new(VecModel::from(ticks)));
        w.set_guide_now((now - view) as f32 / GUIDE_SPAN as f32);
        w.set_guide_offset((view - origin) as f32 / GUIDE_SPAN as f32);
        if moved {
            self.guide.refresh();
        }
        moved
    }

    /// A click in the grid: channel `i` at `x` across the timeline, 0 to 1.
    /// Left of it, on the channel column, the channel at the selected time.
    fn guide_click(&mut self, i: usize, x: f32) {
        self.guide_selected = i.min(self.guide.row_count().saturating_sub(1));
        self.window().set_guide_selected(self.guide_selected as i32);
        if x < 0.0 {
            self.guide_reselect();
            return;
        }
        let now = import::unix_now();
        let Some(slots) = self.guide_slots(self.guide_selected, now) else {
            return;
        };
        let t = self.guide_view + (x * GUIDE_SPAN as f32) as i64;
        let slot = slots[slot_at(&slots, t)].clone();
        self.guide_anchor = guide_anchor(&slot, now);
        self.set_guide_cell(slot);
    }

    /// A horizontal drag or wheel: the span from `from`, moved back by `dx`
    /// timeline widths (a drag right goes back in time), within the window.
    fn guide_pan(&mut self, from: i64, dx: f32) {
        let now = import::unix_now();
        let view = from - (dx * GUIDE_SPAN as f32) as i64;
        self.set_guide_view(guide_clamp(view, now, self.guide_back));
    }

    /// Selects channel `i` of the grid, at the selected time.
    fn guide_select(&mut self, i: usize) {
        self.guide_selected = i.min(self.guide.row_count().saturating_sub(1));
        let w = self.window();
        w.set_guide_selected(self.guide_selected as i32);
        w.invoke_show_guide_selected();
        self.guide_reselect();
    }

    /// Selects the cell at the selected time on the selected channel.
    fn guide_reselect(&mut self) {
        let now = import::unix_now();
        let Some(slots) = self.guide_slots(self.guide_selected, now) else {
            self.guide.sel.replace(None);
            self.show_guide_detail();
            return;
        };
        let at = guide_time(self.guide_anchor, now, self.guide_back);
        let slot = slots[slot_at(&slots, at)].clone();
        self.set_guide_cell(slot);
    }

    /// Row `i`'s cells across the kept window: one channel's guide, read
    /// as it is needed (ADR 0003). `None` without the row.
    fn guide_slots(&self, i: usize, now: i64) -> Option<Vec<Slot>> {
        let key = *self.guide.keys.borrow().get(i)?;
        let (lo, hi) = guide_window(now, self.guide_back);
        let epg_id = read_channel(&self.conn, key, now)?.guide_id;
        let progs = epg_id.map_or_else(Vec::new, |epg_id| {
            store::programmes(&self.conn, &self.provider().id, &epg_id, lo, hi).unwrap_or_else(
                |e| {
                    log::error!("reading programmes: {e}");
                    Vec::new()
                },
            )
        });
        Some(guide_slots(&progs, lo, hi))
    }

    /// Selects `slot` on the selected channel, redrawing the rows the
    /// selection leaves and joins, and the detail strip.
    fn set_guide_cell(&mut self, slot: Slot) {
        self.guide.sel.replace(Some((self.guide_selected, slot)));
        self.guide.ask_first(self.guide_selected);
        self.show_guide_sel();
        self.show_guide_detail();
    }

    /// The selected cell, placed in the rows' page.
    fn show_guide_sel(&self) {
        let sel = self.guide.sel.borrow();
        let origin = self.guide.origin.get();
        let sel = sel.as_ref().map_or(
            GuideSel {
                row: -1,
                ..Default::default()
            },
            |(row, slot)| guide_sel(*row, slot, origin),
        );
        self.window().set_guide_sel(sel);
    }

    /// `h`/`l`: the previous or next cell on the channel, the timeline
    /// following as far as needed.
    fn guide_step(&mut self, d: isize) {
        let now = import::unix_now();
        let Some(slots) = self.guide_slots(self.guide_selected, now) else {
            return;
        };
        let from = self.guide.sel.borrow().as_ref().map(|(_, s)| s.from);
        let at = from.map_or_else(|| slot_at(&slots, now), |f| slot_at(&slots, f));
        let slot = slots[at.saturating_add_signed(d).min(slots.len() - 1)].clone();
        self.guide_anchor = guide_anchor(&slot, now);
        let back = self.guide_back;
        self.set_guide_view(guide_follow(self.guide_view, slot.from, slot.to, now, back));
        self.set_guide_cell(slot);
    }

    /// `n`, `<`, `>`, `P`: through time, on the same channel.
    fn jump_guide(&mut self, jump: Jump) {
        let now = import::unix_now();
        let at = guide_time(self.guide_anchor, now, self.guide_back);
        let (t, start) = guide_jump(jump, at, self.guide_view, now, self.guide_back);
        self.guide_anchor = (jump != Jump::Now).then_some(t);
        self.set_guide_view(start);
        self.guide_reselect();
    }

    /// The minute tick in the grid: the now line, the airing highlight, a
    /// span on the current half hour moving on with it, a selection on now
    /// following the clock, and the detail strip's times.
    fn guide_tick(&mut self) {
        let now = import::unix_now();
        let start = match self.guide_on_now {
            true => guide_start(now),
            false => guide_clamp(self.guide_view, now, self.guide_back),
        };
        if !self.set_guide_view(start) {
            self.guide.refresh();
        }
        if self.guide_anchor.is_some() {
            self.show_guide_detail();
            return;
        }
        self.guide_reselect();
        let cell = self
            .guide
            .sel
            .borrow()
            .as_ref()
            .map(|(_, s)| (s.from, s.to));
        if let Some((from, to)) = cell {
            let back = self.guide_back;
            self.set_guide_view(guide_follow(self.guide_view, from, to, now, back));
        }
    }

    /// The detail strip above the grid (SPEC 8.2): the selected cell.
    fn show_guide_detail(&self) {
        let now = import::unix_now();
        let sel = self.guide.sel.borrow();
        let detail = sel.as_ref().and_then(|(i, slot)| {
            let key = *self.guide.keys.borrow().get(*i)?;
            let row = read_channel(&self.conn, key, now)?;
            let channel = match row.num {
                Some(n) => format!("{n} · {}", row.name),
                None => row.name,
            };
            let Some(p) = &slot.programme else {
                return Some(GuideDetail {
                    channel: channel.into(),
                    title: NO_INFO.into(),
                    ..Default::default()
                });
            };
            let description = row.guide_id.and_then(|epg_id| {
                store::description(&self.conn, &self.provider().id, &epg_id, p.start)
                    .unwrap_or_else(|e| {
                        log::error!("reading a description: {e}");
                        None
                    })
            });
            let recording =
                store::active_recording(&self.conn, &self.provider().id, row.stream_id, p.start)
                    .unwrap_or_else(|e| {
                        log::error!("reading recordings: {e}");
                        None
                    });
            let replay = replayable(p, slot.to, row.archive_days, now);
            Some(GuideDetail {
                channel: channel.into(),
                title: p.title.as_str().into(),
                times: guide_times(p, now).into(),
                replay: match replay {
                    Some(Ok(())) => CATCH_UP_HINT,
                    None if row.archive_days > 0 && slot.from <= now => START_OVER_HINT,
                    _ => "",
                }
                .into(),
                description: description.unwrap_or_default().into(),
                airing: (slot.from..slot.to).contains(&now),
                record: recording
                    .as_ref()
                    .map(record_detail)
                    .unwrap_or_default()
                    .into(),
            })
        });
        self.window().set_guide_detail(detail.unwrap_or_default());
    }

    /// Enter in the grid (SPEC 8.2): a past programme the archive keeps
    /// plays from its start; on one it does not, or a past gap, the status
    /// bar says why. Now or later plays the channel live.
    fn guide_play(&mut self) {
        let now = import::unix_now();
        let Some((i, slot)) = self.guide.sel.borrow().clone() else {
            return;
        };
        let Some(key) = self.guide.keys.borrow().get(i).copied() else {
            return;
        };
        if slot.to > now {
            return self.play_key(key);
        }
        let Some(p) = slot.programme else {
            return self.set_status("Nothing to play: the guide has no programme here");
        };
        let Some(row) = read_channel(&self.conn, key, now) else {
            return;
        };
        match replayable(&p, slot.to, row.archive_days, now) {
            Some(Err(why)) => self.set_status(&why),
            // Cut where an overlapping next one starts, as drawn (SPEC 6.1).
            _ => self.play_from(key, Some(Programme { stop: slot.to, ..p })),
        }
    }

    /// `S` in the grid (SPEC 8.2): the selected programme, airing now, from
    /// its start; otherwise the status bar says why not.
    fn guide_start_over(&mut self) {
        let now = import::unix_now();
        let Some((i, slot)) = self.guide.sel.borrow().clone() else {
            return;
        };
        let Some(key) = self.guide.keys.borrow().get(i).copied() else {
            return;
        };
        if !(slot.from..slot.to).contains(&now) {
            return self.set_status("Start over is for the programme airing now");
        }
        let Some(row) = read_channel(&self.conn, key, now) else {
            return;
        };
        // Cut where an overlapping next one starts, as drawn (SPEC 6.1).
        let airing = slot.programme.map(|p| Programme { stop: slot.to, ..p });
        match start_over(airing, row.archive_days) {
            Ok(p) => self.play_from(key, Some(p)),
            Err(why) => self.set_status(why),
        }
    }

    /// `S` in the list (SPEC 8.3): the programme airing now on the channel
    /// playing, from its start; a movie or episode playing from its own
    /// (VOD ticket 07). Nothing while mpv is closed.
    fn start_over_playing(&mut self) {
        if self.movie_playing() {
            self.control(Control::Restart);
            return self.set_status("From the start");
        }
        let now = import::unix_now();
        let Some(key) = self
            .now_playing
            .as_ref()
            .filter(|_| self.mpv.is_some())
            .map(|np| np.key)
        else {
            return;
        };
        let Some(row) = read_channel(&self.conn, key, now) else {
            return;
        };
        match start_over(row.now, row.archive_days) {
            Ok(p) => self.play_from(key, Some(p)),
            Err(why) => self.set_status(why),
        }
    }

    /// Selects row `i` of the grid if it is open, else of the list.
    fn move_to(&mut self, i: usize) {
        if self.guide_open {
            self.guide_select(i);
        } else {
            self.select(i);
        }
    }

    /// The selected channel's row key, in the grid if it is open.
    fn selected_key(&self) -> Option<i64> {
        if self.guide_open {
            self.guide.keys.borrow().get(self.guide_selected).copied()
        } else {
            self.channels.keys.borrow().get(self.selected).copied()
        }
    }

    fn select(&mut self, i: usize) {
        let count = self.channels.row_count();
        self.selected = i.min(count.saturating_sub(1));
        let w = self.window();
        w.set_selected(self.selected as i32);
        w.invoke_show_selected();
        self.show_detail();
    }

    /// Fills the detail panel for the selected channel: now, as in its row,
    /// then the rest of today.
    fn show_detail(&mut self) {
        if self.listing() != Kind::Live {
            self.show_title_detail();
            return;
        }
        self.leave_series();
        let now = import::unix_now();
        let row = self.channels.row(self.selected, now);
        let w = self.window();
        w.set_detail_title(false);
        w.set_detail_series(false);
        w.set_detail_watched(false);
        w.set_detail_now_heading("NOW".into());
        w.set_detail_now_hint("".into());
        w.set_detail_now_bar(true);
        let key = self.channels.keys.borrow().get(self.selected).copied();
        w.set_detail_key(key.unwrap_or(0) as i32);
        let name = row.as_ref().map_or("", |r| &r.name);
        w.set_detail_name(name.into());
        w.set_detail_initials(initials(name).into());
        w.set_detail_tile(tile(name));
        let icon = row.as_ref().and_then(|r| r.icon_url.as_deref());
        let logo = self.logos.get(icon, DETAIL_LOGO);
        self.detail_logo = icon
            .filter(|_| logo.size().width == 0)
            .map(|u| (u.to_owned(), false));
        w.set_detail_logo(logo);
        w.set_detail_num(
            row.as_ref()
                .and_then(|r| r.num)
                .map(|n| n.to_string())
                .unwrap_or_default()
                .into(),
        );
        w.set_detail_favourite(row.as_ref().is_some_and(|r| r.favourite));
        w.set_detail_format(
            row.as_ref()
                .filter(|r| !r.extension.is_empty())
                .map(|r| format!(".{}", r.extension))
                .unwrap_or_default()
                .into(),
        );
        w.set_detail_category(
            match &row {
                Some(r) if !r.available => UNAVAILABLE,
                Some(r) => r.category.as_deref().unwrap_or(""),
                None => "",
            }
            .into(),
        );
        let later = match row.as_ref().and_then(|r| r.guide_id.as_deref()) {
            Some(epg_id) => store::later(
                &self.conn,
                &self.provider().id,
                epg_id,
                now,
                end_of_day(now),
            )
            .unwrap_or_else(|e| {
                log::error!("reading programmes: {e}");
                Vec::new()
            }),
            None => Vec::new(),
        };
        // Where the guide comes from (EPG sources ticket 02), named as
        // Settings › EPG names it.
        let source = match row.as_ref().and_then(|r| r.guide_id.as_deref()) {
            Some(g) => match store::source_ident(&self.conn, g) {
                Ok(Some(ident)) => {
                    let provider = self.provider();
                    let order = config::epg_order(provider);
                    let at = order
                        .iter()
                        .position(|s| !s.provider_guide && s.ident() == ident);
                    let label = at.map_or_else(
                        || config::source_label(&ident),
                        |i| config::epg_labels(provider)[i].clone(),
                    );
                    format!("from {label}")
                }
                Ok(None) => format!("from {}", self.provider().name),
                Err(e) => {
                    log::error!("reading guide sources: {e}");
                    String::new()
                }
            },
            None => String::new(),
        };
        w.set_detail_guide_source(source.into());
        let on_now = row.as_ref().and_then(|r| r.now.as_ref());
        w.set_detail_has_now(on_now.is_some());
        w.set_detail_now_title(on_now.map_or("", |p| &p.title).into());
        w.set_detail_now_times(on_now.map(|p| now_times(p, now)).unwrap_or_default().into());
        w.set_detail_progress(on_now.map_or(0.0, |p| progress(p, now)));
        // "No guide" only when there is nothing at all to show.
        let no_guide = match &row {
            Some(r) if r.available && on_now.is_none() && later.is_empty() => {
                match r.epg_channel_id {
                    None => "The provider doesn't link it to its programme guide.",
                    Some(_) => "The programme guide has nothing for it.",
                }
            }
            _ => "",
        };
        w.set_detail_no_guide(no_guide.into());
        // Rows are changed in place: a new model would rebuild the list and
        // lose its scroll position on every minute tick.
        let old = self.later.row_count();
        for (i, p) in later.iter().enumerate() {
            let item = LaterItem {
                time: clock(p.start).into(),
                title: p.title.as_str().into(),
            };
            if i < old {
                self.later.set_row_data(i, item);
            } else {
                self.later.push(item);
            }
        }
        for i in (later.len()..old).rev() {
            self.later.remove(i);
        }
        self.show_alternates(key);
    }

    /// The detail panel's other listings of channel `key`: how many on
    /// each move, their rows only while open. The selection in them is kept
    /// while the channel stays selected.
    fn show_alternates(&mut self, key: Option<i64>) {
        // A negative key is an unavailable favourite: no row to match.
        let key = key.filter(|&k| k > 0);
        if key != self.alternates.of {
            self.alternates = AlternatesPanel {
                of: key,
                ..AlternatesPanel::default()
            };
        }
        let mut keys = match key {
            Some(k) => store::alternates(&self.conn, &self.provider().id, k, ALTERNATES_MAX + 1)
                .unwrap_or_else(|e| {
                    log::error!("reading other listings: {e}");
                    Vec::new()
                }),
            None => Vec::new(),
        };
        let line = match keys.len() {
            0 => String::new(),
            1 => "+1 listing".into(),
            n if n > ALTERNATES_MAX => format!("+{ALTERNATES_MAX} listings or more"),
            n => format!("+{n} listings"),
        };
        keys.truncate(ALTERNATES_MAX);
        self.alternates.keys = keys;
        self.window().set_detail_alternates_line(line.into());
        let open = self.alternates.focused;
        self.focus_alternates(open);
    }

    /// `o`, or a click on the panel's chip: the other listings open in
    /// LATER's place, keys moving in them.
    fn open_alternates(&mut self) {
        if self.listing() != Kind::Live {
            return self.set_status("o shows a channel's other listings");
        }
        if self.alternates.keys.is_empty() {
            return self.set_status("No other listings of this channel");
        }
        self.focus_alternates(true);
    }

    /// Opens the other listings, reading their rows, or closes them.
    fn focus_alternates(&mut self, on: bool) {
        let on = on && !self.alternates.keys.is_empty();
        let items: Vec<AlternateItem> = match on {
            true => {
                let now = import::unix_now();
                self.alternates
                    .keys
                    .iter()
                    .map(|&k| match read_channel(&self.conn, k, now) {
                        Some(r) => AlternateItem {
                            num: r.num.map(|n| n.to_string()).unwrap_or_default().into(),
                            category: r.category.unwrap_or_default().into(),
                            name: r.name.into(),
                        },
                        None => AlternateItem::default(),
                    })
                    .collect()
            }
            false => Vec::new(),
        };
        let a = &mut self.alternates;
        a.focused = on;
        a.selected = a.selected.min(a.keys.len().saturating_sub(1));
        let selected = a.selected;
        let w = self.window();
        w.set_detail_alternates(ModelRc::new(VecModel::from(items)));
        w.set_detail_alternate(selected as i32);
        w.set_alternates_focused(on);
    }

    fn select_alternate(&mut self, i: usize) {
        self.alternates.selected = i.min(self.alternates.keys.len().saturating_sub(1));
        self.window()
            .set_detail_alternate(self.alternates.selected as i32);
    }

    /// Enter on an other listing, or a click: plays it, the list staying
    /// on the channel selected so the next can be tried.
    fn play_alternate(&mut self, i: usize) {
        if let Some(&key) = self.alternates.keys.get(i) {
            self.select_alternate(i);
            self.play_key(key);
        }
    }

    /// A key while in the other listings: `true` if it was theirs. Any
    /// other key goes back to the list, as in the episodes.
    fn alternates_key(&mut self, action: &Action) -> bool {
        let sel = self.alternates.selected as isize;
        match *action {
            Action::Move(d) => self.select_alternate((sel + d).max(0) as usize),
            Action::Top => self.select_alternate(0),
            Action::Bottom => self.select_alternate(usize::MAX),
            Action::Play => self.play_alternate(self.alternates.selected),
            Action::ClearFilter | Action::Alternates => self.focus_alternates(false),
            Action::Control(_)
            | Action::Record
            | Action::LastChannel
            | Action::Refresh
            | Action::Account
            | Action::Keys
            | Action::Recordings
            | Action::Settings
            | Action::Quit => return false,
            #[cfg(feature = "devtools")]
            Action::Snapshot => return false,
            _ => {
                self.focus_alternates(false);
                return false;
            }
        }
        true
    }

    /// Fills the detail panel for the selected movie or series with what
    /// its row holds (VOD ticket 03), and asks for a movie's info if it is
    /// not known (04); a series' comes with 05.
    fn show_title_detail(&mut self) {
        self.show_alternates(None);
        let kind = self.listing();
        let row = self.channels.title(self.selected).unwrap_or_default();
        let w = self.window();
        let key = self.channels.keys.borrow().get(self.selected).copied();
        w.set_detail_key(key.unwrap_or(0) as i32);
        w.set_detail_title(true);
        w.set_detail_name(row.name.as_str().into());
        w.set_detail_initials(initials(&row.name).into());
        w.set_detail_tile(tile(&row.name));
        let icon = row.poster_url.as_deref();
        let poster = self.logos.poster(icon, DETAIL_POSTER);
        self.detail_logo = icon
            .filter(|_| poster.size().width == 0)
            .map(|u| (u.to_owned(), true));
        w.set_detail_logo(poster);
        w.set_detail_num("".into());
        w.set_detail_favourite(row.favourite);
        w.set_detail_format(
            row.extension
                .as_ref()
                .map(|e| format!(".{e}"))
                .unwrap_or_default()
                .into(),
        );
        w.set_detail_category(row.category.as_deref().unwrap_or("").into());
        w.set_detail_no_guide("".into());
        self.later.set_vec(Vec::new());
        w.set_detail_plot(row.plot.as_deref().unwrap_or("").into());
        w.set_detail_series(kind == Kind::Series);
        if kind == Kind::Series {
            w.set_detail_meta(series_meta(&row).into());
            self.show_series(&row, key);
        } else {
            w.set_detail_meta(title_meta(&row).into());
            w.set_detail_facts(ModelRc::new(VecModel::from(title_facts(kind, &row))));
            self.leave_series();
        }
        self.show_resume(kind, &row);
        let w = self.window();
        w.set_detail_download("".into());
        w.set_detail_download_warn(false);
        // Each time, as a file restored from the trash changes no row.
        self.show_download_marks();
        if !row.info
            && let Some(key) = key
        {
            self.info_asked.insert((kind, row.stream_id), key);
            let _ = self.info_tx.send(info::Ask {
                provider_id: self.provider().id.clone(),
                kind,
                id: row.stream_id,
            });
        }
    }

    /// The NOW card as a movie's or series' resume point, or the Watched
    /// chip once it was watched to the end (VOD ticket 07).
    fn show_resume(&self, kind: Kind, row: &TitleRow) {
        let carry_on = self.categories[self.category] == Category::Continue;
        let resume = |p: &Progress| {
            let (from, left) = resume_lines(p);
            let times = [from, left]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("  ·  ");
            (times, part_way(p))
        };
        let mut watched = false;
        let card = match kind {
            Kind::Movie => match row.progress {
                Some(p) if p.pos > 0 => {
                    let (from, left) = resume_lines(&p);
                    Some(("RESUME", from, left, part_way(&p), START_OVER_HINT))
                }
                p => {
                    watched = p.is_some_and(|p| p.watched);
                    None
                }
            },
            _ => {
                match &self.series.up_next {
                    UpNext::Resume(e, p) => {
                        let (times, part) = resume(p);
                        // `S` plays it from the start only in Continue watching.
                        let hint = if carry_on { START_OVER_HINT } else { "" };
                        Some(("RESUME", episode_line(e), times, part, hint))
                    }
                    UpNext::Next(e) => {
                        let length = e.length.map(duration).unwrap_or_default();
                        Some(("UP NEXT", episode_line(e), length, None, ""))
                    }
                    UpNext::Finished => {
                        watched = true;
                        None
                    }
                    UpNext::Nothing | UpNext::Unknown => None,
                }
            }
        };
        let w = self.window();
        w.set_detail_watched(watched);
        w.set_detail_has_now(card.is_some());
        if let Some((heading, title, times, part, hint)) = card {
            w.set_detail_now_heading(heading.into());
            w.set_detail_now_title(title.into());
            w.set_detail_now_times(times.into());
            w.set_detail_now_bar(part.is_some());
            w.set_detail_progress(part.unwrap_or(0.0));
            w.set_detail_now_hint(hint.into());
        }
    }

    /// The series panel for series `row` (VOD ticket 05). Its episodes are
    /// read when it is newly selected or its info has come; the season and
    /// episode chosen are kept while it stays selected, and a new one opens
    /// on its episode to carry on with (VOD ticket 07). The marks of those
    /// played are read each time, as they change while one plays.
    fn show_series(&mut self, row: &TitleRow, key: Option<i64>) {
        let fresh = self.series.key != key;
        let provider_id = &self.provider().id;
        let progress = store::episode_progress(&self.conn, provider_id, row.stream_id)
            .unwrap_or_else(|e| {
                log::error!("reading series progress: {e}");
                HashMap::new()
            });
        let last = store::last_episode(&self.conn, provider_id, row.stream_id)
            .inspect_err(|e| log::error!("reading series progress: {e}"))
            .ok()
            .flatten();
        let downloads = store::episode_downloads(&self.conn, provider_id, row.stream_id)
            .unwrap_or_else(|e| {
                log::error!("reading downloads: {e}");
                HashMap::new()
            });
        let marks_changed = progress != self.series.progress || downloads != self.series.downloads;
        if fresh || self.series.loaded != row.info {
            let episodes = if row.info {
                store::episodes(&self.conn, &self.provider().id, row.stream_id).unwrap_or_else(
                    |e| {
                        log::error!("reading episodes: {e}");
                        Vec::new()
                    },
                )
            } else {
                Vec::new()
            };
            let p = &mut self.series;
            if fresh {
                *p = SeriesPanel {
                    key,
                    ..Default::default()
                };
            }
            p.seasons = episodes.iter().map(|e| e.season).collect();
            p.seasons.dedup();
            p.episodes = episodes;
            p.loaded = row.info;
            p.up_next = store::up_next(&p.episodes, last.as_ref());
            if let UpNext::Resume(e, _) | UpNext::Next(e) = p.up_next.clone() {
                p.season = p.seasons.iter().position(|&s| s == e.season).unwrap_or(0);
                let at = p.in_season().position(|x| x.stream_id == e.stream_id);
                p.selected = at.unwrap_or(0);
            }
            p.season = p.season.min(p.seasons.len().saturating_sub(1));
            p.progress = progress;
            p.downloads = downloads;
            self.show_season();
        } else if marks_changed {
            let p = &mut self.series;
            p.up_next = store::up_next(&p.episodes, last.as_ref());
            p.progress = progress;
            p.downloads = downloads;
            self.show_season();
        }
        let w = self.window();
        w.set_detail_episodes_note(
            match (row.info, self.series.episodes.is_empty()) {
                (false, _) => "Loading episodes…",
                (true, true) => "The provider lists no episodes.",
                (true, false) => "",
            }
            .into(),
        );
    }

    /// The season pills and the chosen season's episodes.
    fn show_season(&mut self) {
        let p = &mut self.series;
        let count = p.in_season().count();
        p.selected = p.selected.min(count.saturating_sub(1));
        let few = p.seasons.len() <= 3;
        let pills: Vec<slint::SharedString> = p
            .seasons
            .iter()
            .map(|s| {
                if few {
                    format!("Season {s}")
                } else {
                    format!("S{s}")
                }
                .into()
            })
            .collect();
        let items: Vec<EpisodeItem> = p
            .in_season()
            .map(|e| {
                episode_item(
                    e,
                    p.progress.get(&e.stream_id),
                    p.downloads.get(&e.stream_id),
                )
            })
            .collect();
        let w = self.window();
        w.set_detail_seasons(ModelRc::new(VecModel::from(pills)));
        w.set_detail_season(self.series.season as i32);
        w.set_detail_episodes(ModelRc::new(VecModel::from(items)));
        w.set_detail_episode(self.series.selected as i32);
        w.set_episodes_focused(self.series.focused);
    }

    /// Not on a series: no series panel, and keys move in the list.
    fn leave_series(&mut self) {
        if self.series.key.is_some() || self.series.focused {
            self.series = SeriesPanel::default();
            let w = self.window();
            w.set_episodes_focused(false);
            w.set_detail_seasons(ModelRc::default());
            w.set_detail_episodes(ModelRc::default());
        }
    }

    /// Enter or a double click on a series: keys move in its episodes.
    fn open_episodes(&mut self) {
        let Some(row) = self.channels.title(self.selected) else {
            return;
        };
        if !row.info {
            return self.set_status(&format!("Episodes of {} are loading", row.name));
        }
        if self.series.episodes.is_empty() {
            return self.set_status(&format!("The provider lists no episodes of {}", row.name));
        }
        self.focus_episodes(true);
    }

    /// Keys move in the episodes, or back in the list.
    fn focus_episodes(&mut self, on: bool) {
        self.series.focused = on;
        self.window().set_episodes_focused(on);
    }

    /// Episode `i` of the season shown, kept in view.
    fn select_episode(&mut self, i: usize) {
        let count = self.series.in_season().count();
        self.series.selected = i.min(count.saturating_sub(1));
        self.window()
            .set_detail_episode(self.series.selected as i32);
    }

    /// The series panel, if it shows `episode`'s series, selects its season
    /// and it, as the list follows the channel playing (VOD ticket 08).
    fn follow_episode(&mut self, episode: &Episode) {
        let p = &mut self.series;
        if p.key.is_none() || !p.episodes.iter().any(|e| e.stream_id == episode.stream_id) {
            return;
        }
        let Some(season) = p.seasons.iter().position(|&s| s == episode.season) else {
            return;
        };
        p.season = season;
        let at = p.in_season().position(|e| e.stream_id == episode.stream_id);
        p.selected = at.unwrap_or(0);
        self.show_season();
    }

    /// Season pill `i`, from its first episode.
    fn pick_season(&mut self, i: usize) {
        if i >= self.series.seasons.len() || i == self.series.season {
            return;
        }
        self.series.season = i;
        self.series.selected = 0;
        self.show_season();
    }

    /// A key while keys move in a series' episodes (VOD ticket 05): moving
    /// and Enter act on the episodes, `h` and `l` change season, Esc goes
    /// back to the list. The Player's keys, and the views over everything,
    /// work as anywhere; any other key goes back to the list and acts
    /// there. True if it was handled here.
    fn episodes_key(&mut self, action: &Action) -> bool {
        let page = 8;
        let sel = self.series.selected as isize;
        let to = |i: isize| i.max(0) as usize;
        match *action {
            Action::Move(d) => self.select_episode(to(sel + d)),
            Action::HalfPage(d) => self.select_episode(to(sel + d * page / 2)),
            Action::Page(d) => self.select_episode(to(sel + d * page)),
            Action::Top => self.select_episode(0),
            Action::Bottom => self.select_episode(usize::MAX),
            Action::Category(d) => {
                let last = self.series.seasons.len().saturating_sub(1) as isize;
                self.pick_season(to((self.series.season as isize + d).min(last)))
            }
            Action::Play => self.play_episode(false),
            Action::StartOver => self.play_episode(true),
            Action::Watched => self.toggle_watched_episode(),
            Action::Download => self.download_episode(),
            Action::DownloadSeason => self.download_season(),
            Action::ClearFilter => self.focus_episodes(false),
            Action::Control(_)
            | Action::Record
            | Action::LastChannel
            | Action::Refresh
            | Action::Account
            | Action::Keys
            | Action::Recordings
            | Action::Settings
            | Action::Quit => return false,
            #[cfg(feature = "devtools")]
            Action::Snapshot => return false,
            _ => {
                self.focus_episodes(false);
                return false;
            }
        }
        true
    }

    /// A title's info is in: its row, and the panel if it is selected, read
    /// it again. A series' that failed says so in place of its episodes.
    fn on_info(&mut self, filled: info::Filled) {
        let ask = &filled.ask;
        let Some(key) = self.info_asked.remove(&(ask.kind, ask.id)) else {
            return;
        };
        if ask.provider_id != self.provider().id || self.listing() != ask.kind {
            return;
        }
        let i = self.channels.keys.borrow().iter().position(|&k| k == key);
        let Some(i) = i else {
            return;
        };
        if !filled.ok {
            if ask.kind == Kind::Series && i == self.selected {
                self.window().set_detail_episodes_note(
                    "Couldn't load the episodes. Select the series again to retry.".into(),
                );
            }
            return;
        }
        self.channels.notify.row_changed(i);
        if i == self.selected {
            self.show_detail();
        }
    }

    /// Re-reads everything that depends on the current time.
    fn refresh_times(&mut self) {
        let t = Instant::now();
        self.channels.refresh();
        self.show_detail();
        if self.guide_open {
            self.guide_tick();
        }
        self.show_playing();
        // The expiry turns relative, and its colour, within 7 days.
        self.show_source_status();
        log::debug!("minute refresh in {} µs", t.elapsed().as_micros());
    }

    /// Recently played (SPEC 8.2): the last few, latest first.
    fn show_recent(&mut self) {
        let now = import::unix_now();
        self.recent =
            store::recent(&self.conn, &self.provider().id, RECENT_SHOWN).unwrap_or_else(|e| {
                log::error!("reading recently played: {e}");
                Vec::new()
            });
        let items: Vec<RecentItem> = self
            .recent
            .iter()
            .filter_map(|&key| read_channel(&self.conn, key, now))
            .map(|r| RecentItem {
                num: r.num.map(|n| n.to_string()).unwrap_or_default().into(),
                initials: initials(&r.name).into(),
                tile: tile(&r.name),
                logo: shown_logo(
                    &self.logos,
                    &mut self.shown_logos,
                    r.icon_url.as_deref(),
                    RECENT_LOGO,
                    false,
                ),
                name: r.name.into(),
            })
            .collect();
        self.window()
            .set_recent(ModelRc::new(VecModel::from(items)));
    }

    /// The NOW PLAYING card and the status bar's playing state, shown while
    /// mpv runs.
    fn show_playing(&mut self) {
        let now = import::unix_now();
        let w = self.window();
        if let (Some(m), Some(mpv)) = (&self.movie, &self.mpv) {
            let mut card = movie_playing(m, mpv);
            // An episode shows its series' cover.
            let (kind, id) = match m.item {
                Watchable::Movie(id) => (Kind::Movie, id),
                Watchable::Episode { series_id, .. } => (Kind::Series, series_id),
            };
            let url = store::poster_url_by_id(&self.conn, &m.provider_id, kind, id)
                .inspect_err(|e| log::error!("reading {}: {e}", kind.as_str()))
                .ok()
                .flatten();
            card.logo = shown_logo(
                &self.logos,
                &mut self.shown_logos,
                url.as_deref(),
                PLAYING_POSTER,
                true,
            );
            return w.set_playing(card);
        }
        let (Some(np), Some(mpv)) = (&mut self.now_playing, &self.mpv) else {
            w.set_playing(PlayingItem::default());
            return;
        };
        // Re-found by `stream_id`: an import may have changed its row key.
        let row = store::channel_key(&self.conn, &np.provider_id, np.stream_id)
            .inspect_err(|e| log::error!("reading channels: {e}"))
            .ok()
            .flatten()
            .and_then(|key| {
                np.key = key;
                read_channel(&self.conn, key, now)
            });
        let name = row.as_ref().map_or(np.name.as_str(), |r| &r.name);
        let on_now = row.as_ref().and_then(|r| r.now.as_ref());
        let icon = row.as_ref().and_then(|r| r.icon_url.as_deref());
        let logo = shown_logo(
            &self.logos,
            &mut self.shown_logos,
            icon,
            PLAYING_LOGO,
            false,
        );
        let behind = mpv.behind.at(Instant::now());
        if let Some(c) = &np.catch_up {
            // Times from the guide and mpv's `time-pos`, never `duration`
            // (SPEC 6.1).
            let p = &c.programme;
            let into = c.elapsed();
            let len = p.stop - p.start;
            w.set_playing(PlayingItem {
                visible: true,
                heading: catch_up_heading(mpv.paused).into(),
                behind: true,
                catch_up: true,
                movie: false,
                name: name.into(),
                initials: initials(name).into(),
                tile: tile(name),
                logo,
                title: p.title.as_str().into(),
                has_guide: true,
                span: format!("{}{}", day_prefix(p.start, now), span(p)).into(),
                left: format!("{} left", duration(len - into)).into(),
                progress: into as f32 / len.max(1) as f32,
                paused: mpv.paused,
                muted: mpv.muted,
                fullscreen: mpv.fullscreen,
                recording: Default::default(),
                start_over: Default::default(),
            });
            return;
        }
        w.set_playing(PlayingItem {
            visible: true,
            heading: playing_heading(mpv.paused, behind).into(),
            behind: !behind.is_zero(),
            catch_up: false,
            movie: false,
            name: name.into(),
            initials: initials(name).into(),
            tile: tile(name),
            logo,
            title: on_now.map_or(NO_EPG, |p| &p.title).into(),
            has_guide: on_now.is_some(),
            span: on_now.map_or_else(|| watching(np.since, now), span).into(),
            left: on_now.map(|p| time_left(p, now)).unwrap_or_default().into(),
            progress: on_now.map_or(0.0, |p| progress(p, now)),
            paused: mpv.paused,
            muted: mpv.muted,
            fullscreen: mpv.fullscreen,
            recording: mpv
                .recording
                .as_ref()
                .map(|(since, _)| rec_elapsed(since.elapsed()))
                .unwrap_or_default()
                .into(),
            start_over: match row.as_ref().is_some_and(|r| r.archive_days > 0) && on_now.is_some() {
                true => START_OVER_HINT.into(),
                false => Default::default(),
            },
        });
    }

    /// Only the REC indicator, each second: [`Screen::show_playing`] reads
    /// the database.
    fn show_recording(&self) {
        let w = self.window();
        let mut playing = w.get_playing();
        playing.recording = self
            .mpv
            .as_ref()
            .and_then(|m| m.recording.as_ref())
            .map(|(since, _)| rec_elapsed(since.elapsed()))
            .unwrap_or_default()
            .into();
        w.set_playing(playing);
    }

    /// A logo arrived from the loader: redraws what was waiting for it.
    /// A grid row's table is in: the row reads its guide again, and the
    /// selection with it, as a gap may now be a programme.
    fn on_table(&mut self, filled: tables::Filled) {
        if filled.provider_id != self.provider().id {
            return;
        }
        let i = self
            .guide
            .keys
            .borrow()
            .iter()
            .position(|&k| k == filled.key);
        let Some(i) = i else {
            return;
        };
        self.guide.notify.row_changed(i);
        if self.guide_open && i == self.guide_selected {
            self.guide_reselect();
        }
    }

    fn on_logo(&mut self, logo: logos::Logo) {
        let url = logo.url.clone();
        if !self.logos.insert(logo) {
            return;
        }
        self.channels.logo_ready(&url);
        self.guide.logo_ready(&url);
        if let Some((_, poster)) = self.detail_logo.take_if(|(u, _)| *u == url) {
            let image = match poster {
                true => self.logos.poster(Some(&url), DETAIL_POSTER),
                false => self.logos.get(Some(&url), DETAIL_LOGO),
            };
            self.window().set_detail_logo(image);
        }
        if self.shown_logos.remove(&url) {
            self.show_recent();
            self.show_playing();
        }
    }

    fn play(&mut self, i: usize) {
        match self.listing() {
            Kind::Live => {}
            Kind::Movie => return self.play_movie(i, false),
            Kind::Series if self.categories[self.category] == Category::Continue => {
                return self.play_up_next(false);
            }
            Kind::Series => return self.open_episodes(),
        }
        let key = self.channels.keys.borrow().get(i).copied();
        if let Some(key) = key {
            self.play_key(key);
        }
    }

    /// Plays movie row `i` in the Player, titled with its name (SPEC 9.1),
    /// from where it stopped unless `from_start` (VOD ticket 07).
    fn play_movie(&mut self, i: usize, from_start: bool) {
        let pressed = Instant::now();
        let Some(mut row) = self.channels.title(i) else {
            return;
        };
        if self.replaying {
            return;
        }
        let source = &self.sources[self.active];
        match movie_request(&source.client, &row) {
            Ok((url, title)) => {
                let target = self.vod_target(false, row.stream_id, url);
                log::info!("play movie {title}: {}", xtream::redact(&target.0));
                // The card's length is mpv's, not the info's.
                row.length = None;
                let meta = title_meta(&row);
                let start = row.progress.map(|p| p.pos).filter(|_| !from_start);
                let item = Watchable::Movie(row.stream_id);
                self.play_vod(target, title, row.name, meta, item, start, pressed);
            }
            Err(e) => self.set_error(&e),
        }
    }

    /// Plays the selected episode of the series shown (VOD ticket 05).
    fn play_episode(&mut self, from_start: bool) {
        let Some(row) = self.channels.title(self.selected) else {
            return;
        };
        let Some(episode) = self.series.in_season().nth(self.series.selected).cloned() else {
            return;
        };
        let start = self
            .series
            .progress
            .get(&episode.stream_id)
            .map(|p| p.pos)
            .filter(|_| !from_start);
        self.play_episode_of(row.stream_id, &row.name, &episode, start);
    }

    /// Plays series `series_id`'s `episode` from `start` seconds in, titled
    /// with the series' `name`, `S2 E4` and its name (SPEC 9.1).
    fn play_episode_of(
        &mut self,
        series_id: i64,
        name: &str,
        episode: &Episode,
        start: Option<i64>,
    ) {
        let pressed = Instant::now();
        if self.replaying {
            return;
        }
        let source = &self.sources[self.active];
        match episode_request(&source.client, name, episode) {
            Ok((url, title, line)) => {
                let target = self.vod_target(true, episode.stream_id, url);
                log::info!("play episode {title}: {}", xtream::redact(&target.0));
                let item = watchable(series_id, episode);
                self.play_vod(target, title, name.to_string(), line, item, start, pressed);
                self.follow_episode(episode);
            }
            Err(e) => self.set_error(&e),
        }
    }

    /// Enter in Continue watching on a series (VOD ticket 07): its episode
    /// part way, else the next, from the start with `from_start`. Until its
    /// episodes are read, or with nothing to carry on with, into its
    /// episodes as elsewhere.
    fn play_up_next(&mut self, from_start: bool) {
        let Some(row) = self.channels.title(self.selected) else {
            return;
        };
        match store::series_up_next(&self.conn, &row.provider_id, row.stream_id) {
            Ok(UpNext::Resume(e, p)) => {
                let start = (!from_start).then_some(p.pos);
                self.play_episode_of(row.stream_id, &row.name, &e, start)
            }
            Ok(UpNext::Next(e)) => self.play_episode_of(row.stream_id, &row.name, &e, None),
            Ok(_) => self.open_episodes(),
            Err(e) => log::error!("reading series progress: {e}"),
        }
    }

    /// The episode `d` along from the one playing, with its series' id and
    /// name; `None` when no episode plays, `Some(.., None)` past either end.
    fn episode_along(&self, d: isize) -> Option<(i64, String, Option<Episode>)> {
        let m = self.movie.as_ref()?;
        let Watchable::Episode {
            series_id,
            stream_id,
            ..
        } = m.item
        else {
            return None;
        };
        let episodes = store::episodes(&self.conn, &m.provider_id, series_id)
            .inspect_err(|e| log::error!("reading episodes: {e}"))
            .unwrap_or_default();
        let e = neighbour(&episodes, stream_id, d).cloned();
        Some((series_id, m.name.clone(), e))
    }

    /// An episode played to its end: the next one plays after a countdown
    /// that Esc, or Backspace in mpv, cancels (VOD ticket 08). None after
    /// the last.
    fn count_to_next(&mut self) {
        match self.episode_along(1) {
            Some((series_id, name, Some(episode))) => {
                let due = Instant::now() + NEXT_IN;
                self.next = Some(NextEpisode {
                    series_id,
                    name,
                    episode,
                    due,
                });
                self.count_down();
            }
            Some((_, name, None)) => {
                self.set_status(&format!("That was the last episode of {name}"))
            }
            None => {}
        }
    }

    /// Shows the seconds left before the next episode, in the status bar
    /// and on mpv's OSD, or plays it once they are up. False once there is
    /// nothing left to count.
    fn count_down(&mut self) -> bool {
        let Some(next) = &self.next else {
            return false;
        };
        let left = next.due.saturating_duration_since(Instant::now());
        if left.is_zero() {
            self.play_next();
            return false;
        }
        let secs = left.as_secs_f64().ceil() as u64;
        let line = episode_line(&next.episode);
        self.set_status(&format!("Next episode in {secs} s: {line}. Esc cancels"));
        let osd = format!(
            "Next in {secs} s: {}  {line}\nPgDn plays it now · Backspace cancels",
            next.name
        );
        let _ = self.player_tx.send(PlayerCommand::Show(osd));
        true
    }

    /// Plays the episode the countdown was for.
    fn play_next(&mut self) {
        if let Some(n) = self.next.take() {
            self.play_episode_resumed(n.series_id, &n.name, &n.episode);
        }
    }

    /// Plays series `series_id`'s `episode`, resuming it if part way.
    fn play_episode_resumed(&mut self, series_id: i64, name: &str, episode: &Episode) {
        let provider_id = self.provider().id.clone();
        let start = store::episode_progress(&self.conn, &provider_id, series_id)
            .inspect_err(|e| log::error!("reading series progress: {e}"))
            .ok()
            .and_then(|p| p.get(&episode.stream_id).map(|p| p.pos));
        self.play_episode_of(series_id, name, episode, start);
    }

    /// Stops the countdown to the next episode, if one runs.
    fn cancel_next(&mut self) {
        if self.next.take().is_some() {
            self.set_status("Next episode cancelled");
        }
    }

    /// Sends a play to the player task, returning its id.
    fn send_play(
        &mut self,
        url: String,
        title: String,
        start: Option<f64>,
        pressed: Instant,
    ) -> u64 {
        self.next = None;
        self.plays += 1;
        let _ = self.player_tx.send(PlayerCommand::Play(PlayRequest {
            id: self.plays,
            url,
            title,
            start,
            pressed,
        }));
        self.plays
    }

    /// Where a movie (or with `episode`, an episode) plays from: its
    /// downloaded file if it has one (VOD ticket 10), else `url`; `true`
    /// for the file.
    fn vod_target(&self, episode: bool, stream_id: i64, url: reqwest::Url) -> (String, bool) {
        match self.downloaded(episode, stream_id) {
            Some(file) => (file, true),
            None => (url.to_string(), false),
        }
    }

    /// Plays a movie or an episode from `target`, a URL or with `local`
    /// its downloaded file, in the Player, mpv titled `title`, from `start`
    /// seconds in; the NOW PLAYING card shows `name` and, below it, `meta`.
    #[allow(clippy::too_many_arguments)]
    fn play_vod(
        &mut self,
        (target, local): (String, bool),
        title: String,
        name: String,
        meta: String,
        item: Watchable,
        start: Option<i64>,
        pressed: Instant,
    ) {
        self.keep_progress(false);
        // From the provider it takes a connection; a file takes none.
        let provider_id = self.provider().id.clone();
        let watching = recorder::Command::Watching((!local).then(|| provider_id.clone()));
        let _ = self.recorder_tx.send(watching);
        let start = start.filter(|s| *s > 0).map(|s| s as f64);
        let play = self.send_play(target, title, start, pressed);
        // Recently played lists an episode's series (VOD ticket 11).
        let (kind, stream_id) = match item {
            Watchable::Movie(id) => (Kind::Movie, id),
            Watchable::Episode { series_id, .. } => (Kind::Series, series_id),
        };
        let _ = self.write_tx.send(Write::Played {
            provider_id: provider_id.clone(),
            kind,
            stream_id,
            at: import::unix_now(),
        });
        self.now_playing = None;
        let pos = start.unwrap_or(0.0);
        self.movie = Some(MoviePlaying {
            meta,
            name,
            pos,
            length: None,
            provider_id,
            item,
            play,
            heard: false,
            saved: pos,
            done: false,
        });
        self.set_status("");
        self.show_playing();
    }

    /// Saves where the movie or episode playing is, once mpv has said and
    /// if it moved; `ended` when it played to its end (VOD ticket 07).
    fn keep_progress(&mut self, ended: bool) {
        if let Some(save) = self.progress_save(ended) {
            let _ = self.write_tx.send(Write::Progress(save));
        }
    }

    /// What [`Screen::keep_progress`] saves, if anything.
    fn progress_save(&mut self, ended: bool) -> Option<ProgressSave> {
        let m = self.movie.as_mut().filter(|m| !m.done)?;
        if !ended && (!m.heard || m.pos == m.saved) {
            return None;
        }
        m.saved = m.pos;
        m.done = ended;
        Some(ProgressSave {
            provider_id: m.provider_id.clone(),
            item: m.item.clone(),
            pos: m.pos,
            length: m.length,
            ended,
            at: import::unix_now(),
        })
    }

    /// A position was saved: the rows and the detail panel show it, and
    /// Continue watching is read again, the selection following its title.
    fn on_progress(&mut self) {
        if self.categories[self.category] == Category::Continue && !self.guide_open {
            let key = self.channels.keys.borrow().get(self.selected).copied();
            if let Err(e) = self.load_view(false) {
                log::error!("{e}");
                return self.set_error(&e);
            }
            let at = key.and_then(|k| self.channels.keys.borrow().iter().position(|&c| c == k));
            if let Some(i) = at {
                self.select(i);
            }
            return;
        }
        if self.listing() != Kind::Live {
            self.channels.refresh();
            self.show_detail();
        }
    }

    /// Plays a channel by row key, selecting it if it is in the list shown.
    fn play_following(&mut self, key: i64) {
        let position = self.channel_position(key);
        if let Some(i) = position {
            self.select(i);
        }
        self.play_key(key);
    }

    fn play_key(&mut self, key: i64) {
        self.play_from(key, None);
    }

    /// Plays a channel by row key: live, or with `programme` that past
    /// programme from the channel's archive (SPEC 6.1).
    fn play_from(&mut self, key: i64, programme: Option<Programme>) {
        let pressed = Instant::now();
        let now = import::unix_now();
        let Some(row) = read_channel(&self.conn, key, now) else {
            return;
        };
        if !row.available {
            self.set_error(&format!("{} is no longer in the provider's list", row.name));
            return;
        }
        if self.replaying {
            return;
        }
        let source = &self.sources[self.active];
        let (url, catch_up) = match programme {
            None => (
                source.client.stream_url(row.stream_id, &row.extension),
                None,
            ),
            Some(p) => {
                let offset = match &source.info {
                    None => return self.set_error("Catch-up waits for the account check"),
                    Some(info) => info.server_offset(),
                };
                let Some(offset) = offset else {
                    return self.set_error(
                        "The provider sends no server time, so its archive can't be found",
                    );
                };
                let after = self.record_after;
                let (start, minutes, from) =
                    xtream::archive_request(p.start, p.stop, offset, after);
                let url =
                    source
                        .client
                        .timeshift_url(row.stream_id, &start, minutes, &row.extension);
                let catch_up = CatchUp {
                    programme: p,
                    from,
                    pos: 0.0,
                };
                (url, Some(catch_up))
            }
        };
        let replayed = catch_up.as_ref().map(|c| c.programme.title.as_str());
        let title = media_title(row.num, &row.name, replayed);
        match url {
            Ok(url) => {
                log::info!("play {title}: {}", xtream::redact(url.as_str()));
                let provider_id = self.provider().id.clone();
                let watching = recorder::Command::Watching(Some(provider_id.clone()));
                let _ = self.recorder_tx.send(watching);
                self.keep_progress(false);
                self.send_play(url.to_string(), title, None, pressed);
                let _ = self.write_tx.send(Write::Played {
                    provider_id: provider_id.clone(),
                    kind: Kind::Live,
                    stream_id: row.stream_id,
                    at: now,
                });
                self.movie = None;
                self.now_playing = Some(NowPlaying {
                    key,
                    provider_id,
                    stream_id: row.stream_id,
                    name: row.name,
                    since: now,
                    catch_up,
                });
                // The card shows what plays; clear any old error.
                self.set_status("");
                self.show_playing();
            }
            Err(e) => self.set_error(&e),
        }
    }

    /// `-`: the channel played before the current one; with mpv closed, the
    /// one played last.
    fn last_channel(&mut self) {
        let id = self.provider().id.clone();
        let current = self
            .now_playing
            .as_ref()
            .filter(|p| self.mpv.is_some() && p.provider_id == id)
            .map(|p| p.stream_id);
        match store::last_channel(&self.conn, &id, current) {
            Ok(Some(key)) => self.play_following(key),
            Ok(None) if current.is_none() => self.set_status("Recently played is empty"),
            Ok(None) => self.set_status("No earlier channel"),
            Err(e) => {
                log::error!("reading recently played: {e}");
                self.set_error(&format!("reading recently played: {e}"));
            }
        }
    }

    /// A channel play was saved: redraw the panel's list, and in the
    /// Recently played view re-read it, the selection following its channel.
    fn on_played(&mut self) {
        self.show_recent();
        if self.categories[self.category] != Category::Recent {
            return;
        }
        if self.guide_open {
            self.load_guide(false);
        }
        let key = self.channels.keys.borrow().get(self.selected).copied();
        if let Err(e) = self.load_view(false) {
            log::error!("{e}");
            self.set_error(&e);
            return;
        }
        let position = self
            .channels
            .keys
            .borrow()
            .iter()
            .position(|&k| Some(k) == key);
        if let Some(i) = position {
            self.select(i);
        }
    }

    /// `c` or the Clear button: the first press arms, a second within
    /// [`CLEAR_CONFIRM`] clears. Any other key disarms (see
    /// [`Screen::handle_key`]).
    fn clear_pressed(&mut self, again: &str) {
        let provider_id = self.provider().id.clone();
        let (write, what) = match self.categories[self.category] {
            Category::Recent => (Write::ClearHistory(provider_id), "recently played"),
            Category::Continue => (Write::ClearContinue(provider_id), "Continue watching"),
            _ => {
                self.set_status("c clears the Recently played or Continue watching list");
                return;
            }
        };
        if self.disarm_clear() {
            let _ = self.write_tx.send(write);
            return;
        }
        let w = self.window();
        w.set_clear_armed(true);
        self.set_status(&format!("{again} to clear {what}"));
        let weak = self.window.clone();
        self.clear_timer
            .start(slint::TimerMode::SingleShot, CLEAR_CONFIRM, move || {
                if let Some(w) = weak.upgrade() {
                    w.set_clear_armed(false);
                    w.set_status("".into());
                }
            });
    }

    /// Drops a pending clear and its status message. True if one was pending.
    fn disarm_clear(&mut self) -> bool {
        let w = self.window();
        if !w.get_clear_armed() {
            return false;
        }
        self.clear_timer.stop();
        w.set_clear_armed(false);
        self.set_status("");
        true
    }

    fn on_history_cleared(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            log::error!("{e}");
            self.set_error(&e);
            return;
        }
        self.show_recent();
        if self.categories[self.category] == Category::Recent
            && let Err(e) = self.load_view(true)
        {
            log::error!("{e}");
            self.set_error(&e);
            return;
        }
        self.set_status("Recently played cleared");
    }

    /// `w` in the list: the selected movie watched or not, or for a series
    /// the season its panel shows (VOD ticket 07). A season partly watched
    /// becomes all watched; only one all watched goes back.
    fn toggle_watched(&mut self) {
        let Some(row) = self.channels.title(self.selected) else {
            return self.set_status("w marks movies and episodes watched");
        };
        if self.listing() == Kind::Movie {
            let watched = !row.progress.is_some_and(|p| p.watched);
            let item = Watchable::Movie(row.stream_id);
            return self.mark_watched(vec![item], watched, &row.name);
        }
        if !row.info {
            return self.set_status(&format!("Episodes of {} are loading", row.name));
        }
        let p = &self.series;
        let Some(&season) = p.seasons.get(p.season) else {
            return self.set_status(&format!("The provider lists no episodes of {}", row.name));
        };
        let episodes: Vec<&Episode> = p.in_season().collect();
        let watched = !episodes
            .iter()
            .all(|e| p.progress.get(&e.stream_id).is_some_and(|p| p.watched));
        let items = episodes
            .iter()
            .map(|e| watchable(row.stream_id, e))
            .collect();
        self.mark_watched(items, watched, &format!("{} season {season}", row.name));
    }

    /// `w` in a series' episodes: the selected one watched or not.
    fn toggle_watched_episode(&mut self) {
        let Some(row) = self.channels.title(self.selected) else {
            return;
        };
        let p = &self.series;
        let Some(e) = p.in_season().nth(p.selected) else {
            return;
        };
        let watched = !p.progress.get(&e.stream_id).is_some_and(|p| p.watched);
        let item = watchable(row.stream_id, e);
        let what = format!("{} {}", row.name, episode_label(e));
        self.mark_watched(vec![item], watched, &what);
    }

    /// `d` (VOD ticket 10): queues the selected movie; on a series, says
    /// how to choose an episode.
    fn download_selected(&mut self) {
        let Some(row) = self.channels.title(self.selected) else {
            return self.set_status("d downloads movies and episodes");
        };
        if self.listing() != Kind::Movie {
            return self.set_status(
                "d downloads an episode: Enter, then d on it. D downloads the season shown",
            );
        }
        let Some(extension) = row.extension.clone() else {
            return self.set_error(&format!("The provider gives no format for {}", row.name));
        };
        let title = store::NewDownload {
            provider_id: row.provider_id.clone(),
            stream_id: row.stream_id,
            series_id: None,
            season: None,
            episode_num: None,
            title: row.name.clone(),
            episode: None,
            extension,
        };
        self.queue_downloads(vec![title], row.name, false);
    }

    /// `d` in a series' episodes: queues the selected one.
    fn download_episode(&mut self) {
        let Some(row) = self.channels.title(self.selected) else {
            return;
        };
        let Some(e) = self.series.in_season().nth(self.series.selected).cloned() else {
            return;
        };
        let what = format!("{} {}", row.name, episode_label(&e));
        match episode_download(&row, &e) {
            Some(title) => self.queue_downloads(vec![title], what, false),
            None => self.set_error(&format!("The provider gives no format for {what}")),
        }
    }

    /// `D`: queues every episode of the season the series' panel shows.
    fn download_season(&mut self) {
        let row = self.channels.title(self.selected);
        let Some(row) = row.filter(|_| self.listing() == Kind::Series) else {
            return self.set_status("D downloads a season of a series");
        };
        if !row.info {
            return self.set_status(&format!("Episodes of {} are loading", row.name));
        }
        let p = &self.series;
        let Some(&season) = p.seasons.get(p.season) else {
            return self.set_status(&format!("The provider lists no episodes of {}", row.name));
        };
        let titles: Vec<_> = p
            .in_season()
            .filter_map(|e| episode_download(&row, e))
            .collect();
        let what = format!("{} season {season}", row.name);
        if titles.is_empty() {
            return self.set_error(&format!("The provider gives no format for {what}"));
        }
        self.queue_downloads(titles, what, true);
    }

    /// Asks the recorder to queue `titles`, named `what` in the status bar
    /// once it has; `season` when they are a season's.
    fn queue_downloads(&mut self, titles: Vec<store::NewDownload>, what: String, season: bool) {
        self.dl_asked = (what, season);
        let _ = self.recorder_tx.send(recorder::Command::Download(titles));
    }

    /// A downloaded movie's (or with `episode`, episode's) file, if it is
    /// done and still there: it plays from that (VOD ticket 10).
    fn downloaded(&self, episode: bool, stream_id: i64) -> Option<String> {
        store::download_of(&self.conn, &self.provider().id, episode, stream_id)
            .inspect_err(|e| log::error!("reading downloads: {e}"))
            .ok()
            .flatten()
            .filter(|d| d.state == store::DlState::Done)
            .and_then(|d| d.file)
            .filter(|f| std::path::Path::new(f).exists())
    }

    /// Shows the download marks of the movie or series in the detail panel
    /// as they now are, changing only the episodes' rows that differ.
    fn show_download_marks(&mut self) {
        if !self.window().get_detail_title() || self.searching {
            return;
        }
        let Some(row) = self.channels.title(self.selected) else {
            return;
        };
        if self.listing() == Kind::Movie {
            let d = store::download_of(&self.conn, &row.provider_id, false, row.stream_id)
                .inspect_err(|e| log::error!("reading downloads: {e}"))
                .ok()
                .flatten();
            let (mark, warn) = download_mark(d.as_ref());
            let w = self.window();
            w.set_detail_download(mark.into());
            w.set_detail_download_warn(warn);
            return;
        }
        let downloads = store::episode_downloads(&self.conn, &row.provider_id, row.stream_id)
            .unwrap_or_else(|e| {
                log::error!("reading downloads: {e}");
                HashMap::new()
            });
        let model = self.window().get_detail_episodes();
        let p = &mut self.series;
        p.downloads = downloads;
        for (i, e) in p.in_season().enumerate() {
            let mark: slint::SharedString = download_mark(p.downloads.get(&e.stream_id)).0.into();
            if let Some(mut item) = model.row_data(i)
                && item.download != mark
            {
                item.download = mark;
                model.set_row_data(i, item);
            }
        }
    }

    /// Saves `items` watched or not, saying so as `what`.
    fn mark_watched(&mut self, items: Vec<Watchable>, watched: bool, what: &str) {
        let _ = self.write_tx.send(Write::Watched {
            provider_id: self.provider().id.clone(),
            items,
            watched,
        });
        let state = if watched { "watched" } else { "not watched" };
        self.set_status(&format!("{what}: {state}"));
    }

    /// Continue watching was emptied: the view shows it if open.
    fn on_continue_cleared(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            log::error!("{e}");
            self.set_error(&e);
            return;
        }
        if self.categories[self.category] == Category::Continue
            && let Err(e) = self.load_view(true)
        {
            log::error!("{e}");
            self.set_error(&e);
            return;
        }
        self.set_status("Continue watching cleared");
    }

    fn play_recent(&mut self, i: usize) {
        if let Some(&key) = self.recent.get(i) {
            self.play_following(key);
        }
    }

    /// A movie is in mpv: the live-only keys say so (VOD ticket 04).
    fn movie_playing(&self) -> bool {
        self.mpv.is_some() && self.movie.is_some()
    }

    /// Pause, go live, stop, mute, fullscreen: sent only while mpv runs.
    fn control(&self, control: Control) {
        if self.mpv.is_some() {
            let _ = self.player_tx.send(PlayerCommand::Control(control));
        }
    }

    /// `L`: back to live, saying `LIVE`. From catch-up, the same channel's
    /// live stream; otherwise the Player reloads it if behind live.
    fn go_live(&mut self) {
        if self.movie_playing() {
            return self.set_status("Go live is for channels");
        }
        let key = self
            .now_playing
            .as_ref()
            .filter(|np| np.catch_up.is_some() && self.mpv.is_some())
            .map(|np| np.key);
        if let Some(key) = key {
            return self.back_to_live(key, "LIVE");
        }
        let behind = self
            .mpv
            .as_ref()
            .is_some_and(|m| !m.behind.at(Instant::now()).is_zero());
        self.control(Control::Live);
        if behind {
            self.set_status("LIVE");
        }
    }

    /// From catch-up, plays channel `key` live and says `message` in the
    /// status bar, unless that failed and the status bar says why. In the
    /// grid, a selection on that channel goes back to now, as `n` does.
    fn back_to_live(&mut self, key: i64, message: &str) {
        self.play_key(key);
        let live = self
            .now_playing
            .as_ref()
            .is_some_and(|np| np.catch_up.is_none());
        if !live {
            return;
        }
        let selected = self.guide.keys.borrow().get(self.guide_selected).copied();
        if self.guide_open && selected == Some(key) {
            self.jump_guide(Jump::Now);
        }
        self.set_status(message);
    }

    /// What `R` records at `now`: the channel playing and the programme
    /// airing, as the recorder stores it (its `stop` unused).
    fn playing_programme(&self, now: i64) -> Option<store::NewRecording> {
        let np = self.now_playing.as_ref()?;
        let row = read_channel(&self.conn, np.key, now);
        Some(store::NewRecording {
            provider_id: np.provider_id.clone(),
            stream_id: np.stream_id,
            channel: row.as_ref().map_or(&np.name, |r| &r.name).clone(),
            guide_id: row.as_ref().and_then(|r| r.guide_id.clone()),
            title: row.and_then(|r| r.now).map(|p| p.title),
            start: now,
            stop: now,
        })
    }

    /// `R`: records the channel playing, or stops recording it.
    fn record(&mut self) {
        if self.movie_playing() {
            return self.set_status("Recording is for channels");
        }
        let Some(mpv) = &self.mpv else {
            return;
        };
        let Some(np) = &self.now_playing else {
            return;
        };
        if np.catch_up.is_some() {
            return self.set_status("Recording catch-up is not supported yet");
        }
        if mpv.recording.is_some() {
            let _ = self.player_tx.send(PlayerCommand::Record(None));
            return;
        }
        let now = import::unix_now();
        let Some(r) = self.playing_programme(now) else {
            return;
        };
        let name = recording_name(&r.channel, r.title.as_deref(), now);
        match reserve_recording(&self.recordings_dir, &name) {
            Ok(file) => {
                let _ = self.player_tx.send(PlayerCommand::Record(Some(file)));
            }
            Err(e) => {
                let dir = self.recordings_dir.display();
                log::warn!("cannot record to {dir}: {e}");
                self.set_error(&format!("Can't record to {dir}: {e}"));
            }
        }
    }

    /// `R` in the guide grid: schedules the selected programme, or cancels
    /// it if scheduled or recording (SPEC 8.2, 9.5). One airing now starts
    /// at once; the recorder says how it went.
    fn record_programme(&mut self) {
        let now = import::unix_now();
        let Some((i, slot)) = self.guide.sel.borrow().clone() else {
            return;
        };
        let p = match recordable(slot, now) {
            Ok(p) => p,
            Err(why) => return self.set_status(why),
        };
        let key = self.guide.keys.borrow().get(i).copied();
        let Some(row) = key.and_then(|key| read_channel(&self.conn, key, now)) else {
            return;
        };
        let provider_id = self.provider().id.clone();
        let command =
            match store::active_recording(&self.conn, &provider_id, row.stream_id, p.start) {
                Ok(Some(r)) => recorder::Command::Cancel(r.id),
                Ok(None) => recorder::Command::Schedule(store::NewRecording {
                    provider_id,
                    stream_id: row.stream_id,
                    channel: row.name,
                    guide_id: row.guide_id,
                    title: Some(p.title),
                    start: p.start,
                    stop: p.stop,
                }),
                Err(e) => {
                    log::error!("reading recordings: {e}");
                    return;
                }
            };
        let _ = self.recorder_tx.send(command);
    }

    /// mpv started, stopped, or reported a control's state.
    fn on_player(&mut self, event: Event) {
        match event {
            Event::Recording(file) => {
                self.set_status(&format!("Recording to {}", file.display()));
                // Stored by the recorder, for the Recordings view.
                if let Some(r) = self.playing_programme(import::unix_now()) {
                    let _ = self
                        .recorder_tx
                        .send(recorder::Command::Playing(r, file.clone()));
                }
                let now = Instant::now();
                self.mpv
                    .iter_mut()
                    .for_each(|m| m.recording = Some((now, file.clone())));
            }
            Event::RecordingEnded(error) => {
                let file = self.mpv.as_mut().and_then(|m| m.recording.take());
                // It removes a file left empty: the name was only reserved.
                let ended = recorder::Command::PlayingEnded(error.clone());
                let _ = self.recorder_tx.send(ended);
                match (error, file) {
                    (Some(e), _) => {
                        log::warn!("recording failed: {e}");
                        self.set_error(&format!("Recording stopped: {e}"));
                    }
                    (None, Some((_, file))) => {
                        self.set_status(&format!("Recorded to {}", file.display()));
                    }
                    (None, None) => {}
                }
            }
            Event::Started => self.mpv = Some(MpvState::default()),
            Event::Stopped => {
                self.keep_progress(false);
                self.next = None;
                self.mpv = None;
                let _ = self.recorder_tx.send(recorder::Command::Watching(None));
            }
            Event::Paused(on) => self.mpv.iter_mut().for_each(|m| m.paused = on),
            Event::Muted(on) => self.mpv.iter_mut().for_each(|m| m.muted = on),
            Event::Fullscreen(on) => self.mpv.iter_mut().for_each(|m| m.fullscreen = on),
            Event::Behind(b) => self.mpv.iter_mut().for_each(|m| m.behind = b),
            Event::Handed(play) => {
                self.handed = play;
                return;
            }
            Event::Position(pos) if self.movie.is_some() => {
                let m = self.movie.as_mut().unwrap();
                // An answer about what played before it.
                if m.play != self.handed {
                    return;
                }
                m.pos = pos;
                m.heard = true;
                if m.length.is_none() {
                    let _ = self.player_tx.send(PlayerCommand::Length);
                }
                if (m.pos - m.saved).abs() >= SAVE_EVERY {
                    self.keep_progress(false);
                }
            }
            Event::Length(len) => match self.movie.as_mut() {
                Some(m) if m.play == self.handed => m.length = Some(len),
                _ => return,
            },
            Event::Position(pos) => {
                let catch_up = self
                    .now_playing
                    .as_mut()
                    .and_then(|np| np.catch_up.as_mut());
                match catch_up {
                    Some(c) => c.pos = pos,
                    None => return,
                }
            }
            Event::Ended if self.movie.as_ref().is_some_and(|m| m.play == self.handed) => {
                self.keep_progress(true);
                return self.count_to_next();
            }
            Event::Ended => return self.caught_up(),
            Event::Reconnecting(true) => return self.set_status("RECONNECTING"),
            // Back, or the user did something else: unless that said
            // something since.
            Event::Reconnecting(false) => {
                if self.window().get_status() == "RECONNECTING" {
                    self.set_status("");
                }
                return;
            }
            Event::Failed { .. } | Event::Remote(_) => return,
        }
        self.show_playing();
    }

    /// What plays ran out. A start over that reaches its end goes live by
    /// itself (catch-up ticket 04); a past programme's end is left as mpv
    /// leaves it.
    fn caught_up(&mut self) {
        let Some(np) = &self.now_playing else {
            return;
        };
        // Started over: the programme had not ended when it was played.
        if np
            .catch_up
            .as_ref()
            .is_some_and(|c| c.programme.stop > np.since)
        {
            self.back_to_live(np.key, "CAUGHT UP: LIVE");
        }
    }

    /// What the recorder reports (SPEC 9.5), in the status bar.
    fn on_recorder(&mut self, event: recorder::Event) {
        // Each second while a download runs: only what shows its progress.
        if let recorder::Event::DlProgress { .. } = event {
            if self.recordings.as_ref().is_some_and(|v| v.downloads) {
                self.show_recordings();
            }
            self.show_download_marks();
            return;
        }
        let what = rec_what;
        // The grid's marks and detail strip, and the Recordings view,
        // follow the stored states.
        if self.guide_open {
            self.guide.refresh();
            self.show_guide_detail();
        }
        self.load_recordings();
        match event {
            recorder::Event::Scheduled(r) => {
                let when = format!("{} – {}", clock(r.padded_start), clock(r.padded_stop));
                self.set_status(&format!("Scheduled to record {}, {when}", what(&r)));
            }
            recorder::Event::Started(r) => self.set_status(&format!("Recording {}", what(&r))),
            recorder::Event::Ended(r) => match r.state {
                store::RecState::Failed => {
                    let note = r.note.as_deref().unwrap_or("unknown error");
                    self.set_error(&format!("Recording {} failed: {note}", what(&r)));
                }
                store::RecState::Cancelled => {
                    self.set_status(&format!("Cancelled recording {}", what(&r)));
                }
                _ => match &r.file {
                    Some(file) => self.set_status(&format!("Recorded to {file}")),
                    None => self.set_status(&format!("Recorded {}", what(&r))),
                },
            },
            recorder::Event::Missed(n) => {
                let n = count(n as i64, "scheduled recording");
                self.set_error(&format!("Missed {n}: the app was not running"));
            }
            recorder::Event::Listed => {}
            recorder::Event::Cleared(n, with_file) => {
                self.set_status(&cleared_message(n, with_file, "recording"));
            }
            recorder::Event::Deleted(r, deleted) => {
                match deleted_message(&r, &deleted, &self.recordings_dir) {
                    (message, false) => self.set_status(&message),
                    (message, true) => self.set_error(&message),
                }
            }
            recorder::Event::DlProgress { .. } => {}
            recorder::Event::DlQueued(q) => {
                self.set_status(&queued_message(&self.dl_asked.0, self.dl_asked.1, q))
            }
            recorder::Event::DlStarted(d, resumed) => {
                let verb = if resumed {
                    "Resumed downloading"
                } else {
                    "Downloading"
                };
                self.set_status(&format!("{verb} {}", dl_what(&d)));
            }
            recorder::Event::DlPaused(d) => self.set_status(&format!(
                "Paused downloading {}: its connection is needed. It carries on once one is free",
                dl_what(&d)
            )),
            recorder::Event::DlEnded(d) => match (d.state, &d.file) {
                (store::DlState::Done, Some(file)) => {
                    self.set_status(&format!("Downloaded to {file}"))
                }
                _ => {
                    let note = d.note.as_deref().unwrap_or("unknown error");
                    self.set_error(&format!(
                        "Downloading {} failed: {note}. d tries again",
                        dl_what(&d)
                    ));
                }
            },
            recorder::Event::DlCancelled(d) => {
                self.set_status(&format!("Cancelled downloading {}", dl_what(&d)));
            }
            recorder::Event::DlDeleted(d, deleted) => {
                match dl_deleted_message(&d, &deleted, &self.downloads_dir) {
                    (message, false) => self.set_status(&message),
                    (message, true) => self.set_error(&message),
                }
            }
            recorder::Event::DlCleared(n, with_file) => {
                self.set_status(&cleared_message(n, with_file, "download"));
            }
            recorder::Event::ProviderRemoved(id, removed) => {
                if !removed {
                    self.set_error(&format!("Could not delete {id}'s data: see the log"));
                }
            }
        }
        self.show_download_marks();
    }

    /// A key pressed inside mpv (SPEC 9.2). True if a digit was typed, so
    /// the caller (re)starts the [`NUMBER_IDLE`] timer.
    fn on_remote(&mut self, r: Remote) -> bool {
        match r {
            Remote::Prev => self.surf(-1),
            Remote::Next => self.surf(1),
            Remote::Digit(d) => {
                let typed = self.number.push(d, Instant::now()).to_string();
                self.set_status(&format!("Channel {typed}…"));
                return true;
            }
            Remote::Back => {
                self.cancel_next();
                self.raise();
            }
            Remote::Live => self.go_live(),
            Remote::Record => self.record(),
            Remote::StartOver => self.start_over_playing(),
        }
        false
    }

    /// Plays the previous or next channel in the list as shown (category and
    /// search filter), from the one playing, skipping unavailable favourites.
    fn surf(&mut self, d: isize) {
        // An episode playing, or just ended: the one before or after it
        // (VOD ticket 08).
        if let Some((series_id, name, e)) = self.episode_along(d) {
            match e {
                Some(e) => self.play_episode_resumed(series_id, &name, &e),
                None if d > 0 => self.set_status(&format!("That is the last episode of {name}")),
                None => self.set_status(&format!("That is the first episode of {name}")),
            }
            return;
        }
        if self.listing() != Kind::Live {
            return;
        }
        let target = {
            let keys = self.channels.keys.borrow();
            let from = self
                .now_playing
                .as_ref()
                .and_then(|p| keys.iter().position(|&k| k == p.key))
                .unwrap_or(self.selected);
            let mut i = from as isize + d;
            // Past unavailable favourites, movies and series.
            while keys
                .get(i as usize)
                .is_some_and(|&k| k < 0 || store::from_list_key(k).0 != Kind::Live)
            {
                i += d;
            }
            (i >= 0 && (i as usize) < keys.len()).then_some(i as usize)
        };
        if let Some(i) = target {
            self.select(i);
            self.play(i);
        }
    }

    /// Plays the typed number once it is due. A channel outside the current
    /// list is looked up in All, the filter dropped, so the selection can
    /// follow it.
    fn play_number(&mut self) {
        let Some(num) = self.number.take_due(Instant::now()) else {
            return;
        };
        let found = store::channel_by_num(&self.conn, &self.provider().id, num)
            .inspect_err(|e| log::error!("reading channels: {e}"))
            .ok()
            .flatten();
        let Some(key) = found else {
            self.set_error(&format!("No channel {num}"));
            return;
        };
        let position = |s: &Self| s.channel_position(key);
        if position(self).is_none() {
            self.query.clear();
            self.searching = false;
            if let Err(e) = self.all_channels() {
                log::error!("{e}");
                self.set_error(&e);
                return;
            }
        }
        match position(self) {
            Some(i) => {
                self.select(i);
                self.play(i);
            }
            // In a hidden category (ticket 20): plays, unselected.
            None => self.play_key(key),
        }
    }

    /// Backspace in mpv: back to the list. winit's `focus_window` does
    /// nothing on Wayland; a critical attention request is sent as
    /// xdg-activation, which Umbriel grants (SPEC 9.2).
    fn raise(&self) {
        use slint::winit_030::{WinitWindowAccessor, winit::window::UserAttentionType};
        self.window().window().with_winit_window(|w| {
            w.request_user_attention(Some(UserAttentionType::Critical));
        });
    }

    /// The status bar's message. The provider is named beside it.
    fn set_status(&self, status: &str) {
        let w = self.window();
        w.set_status(status.into());
        w.set_status_error(false);
    }

    /// As [`Screen::set_status`], shown as a problem.
    fn set_error(&self, error: &str) {
        let w = self.window();
        w.set_status(error.into());
        w.set_status_error(true);
    }

    fn save_favourite(&self, cmd: FavCommand) {
        let _ = self.write_tx.send(Write::Favourite {
            provider_id: self.provider().id.clone(),
            cmd,
        });
    }

    /// A favourites change was saved: redraw the stars and the detail
    /// panel's chip, and in the Favourites list re-read it, the selection
    /// following a moved entry.
    fn on_favourite(&mut self, cmd: FavCommand, result: Result<bool, String>) {
        let changed = match result {
            Ok(changed) => changed,
            Err(e) => {
                log::error!("{e}");
                self.set_error(&e);
                return;
            }
        };
        if !matches!(self.categories[self.category], Category::Favourites) {
            self.channels.refresh();
            self.guide.refresh();
            if !self.guide_open {
                self.show_detail();
            }
            return;
        }
        if self.guide_open {
            self.load_guide(false);
        }
        if let (FavCommand::Move(_, _, d), true) = (cmd, changed) {
            self.selected = self.selected.saturating_add_signed(d);
        }
        if let Err(e) = self.load_view(false) {
            log::error!("{e}");
            self.set_error(&e);
        }
    }

    /// Records what provider `i`'s refresh scheduler reports; for the
    /// provider shown, also shows it and re-reads what it replaced. Another
    /// provider's data is read when it is switched to.
    fn on_refresh(&mut self, i: usize, update: Update) {
        let source = &mut self.sources[i];
        match &update {
            Update::Busy(text) => {
                source.refresh.clone_from(text);
                source.refresh_error = false;
                source.busy = true;
            }
            Update::Account(a) => {
                let max = a.user.max_connections;
                let limit = recorder::Command::Limit(source.provider.id.clone(), max);
                let _ = self.recorder_tx.send(limit);
                if let Some(offset) = a.server_offset() {
                    let id = source.provider.id.clone();
                    self.guide.offsets.borrow_mut().insert(id, offset);
                }
                source.info = Some((**a).clone());
            }
            Update::Idle {
                updated,
                error,
                retry_at,
            } => {
                let now = import::unix_now();
                // The status bar names the provider beside the error already.
                let prefix = format!("{}: ", source.provider.name);
                let error = error
                    .as_deref()
                    .map(|e| e.strip_prefix(&prefix).unwrap_or(e));
                source.refresh = idle_text(*updated, error, *retry_at, now);
                source.refresh_error = error.is_some();
                source.busy = false;
            }
            // It may have moved schedules with their programmes.
            Update::Epg(_) => {
                let _ = self.recorder_tx.send(recorder::Command::Changed);
            }
            Update::Stages(stages) => source.stages.clone_from(stages),
            Update::Channels(_) | Update::Movies(_) | Update::Series(_) => {}
        }
        let handed_over;
        (source.first_run, handed_over) = first_run_after(source.first_run, &update);
        if i != self.active {
            return;
        }
        let has_stages = !self.sources[i].stages.is_empty();
        self.refresh_panel(PanelEvent::Update(&update), has_stages);
        if handed_over {
            self.refresh_panel(PanelEvent::Asked, has_stages);
        }
        self.show_source_status();
        if let Some(d) = &mut self.dashboard
            && matches!(update, Update::Account(_) | Update::Idle { .. })
        {
            d.checking = false;
            self.show_dashboard();
        }
        // Settings › EPG's counts after an import, its "importing…" as the
        // stages move.
        if self.settings_view.as_ref().is_some_and(|v| v.epg.is_some()) {
            if matches!(update, Update::Epg(_) | Update::Idle { .. }) {
                self.load_epg();
            }
            if matches!(
                update,
                Update::Epg(_) | Update::Idle { .. } | Update::Stages(_)
            ) {
                self.show_settings();
            }
        }
        match update {
            Update::Channels(_) => {
                if let Some(m) = &mut self.manage {
                    m.names_read = false;
                }
                self.reindex(Kind::Live);
                if let Err(e) = self.reload() {
                    log::error!("{e}");
                    self.set_error(&e);
                }
            }
            Update::Epg(_) => {
                self.refresh_times();
                // Which channels have a guide may have changed.
                if self.guide_open {
                    self.load_guide(false);
                }
            }
            // Manage mode re-reads with the library.
            Update::Movies(_) | Update::Series(_) => {
                self.reindex(match update {
                    Update::Movies(_) => Kind::Movie,
                    _ => Kind::Series,
                });
                if let Err(e) = self.reload() {
                    log::error!("{e}");
                    self.set_error(&e);
                }
            }
            _ => {}
        }
    }

    /// The status bar's provider, account and refresh parts, for the
    /// provider shown.
    fn show_source_status(&self) {
        let w = self.window();
        let source = &self.sources[self.active];
        w.set_provider_name(source.provider.name.as_str().into());
        let (account, urgency) = source
            .info
            .as_ref()
            .map(|a| account_text(a, import::unix_now()))
            .filter(|(_, urgency)| *urgency != Urgency::None)
            .unwrap_or_default();
        w.set_account_status(account.into());
        w.set_account_urgency(urgency);
        w.set_refresh_status(source.refresh.as_str().into());
        w.set_refresh_error(source.refresh_error);
        w.set_refresh_has_panel(!source.stages.is_empty());
        w.set_refreshing(source.busy);
        w.set_first_run(source.first_run);
        self.show_stages();
    }

    /// The refresh panel's rows for the provider shown, kept counting down
    /// each second while a stage backs off.
    fn show_stages(&self) {
        let source = &self.sources[self.active];
        let (stages, name) = (source.stages.clone(), source.provider.name.clone());
        let show = move |w: &AppWindow| {
            let items = stage_items(&stages, &name, import::unix_now());
            w.set_refresh_stages(ModelRc::new(VecModel::from(items)));
        };
        show(&self.window());
        let backoff = source
            .stages
            .iter()
            .any(|(_, s)| matches!(s, StageState::Backoff { .. }));
        if !backoff {
            self.stage_timer.stop();
            return;
        }
        let weak = self.window.clone();
        self.stage_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(1),
            move || {
                if let Some(w) = weak.upgrade() {
                    show(&w);
                }
            },
        );
    }

    /// Opens or closes the refresh panel as [`refresh_panel`] says. It
    /// stays closed while the first-run screen shows the same rows.
    fn refresh_panel(&self, event: PanelEvent, has_stages: bool) {
        let w = self.window();
        if self.sources[self.active].first_run {
            w.set_refresh_panel_open(false);
            self.panel_timer.stop();
            return;
        }
        let Some((open, close_soon)) = refresh_panel(w.get_refresh_panel_open(), event, has_stages)
        else {
            return;
        };
        w.set_refresh_panel_open(open);
        if !close_soon {
            self.panel_timer.stop();
            return;
        }
        let weak = self.window.clone();
        self.panel_timer
            .start(slint::TimerMode::SingleShot, PANEL_CLOSE, move || {
                if let Some(w) = weak.upgrade() {
                    w.set_refresh_panel_open(false);
                }
            });
    }

    /// `p`: opens the picker on the provider shown, its last row "Add
    /// provider…" (onboarding ticket 01).
    fn open_picker(&mut self) {
        let items: Vec<ProviderItem> = self
            .sources
            .iter()
            .map(|s| ProviderItem {
                name: s.provider.name.as_str().into(),
                note: s.refresh.lines().last().unwrap_or("").into(),
            })
            .chain(std::iter::once(ProviderItem {
                name: "Add provider…".into(),
                note: "In Settings › Providers".into(),
            }))
            .collect();
        let w = self.window();
        w.set_providers(ModelRc::new(VecModel::from(items)));
        self.pick(self.active);
    }

    fn pick(&mut self, i: usize) {
        let i = i.min(self.sources.len());
        self.picking = Some(i);
        let w = self.window();
        w.set_picker_selected(i as i32);
        w.set_picking(true);
    }

    fn close_picker(&mut self) {
        self.picking = None;
        self.window().set_picking(false);
    }

    /// Picker row `i`: switches to that provider, or the last row opens
    /// the add-provider form.
    fn pick_provider(&mut self, i: usize) {
        if i == self.sources.len() {
            self.close_picker();
            self.open_provider_form();
        } else {
            self.switch_provider(i);
        }
    }

    /// `?`: opens or closes the keys panel (ticket 24).
    fn show_keys_panel(&mut self, open: bool) {
        self.keys_open = open;
        if open {
            self.window().invoke_keys_to_top();
        }
        self.window().set_keys_open(open);
    }

    /// `i` or a click on the status bar's account: shows the last check at
    /// once and asks for a fresh one, so active connections are current.
    fn open_dashboard(&mut self) {
        let source = &self.sources[self.active];
        let checking = source.refresh_tx.send(Request::Account).is_ok();
        self.dashboard = Some(Dashboard {
            revealed: [false; 3],
            checking,
        });
        self.show_dashboard();
        self.window().set_account_open(true);
    }

    fn close_dashboard(&mut self) {
        self.dashboard = None;
        let w = self.window();
        w.set_account_open(false);
        // Out of the window, not just hidden.
        w.set_account_server("".into());
        w.set_account_username("".into());
        w.set_account_password("".into());
    }

    /// Shows or masks login field `i`: 0 server, 1 username, 2 password.
    fn reveal(&mut self, i: usize) {
        if let Some(d) = &mut self.dashboard
            && i < 3
        {
            d.revealed[i] = !d.revealed[i];
            self.show_dashboard();
        }
    }

    fn show_dashboard(&self) {
        let Some(d) = &self.dashboard else { return };
        let source = &self.sources[self.active];
        let w = self.window();
        w.set_account(account_view(
            &source.provider.name,
            source.info.as_ref(),
            d.checking,
            import::unix_now(),
        ));
        let p = &source.provider;
        let shown = |i: usize, v: &str| -> slint::SharedString {
            if d.revealed[i] { v.into() } else { "".into() }
        };
        w.set_account_server(shown(0, &p.url));
        w.set_account_username(shown(1, &p.username));
        w.set_account_password(shown(2, &p.password));
    }

    /// A key while the dashboard is open: `1`–`3` show or mask a login
    /// field, Esc, `i` or `q` close.
    fn dashboard_key(&mut self, text: &str, ctrl: bool) -> bool {
        match text {
            "1" | "2" | "3" if !ctrl => self.reveal(text.parse::<usize>().unwrap() - 1),
            _ => {
                if let Some(Action::ClearFilter | Action::Account | Action::Quit) =
                    action(text, ctrl)
                {
                    self.close_dashboard();
                }
            }
        }
        // Modal, like the picker.
        true
    }

    /// `,` or the status bar's gear: opens the Settings view (settings
    /// ticket 01) on its first section, Providers.
    fn open_settings(&mut self) {
        self.settings_view = Some(SettingsView {
            managed: config::managed(&self.settings_path),
            providers_managed: config::managed(&self.credentials_path),
            ..Default::default()
        });
        self.show_settings();
        self.window().set_settings_open(true);
    }

    /// Opens Settings › Providers on the add-provider form, or on the
    /// providers with the note saying why when the file is managed.
    fn open_provider_form(&mut self) {
        if self.settings_view.is_none() {
            self.open_settings();
        }
        let providers = SETTINGS_SECTIONS.iter().position(|&s| s == "Providers");
        self.settings_section(providers.unwrap_or(0));
        if let Some(view) = &mut self.settings_view {
            view.in_fields = true;
            if view.providers_managed.is_none() {
                view.form = Some(ProviderForm::default());
                view.field = 0;
            } else {
                view.field = self.sources.len();
            }
        }
        self.show_settings();
    }

    fn close_settings(&mut self) {
        // The setup screen stays until there is a provider to show.
        if self.setup.is_some() {
            return;
        }
        self.settings_view = None;
        self.window().set_settings_open(false);
    }

    /// The selected section's fields.
    fn settings_items(&self) -> Vec<SettingItem> {
        let Some(view) = &self.settings_view else {
            return Vec::new();
        };
        if let Some(setup) = &self.setup
            && view.form.is_none()
        {
            return setup_items(setup);
        }
        if SETTINGS_SECTIONS[view.section] == "Providers" {
            let now = import::unix_now();
            let providers: Vec<(&str, String)> = self
                .sources
                .iter()
                .map(|s| {
                    let note = match &s.info {
                        Some(a) => {
                            let text = account_text(a, now).0;
                            match a.user.max_connections {
                                Some(1) => format!("{} · {text} · 1 connection", s.provider.id),
                                Some(n) => {
                                    format!("{} · {text} · {n} connections", s.provider.id)
                                }
                                None => format!("{} · {text}", s.provider.id),
                            }
                        }
                        None => s.provider.id.clone(),
                    };
                    (s.provider.name.as_str(), note)
                })
                .collect();
            return provider_items(&providers, view.form.as_ref());
        }
        if view.epg.is_some() {
            return self.epg_items();
        }
        let providers: Vec<(&str, &str)> = self
            .sources
            .iter()
            .map(|s| (s.provider.id.as_str(), s.provider.name.as_str()))
            .collect();
        setting_items(view.section, &self.settings, &providers)
    }

    fn show_settings(&self) {
        let Some(view) = &self.settings_view else {
            return;
        };
        let mut items = self.settings_items();
        if let Some(e) = &view.error
            && let Some(item) = items.get_mut(view.field)
        {
            item.error = e.into();
            item.button = if view.create.is_some() {
                "Create it"
            } else {
                ""
            }
            .into();
        }
        let stepper = view.in_fields
            && items
                .get(view.field)
                .is_some_and(|i| i.kind == SettingKind::Stepper);
        // The password field, typed into or selected.
        let password = view.form.is_some() && view.field == 3;
        let hint = if let Some(hint) = self.epg_hint() {
            hint
        } else if view.create.is_some() {
            "Enter creates it · Esc cancels".into()
        } else if view.editing.is_some() && password {
            "Type · Ctrl v pastes · Ctrl r shows · Enter keeps · Ctrl u clears · Esc cancels".into()
        } else if view.editing.is_some() && view.form.is_some() {
            "Type · Ctrl v pastes · Enter keeps · Ctrl u clears · Esc cancels".into()
        } else if view.editing.is_some() {
            "Type · Ctrl v pastes · Enter saves · Ctrl u clears · Esc cancels".into()
        } else if view.choosing.is_some() {
            "j / k choose · Enter saves · Esc cancels".into()
        } else if view.form.as_ref().is_some_and(|f| f.remove.is_some()) {
            let enter = if view.field == 0 {
                "Enter switches"
            } else {
                "Enter removes"
            };
            format!("j / k move · {enter} · Esc back to the form")
        } else if let Some(form) = &view.form {
            let enter = match form.check {
                FormCheck::Warned(..) => "Enter saves anyway",
                _ if view.field == FORM_FIELDS.len() && form.id.is_some() => "Enter saves",
                _ if view.field == FORM_FIELDS.len() => "Enter checks and saves",
                _ if view.field > FORM_FIELDS.len() => "Enter asks first",
                _ => "Enter edits",
            };
            let show = if password { " · Ctrl r shows" } else { "" };
            let leave = if self.setup.is_some() {
                "q quits"
            } else {
                "Esc leaves the form"
            };
            format!("j / k move · {enter}{show} · {leave}")
        } else if self.setup.is_some() {
            "j / k move · Enter chooses · q quits".into()
        } else {
            let tab = if view.in_fields { "sections" } else { "fields" };
            let enter = match (view.locked(), view.in_fields, stepper) {
                (Some(_), _, _) => "",
                (None, true, true) => " · h / l change",
                (None, true, false) => " · Enter edits",
                (None, false, _) => " · Enter opens",
            };
            format!("Tab {tab}{enter} · Esc to close")
        };
        let mut text = view.editing.clone().unwrap_or_default();
        if password && view.form.as_ref().is_some_and(|f| !f.revealed) {
            text = "•".repeat(text.chars().count());
        }
        // A source's URL, typed into, masked as it shows (SPEC 4.1).
        if let Some(epg) = &view.epg
            && let Some(form) = &epg.form
            && form.fields().get(view.field) == Some(&SourceField::Text(0))
        {
            text = shown_url(&text, epg.revealed);
        }
        let w = self.window();
        w.set_settings_hint(hint.into());
        w.set_settings_editing(view.editing.is_some());
        w.set_settings_text(text.into());
        w.set_settings_section(view.section as i32);
        w.set_settings_in_fields(view.in_fields);
        w.set_settings_field(view.field as i32);
        w.set_settings_choosing(view.choosing.map_or(-1, |c| c as i32));
        w.set_settings_note(view.locked().cloned().unwrap_or_default().into());
        // Row by row, so the rows stay and the selected one scrolls into
        // view as keys move it (app.slint).
        if items.len() == self.settings_rows.row_count() {
            for (i, item) in items.into_iter().enumerate() {
                if self.settings_rows.row_data(i).as_ref() != Some(&item) {
                    self.settings_rows.set_row_data(i, item);
                }
            }
        } else {
            self.settings_rows.set_vec(items);
        }
    }

    /// Selects section `i`, keys on the sections.
    fn settings_section(&mut self, i: usize) {
        if let Some(view) = &mut self.settings_view {
            *view = SettingsView {
                section: i.min(SETTINGS_SECTIONS.len() - 1),
                managed: view.managed.take(),
                providers_managed: view.providers_managed.take(),
                ..Default::default()
            };
            if SETTINGS_SECTIONS[view.section] == "EPG" {
                self.load_epg();
            }
            self.show_settings();
        }
    }

    /// Selects field `i`, keys on the fields.
    fn settings_field(&mut self, i: usize) {
        if let Some(view) = &mut self.settings_view {
            if view.field != i {
                view.error = None;
                view.editing = None;
                view.create = None;
            }
            view.in_fields = true;
            view.field = i;
            view.choosing = None;
        }
    }

    /// Enter on the selected field: a toggle flips and saves, a picker
    /// opens, a text field takes typing. Nothing on a managed file, which
    /// says why.
    fn settings_activate(&mut self) {
        let items = self.settings_items();
        let Some(view) = &mut self.settings_view else {
            return;
        };
        if view.epg.is_some() && view.locked().is_none() && view.editing.is_none() {
            return self.epg_activate();
        }
        if let Some(item) = items.get(view.field)
            && view.locked().is_none()
            && view.editing.is_none()
        {
            match item.kind {
                SettingKind::Choice => view.choosing = Some(item.chosen as usize),
                SettingKind::Toggle => {
                    let flipped = 1 - item.chosen as usize;
                    return self.settings_choose(flipped);
                }
                SettingKind::Text => {
                    view.editing = Some(match &view.form {
                        Some(form) => form.values[view.field].clone(),
                        None => {
                            setting_text(&self.settings, SETTING_FIELDS[view.section][view.field])
                        }
                    });
                }
                SettingKind::Stepper => {}
                SettingKind::Row => match &view.form {
                    Some(form) if form.remove.is_some() => return self.remove_provider(),
                    Some(_) if view.field > FORM_FIELDS.len() => return self.ask_remove(),
                    Some(form) => match form.check {
                        FormCheck::None => return self.check_provider(),
                        FormCheck::Warned(..) => return self.save_anyway(),
                        FormCheck::Checking(_) => {}
                    },
                    None if self.setup.is_some() => {
                        let i = view.field;
                        return self.setup_row(i);
                    }
                    None if view.field == self.sources.len() => return self.open_provider_form(),
                    None => {
                        let i = view.field;
                        return self.edit_provider_form(i);
                    }
                },
            }
        }
        self.show_settings();
    }

    /// A stepper field `field` moved by `d` minutes, within 0 to 60, and
    /// saved.
    fn settings_step(&mut self, field: usize, d: i32) {
        let items = self.settings_items();
        let Some(view) = &mut self.settings_view else {
            return;
        };
        if let Some(item) = items.get(field)
            && item.kind == SettingKind::Stepper
            && view.locked().is_none()
        {
            let minutes = (item.chosen + d).clamp(0, 60);
            if minutes != item.chosen {
                let key = SETTING_FIELDS[view.section][field];
                self.save_setting(key, Some(i64::from(minutes).into()));
            }
        }
        self.show_settings();
    }

    /// Enter in a text field: the typed value checked and saved, closing
    /// the edit, or refused with why under the field, the edit kept. A
    /// directory offered to be created is made first.
    fn settings_confirm(&mut self) {
        let Some(view) = &mut self.settings_view else {
            return;
        };
        let Some(text) = view.editing.clone() else {
            return;
        };
        if view.epg.is_some() {
            return self.epg_confirm(text);
        }
        if let Some(form) = &mut view.form {
            let field = view.field;
            let value = if field == 1 {
                match config::origin(&text) {
                    Ok((origin, cut)) => {
                        form.cut = cut;
                        origin
                    }
                    Err(e) => {
                        view.error = Some(e);
                        return self.show_settings();
                    }
                }
            } else if field == 0 {
                text.trim().to_owned()
            } else {
                text
            };
            if form.values[field] != value {
                form.values[field] = value;
                form.check = FormCheck::None;
            }
            view.editing = None;
            view.error = None;
            return self.show_settings();
        }
        if let Some(dir) = view.create.take()
            && let Err(e) = std::fs::create_dir_all(&dir)
        {
            view.error = Some(format!("Cannot create {}: {e}", dir.display()));
            return self.show_settings();
        }
        let key = SETTING_FIELDS[view.section][view.field];
        match config::save_typed(&self.settings_path, key, &text) {
            Ok(settings) => {
                view.editing = None;
                view.error = None;
                self.apply_settings(settings);
            }
            Err(config::Refused::Missing(dir)) => {
                view.error = Some(format!("{}: does not exist", dir.display()));
                view.create = Some(dir);
            }
            Err(config::Refused::Reason(e)) => view.error = Some(e),
        }
        self.show_settings();
    }

    /// A key while a Settings text field is being typed into: as in
    /// search, typing edits and only Enter and Esc do anything else.
    fn settings_edit_key(&mut self, text: &str, ctrl: bool) {
        let Some(view) = &mut self.settings_view else {
            return;
        };
        let Some(editing) = &mut view.editing else {
            return;
        };
        // Some backends send the control character instead of the letter.
        if ctrl && matches!(text, "r" | "R" | "\u{12}") {
            return self.settings_reveal();
        }
        if ctrl && matches!(text, "u" | "U" | "\u{15}") {
            editing.clear();
            view.create = None;
            return self.show_settings();
        }
        if ctrl && matches!(text, "v" | "V" | "\u{16}") {
            match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                // One line: a copied path often ends in a newline.
                Ok(pasted) => editing.push_str(&pasted.trim_end().replace(['\n', '\r'], " ")),
                Err(e) => view.error = Some(format!("Cannot paste: {e}")),
            }
            view.create = None;
            return self.show_settings();
        }
        match search_key(text, ctrl) {
            Some(SearchKey::Type(t)) => {
                editing.push_str(&t);
                view.create = None;
            }
            Some(SearchKey::Backspace) => {
                editing.pop();
                view.create = None;
            }
            Some(SearchKey::Keep) => return self.settings_confirm(),
            Some(SearchKey::Clear) => {
                view.editing = None;
                view.error = None;
                view.create = None;
            }
            #[cfg(feature = "devtools")]
            Some(SearchKey::Nav(Action::Snapshot)) => {
                return crate::devtools::snapshot(&self.window());
            }
            _ => return,
        }
        self.show_settings();
    }

    /// Option `option` of the selected field, chosen: saved if it changed
    /// anything.
    fn settings_choose(&mut self, option: usize) {
        let Some(view) = &mut self.settings_view else {
            return;
        };
        view.choosing = None;
        if let Some(form) = view.epg.as_mut().and_then(|e| e.form.as_mut())
            && form.remove
        {
            form.forget = option == 1;
            return self.show_settings();
        }
        if view.epg.as_ref().is_some_and(|e| e.form.is_some()) {
            if view.locked().is_none() {
                return self.epg_switch(option == 0);
            }
            return self.show_settings();
        }
        if let Some(r) = view.form.as_mut().and_then(|f| f.remove.as_mut()) {
            r.personal = option == 1;
            return self.show_settings();
        }
        if view.locked().is_none() && SETTINGS_SECTIONS[view.section] == "General" {
            match view.field {
                0 => {
                    let id = option
                        .checked_sub(1)
                        .and_then(|i| self.sources.get(i))
                        .map(|s| s.provider.id.clone());
                    if id != self.settings.active_provider {
                        self.save_setting("active_provider", id.map(Into::into));
                    }
                }
                1 => {
                    let hours = if option == 0 { 12 } else { 6 };
                    if hours != self.settings.refresh_hours {
                        self.save_setting("refresh_hours", Some(i64::from(hours).into()));
                    }
                }
                _ => {}
            }
        }
        self.show_settings();
    }

    /// Saves `key` to `config.toml` and applies it. A refusal shows under
    /// the field.
    fn save_setting(&mut self, key: &str, value: Option<toml_edit::Value>) {
        let saved = config::save_setting(&self.settings_path, key, value);
        let Some(view) = &mut self.settings_view else {
            return;
        };
        match saved {
            Ok(settings) => {
                view.error = None;
                self.apply_settings(settings);
            }
            Err(e) => {
                log::error!("{e}");
                view.error = Some(e);
            }
        }
    }

    /// Ctrl r or `Show`: shows or masks the form's password, or Settings ›
    /// EPG's URLs.
    fn settings_reveal(&mut self) {
        let Some(view) = &mut self.settings_view else {
            return;
        };
        if let Some(form) = &mut view.form {
            form.revealed = !form.revealed;
        } else if let Some(epg) = &mut view.epg {
            // Settings › EPG's URLs (EPG sources ticket 04).
            epg.revealed = !epg.revealed;
        } else {
            return;
        }
        self.show_settings();
    }

    /// The button under a field: `Save anyway` on the form's warning, else
    /// `Create it` on a missing folder.
    fn settings_button(&mut self) {
        let warned = self
            .settings_view
            .as_ref()
            .and_then(|v| v.form.as_ref())
            .is_some_and(|f| matches!(f.check, FormCheck::Warned(..)));
        if warned {
            self.save_anyway();
        } else {
            self.settings_confirm();
        }
    }

    /// Reads Settings › EPG's counts for the provider shown on a worker,
    /// a few hundred ms on a large provider; [`Self::epg_counted`] takes
    /// them, keeping what the section has open.
    fn load_epg(&mut self) {
        let Some(view) = &mut self.settings_view else {
            return;
        };
        view.epg.get_or_insert_with(EpgView::default);
        self.epg_reads += 1;
        let (n, tx, db_path) = (self.epg_reads, self.epg_tx.clone(), self.db_path.clone());
        let provider_id = self.sources[self.active].provider.id.clone();
        self.starter.rt.spawn_blocking(move || {
            let t = Instant::now();
            let counts = store::open_reader(&db_path)
                .and_then(|c| store::source_stats(&c, &provider_id, import::unix_now()))
                .unwrap_or_else(|e| {
                    log::error!("reading guide sources: {e}");
                    Default::default()
                });
            log::debug!("guide source counts in {} ms", t.elapsed().as_millis());
            let _ = tx.send((n, provider_id, counts));
        });
    }

    /// Settings › EPG's counts read by [`Self::load_epg`]. An answer for
    /// another provider, a closed section or an older read is dropped.
    fn epg_counted(&mut self, n: u64, provider_id: &str, (channels, stats): EpgCounts) {
        if n != self.epg_reads || provider_id != self.sources[self.active].provider.id {
            return;
        }
        let Some(view) = &mut self.settings_view else {
            return;
        };
        let Some(epg) = &mut view.epg else {
            return;
        };
        (epg.channels, epg.stats, epg.counted) = (channels, stats, true);
        if let Some(form) = &mut epg.form
            && let Some(source) = form.index.and_then(|i| {
                config::epg_order(&self.sources[self.active].provider)
                    .get(i)
                    .cloned()
            })
        {
            form.counts(epg.stats.get(&source.ident()));
            view.field = view.field.min(form.fields().len() - 1);
        }
        self.show_settings();
    }

    /// Settings › EPG's rows: the source form, or the sources.
    fn epg_items(&self) -> Vec<SettingItem> {
        let Some(epg) = self.settings_view.as_ref().and_then(|v| v.epg.as_ref()) else {
            return Vec::new();
        };
        let source = &self.sources[self.active];
        let order = config::epg_order(&source.provider);
        let labels = config::epg_labels(&source.provider);
        if let Some(form) = &epg.form {
            let saved = form.index.and_then(|i| order.get(i));
            let label = form
                .index
                .and_then(|i| labels.get(i))
                .cloned()
                .unwrap_or_default();
            let stats = saved
                .and_then(|s| epg.stats.get(&s.ident()))
                .cloned()
                .unwrap_or_default();
            return source_form_items(form, &label, epg.revealed, &stats);
        }
        let busy: Vec<&str> = source
            .stages
            .iter()
            .filter(|(_, s)| matches!(s, StageState::Waiting | StageState::Running { .. }))
            .filter_map(|(stage, _)| match stage {
                Stage::Source { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        source_items(
            &order,
            &labels,
            &source.provider.name,
            epg,
            &busy,
            import::unix_now(),
        )
    }

    /// The offer's move, while shown: the new source's place, and the
    /// provider's guide's above it.
    fn epg_offer(&self) -> Option<(usize, usize)> {
        let offer = self.settings_view.as_ref()?.epg.as_ref()?.offer?;
        let order = config::epg_order(&self.sources[self.active].provider);
        let guide = order.iter().position(|s| s.provider_guide)?;
        (guide < offer && offer < order.len()).then_some((offer, guide))
    }

    /// Settings › EPG's hint line while keys are on its rows.
    fn epg_hint(&self) -> Option<String> {
        let view = self.settings_view.as_ref()?;
        let epg = view.epg.as_ref()?;
        if !view.in_fields || view.editing.is_some() || view.locked().is_some() {
            return None;
        }
        let Some(form) = &epg.form else {
            let n = config::epg_order(&self.sources[self.active].provider).len();
            let enter = match view.field {
                f if f < n => "J / K reorder · Enter opens",
                f if f + 1 == self.epg_items().len() => "Enter adds one",
                _ => "Enter moves it",
            };
            return Some(format!(
                "j / k move · {enter} · Ctrl r shows URLs · Tab sections · Esc to close"
            ));
        };
        if form.remove {
            let enter = match form.fields().get(view.field) {
                Some(SourceField::Forget) => "j / k move · Enter switches",
                _ if form.mappings > 0 => "j / k move · Enter removes",
                _ => "Enter removes",
            };
            return Some(format!("{enter} · Esc back to the form"));
        }
        let enter = match form.fields().get(view.field) {
            Some(SourceField::Enabled) => "Enter switches",
            Some(SourceField::Ambiguous) => "Enter lists them",
            Some(SourceField::Text(_)) => "Enter edits",
            Some(SourceField::Save) if form.index.is_none() => "Enter adds and imports",
            Some(SourceField::Save) => "Enter saves",
            _ => "Enter asks first",
        };
        let show = if form.guide {
            ""
        } else {
            " · Ctrl r shows the URL"
        };
        Some(format!("j / k move · {enter}{show} · Esc leaves the form"))
    }

    /// Enter on a row of Settings › EPG: a source opens its form, the
    /// offer moves the new source above the provider's guide, "Add
    /// source…" opens an empty form; in the form, the switch flips, a text
    /// field takes typing, save saves, remove asks and then removes.
    fn epg_activate(&mut self) {
        let order = config::epg_order(&self.sources[self.active].provider);
        let offer = self.epg_offer();
        let Some(view) = &mut self.settings_view else {
            return;
        };
        let field = view.field;
        let Some(epg) = &mut view.epg else {
            return;
        };
        let Some(form) = &mut epg.form else {
            if let Some(source) = order.get(field) {
                let mut form = SourceForm::edit(field, source);
                form.counts(epg.stats.get(&source.ident()));
                epg.form = Some(form);
            } else if let Some((from, to)) = offer.filter(|_| field == order.len()) {
                return self.epg_move(from, to);
            } else {
                epg.form = Some(SourceForm {
                    enabled: true,
                    ..Default::default()
                });
            }
            view.field = 0;
            view.error = None;
            return self.show_settings();
        };
        match form.fields().get(field) {
            Some(SourceField::Enabled) => {
                let on = !form.enabled;
                return self.epg_switch(on);
            }
            Some(&SourceField::Text(i)) => view.editing = Some(form.values[i].clone()),
            Some(SourceField::Save) => return self.epg_save(),
            Some(SourceField::Ambiguous) => return self.epg_ambiguous(),
            Some(SourceField::Forget) => form.forget = !form.forget,
            Some(SourceField::Remove) if form.remove => return self.epg_remove(),
            Some(SourceField::Remove) => {
                form.remove = true;
                form.forget = false;
                // On its Remove row, as the provider's confirm.
                view.field = form.fields().len() - 1;
                view.error = None;
            }
            None => {}
        }
        self.show_settings();
    }

    /// The source form's switch set: on the provider's guide, turning it
    /// off counts the favourites it leaves with no guide, said under it.
    fn epg_switch(&mut self, on: bool) {
        let provider_id = self.sources[self.active].provider.id.clone();
        let Some(form) = self
            .settings_view
            .as_mut()
            .and_then(|v| v.epg.as_mut())
            .and_then(|e| e.form.as_mut())
        else {
            return;
        };
        form.enabled = on;
        if form.guide && !on && form.orphans.is_none() {
            form.orphans = store::only_provider_guide(&self.conn, &provider_id)
                .inspect_err(|e| log::error!("counting favourites: {e}"))
                .ok();
        }
        self.show_settings();
    }

    /// Enter in a source form's text field: the value kept, the URL or
    /// path and the numbers checked first, a refusal under the field.
    fn epg_confirm(&mut self, text: String) {
        let Some(view) = &mut self.settings_view else {
            return;
        };
        let Some(form) = view.epg.as_mut().and_then(|e| e.form.as_mut()) else {
            return;
        };
        let Some(&SourceField::Text(i)) = form.fields().get(view.field) else {
            return;
        };
        let checked = match i {
            0 => config::check_source(&text).map(|_| ()),
            2 => config::check_number(&text, i64::MIN, "Shift").map(|_| ()),
            3 => config::check_number(&text, 1, "Fetch every").map(|_| ()),
            4 => config::check_number(&text, 3, "At most per day").map(|_| ()),
            _ => Ok(()),
        };
        match checked {
            Ok(()) => {
                form.values[i] = text.trim().to_owned();
                view.editing = None;
                view.error = None;
            }
            Err(e) => view.error = Some(e),
        }
        self.show_settings();
    }

    /// The source form's save row: its values checked, then added last in
    /// the order, imported by the next pass at once, or saved in place.
    fn epg_save(&mut self) {
        let provider = self.sources[self.active].provider.clone();
        let Some(view) = &mut self.settings_view else {
            return;
        };
        let Some(form) = view.epg.as_ref().and_then(|e| e.form.as_ref()) else {
            return;
        };
        let source = match form.source() {
            Ok(s) => s,
            Err((i, e)) => {
                view.field = form
                    .fields()
                    .iter()
                    .position(|&f| f == SourceField::Text(i))
                    .unwrap_or(0);
                view.error = Some(e);
                return self.show_settings();
            }
        };
        let order = config::epg_order(&provider);
        let index = form.index;
        let twin = order
            .iter()
            .position(|s| !s.provider_guide && s.ident() == source.ident())
            .filter(|&i| Some(i) != index);
        if let Some(i) = twin {
            let labels = config::epg_labels(&provider);
            view.field = 0;
            view.error = Some(format!("Already a source: {}", labels[i]));
            return self.show_settings();
        }
        let path = self.credentials_path.clone();
        let saved = match index {
            Some(i) => config::edit_epg_source(&path, &provider.id, i, &source),
            None => config::add_epg_source(&path, &provider.id, &source),
        };
        if !self.epg_apply(saved) {
            return;
        }
        let labels = config::epg_labels(&self.sources[self.active].provider);
        let label = labels[index.unwrap_or(order.len())].clone();
        let Some(view) = &mut self.settings_view else {
            return;
        };
        let Some(epg) = &mut view.epg else {
            return;
        };
        epg.form = None;
        view.error = None;
        match index {
            Some(i) => {
                view.field = i;
                self.show_settings();
                self.set_status(&format!("Saved {label}"));
            }
            None => {
                view.field = order.len();
                epg.offer = Some(order.len());
                self.show_settings();
                self.set_status(&format!("Added {label}: importing it"));
            }
        }
    }

    /// The confirm's Remove row: the source leaves `credentials.toml`, its
    /// data going in the next pass's one transaction (SPEC 4.1), its
    /// manual mappings with the switch on.
    fn epg_remove(&mut self) {
        let provider_id = self.sources[self.active].provider.id.clone();
        let Some((index, forget)) = self
            .settings_view
            .as_ref()
            .and_then(|v| v.epg.as_ref())
            .and_then(|e| e.form.as_ref())
            .and_then(|f| Some((f.index?, f.forget)))
        else {
            return;
        };
        let provider = &self.sources[self.active].provider;
        let label = config::epg_labels(provider)
            .get(index)
            .cloned()
            .unwrap_or_default();
        let ident = config::epg_order(provider).get(index).map(EpgSource::ident);
        let saved = config::remove_epg_source(&self.credentials_path, &provider_id, index);
        if !self.epg_apply(saved) {
            return;
        }
        if let Some(ident) = ident.filter(|_| forget) {
            self.write_guide(GuideEdit::Forget(ident));
        }
        if let Some(view) = &mut self.settings_view
            && let Some(epg) = &mut view.epg
        {
            epg.form = None;
            epg.offer = None;
            view.field = index;
            view.error = None;
        }
        self.show_settings();
        self.set_status(&format!("Removed {label}"));
    }

    /// `J`/`K`, a drag or the offer: the source at `from` moved to `to` in
    /// the order, the selection with it.
    fn epg_move(&mut self, from: usize, to: usize) {
        let provider = &self.sources[self.active].provider;
        let n = config::epg_order(provider).len();
        if from == to || from >= n || to >= n {
            return;
        }
        let saved = config::move_epg_source(&self.credentials_path, &provider.id.clone(), from, to);
        if !self.epg_apply(saved) {
            return;
        }
        if let Some(view) = &mut self.settings_view
            && let Some(epg) = &mut view.epg
        {
            epg.offer = None;
            view.field = to;
            view.error = None;
        }
        self.show_settings();
    }

    /// A source change written: the provider shown and its client take
    /// the saved sources, and its refresher stores them at once, importing
    /// a new one. A refusal shows under the selected row.
    fn epg_apply(&mut self, saved: Result<Vec<Provider>, String>) -> bool {
        let providers = match saved {
            Ok(p) => p,
            Err(e) => {
                log::error!("{e}");
                if let Some(view) = &mut self.settings_view {
                    view.error = Some(e);
                }
                self.show_settings();
                return false;
            }
        };
        let source = &mut self.sources[self.active];
        if let Some(p) = providers.into_iter().find(|p| p.id == source.provider.id) {
            source.client.set_provider(p.clone());
            source.provider = p;
        }
        let _ = source.refresh_tx.send(Request::Sources);
        self.load_epg();
        true
    }

    /// A key on Settings › EPG's rows that the other sections do not
    /// have: `J`/`K` reorder, Esc leaves the form or its confirm. Returns
    /// whether it was one.
    fn epg_key(&mut self, action: Option<Action>) -> bool {
        let n = config::epg_order(&self.sources[self.active].provider).len();
        let offer = self.epg_offer().is_some();
        let Some(view) = &mut self.settings_view else {
            return false;
        };
        if !view.in_fields || view.choosing.is_some() || view.locked().is_some() {
            return false;
        }
        let Some(epg) = &mut view.epg else {
            return false;
        };
        match (action, &mut epg.form) {
            (Some(Action::MoveFavourite(d)), None) if view.field < n => {
                let from = view.field;
                let to = (from as isize + d).clamp(0, n as isize - 1) as usize;
                self.epg_move(from, to);
            }
            (Some(Action::ClearFilter), Some(form)) if form.remove => {
                form.remove = false;
                view.field = form.fields().len() - 1;
                view.error = None;
                self.show_settings();
            }
            (Some(Action::ClearFilter), Some(form)) => {
                // On the source, or "Add source…".
                view.field = form.index.unwrap_or(n + offer as usize);
                epg.form = None;
                view.error = None;
                self.show_settings();
            }
            _ => return false,
        }
        true
    }

    /// A drag of Settings › EPG's row `i` by `rows` rows: a source moved
    /// to where it was dropped.
    fn epg_drag(&mut self, i: usize, rows: isize) {
        let in_list = self
            .settings_view
            .as_ref()
            .and_then(|v| v.epg.as_ref())
            .is_some_and(|e| e.form.is_none());
        let locked = self
            .settings_view
            .as_ref()
            .is_some_and(|v| v.locked().is_some());
        let n = config::epg_order(&self.sources[self.active].provider).len();
        if in_list && !locked && i < n {
            let to = (i as isize + rows).clamp(0, n as isize - 1) as usize;
            self.epg_move(i, to);
        }
    }

    /// `E` (EPG sources ticket 05): the selected live channel's guide
    /// sources, in the list or the grid.
    fn open_guide_card(&mut self) {
        let row = self
            .selected_key()
            .filter(|_| self.guide_open || self.listing() == Kind::Live)
            .and_then(|key| read_channel(&self.conn, key, import::unix_now()))
            .filter(|r| r.kind == Kind::Live && r.available);
        let Some(row) = row else {
            return self.set_status("E sets where a live channel's guide comes from");
        };
        self.card.sources = Some(SourcesCard {
            stream_id: row.stream_id,
            name: row.name,
            sources: Vec::new(),
            selected: 0,
        });
        self.load_card();
        self.window().invoke_card_to_top();
        self.show_card();
    }

    /// Reads again what the open card layers show.
    fn load_card(&mut self) {
        let provider_id = self.provider().id.clone();
        if let Some(c) = &mut self.card.sources {
            c.sources = store::channel_sources(&self.conn, &provider_id, c.stream_id)
                .unwrap_or_else(|e| {
                    log::error!("reading guide sources: {e}");
                    Vec::new()
                });
            c.selected = c.selected.min(c.sources.len().saturating_sub(1));
        }
        if let Some(a) = &mut self.card.ambiguous {
            a.all =
                store::ambiguous_channels(&self.conn, &provider_id, a.source).unwrap_or_else(|e| {
                    log::error!("reading guide sources: {e}");
                    Vec::new()
                });
            a.filter();
        }
    }

    /// The card's top layer, drawn.
    fn show_card(&mut self) {
        let w = self.window();
        let provider = self.provider();
        let (title, hint, query, empty, rows, selected) = if let Some(p) = &self.card.picker {
            let empty = if p.query.trim().is_empty() {
                format!("No channels from {} yet: it has not been imported", p.label)
            } else {
                format!("No channel of {} holds “{}”", p.label, p.query.trim())
            };
            (
                format!("Map {} in {}", p.name, p.label),
                "Type to search ids and names · ↑ / ↓ move · Enter maps · Esc back".to_owned(),
                Some(p.query.clone()),
                empty,
                picker_rows(p),
                p.selected,
            )
        } else if let Some(a) = &self.card.ambiguous {
            let rows = a
                .shown
                .iter()
                .map(|&i| CardRow {
                    label: a.all[i].1.clone().into(),
                    ..Default::default()
                })
                .collect();
            let empty = if a.all.is_empty() {
                "None left: each matches one of its channels, or none"
            } else {
                "None holds that"
            };
            (
                format!("Ambiguous in {}", a.label),
                "Several of its channels match each · type to filter · ↑ / ↓ move · Enter picks one · Esc back"
                    .to_owned(),
                Some(a.query.clone()),
                empty.to_owned(),
                rows,
                a.selected,
            )
        } else if let Some(c) = &self.card.sources {
            let rows = c
                .sources
                .iter()
                .map(|s| source_card_row(s, &guide_label(provider, &s.ident)))
                .collect();
            (
                format!("Guide for {}", c.name),
                "j / k move · Enter maps to a channel · p pins or unpins · h / l shift the pin 5 min · Esc closes"
                    .to_owned(),
                None,
                "No guide sources stored yet: the next refresh stores them".to_owned(),
                rows,
                c.selected,
            )
        } else {
            w.set_card_open(false);
            return;
        };
        w.set_card_title(title.into());
        w.set_card_hint(hint.into());
        w.set_card_query_shown(query.is_some());
        w.set_card_query(query.unwrap_or_default().into());
        w.set_card_empty(empty.into());
        w.set_card_rows(ModelRc::new(VecModel::from(rows)));
        w.set_card_selected(selected as i32);
        w.set_card_open(true);
        w.invoke_show_card_selected();
    }

    /// The top layer's selection moved to `to`, kept in range.
    fn card_select(&mut self, to: usize) {
        let (selected, len) = if let Some(p) = &mut self.card.picker {
            let len = p.len();
            (&mut p.selected, len)
        } else if let Some(a) = &mut self.card.ambiguous {
            (&mut a.selected, a.shown.len())
        } else if let Some(c) = &mut self.card.sources {
            (&mut c.selected, c.sources.len())
        } else {
            return;
        };
        *selected = to.min(len.saturating_sub(1));
        let at = *selected as i32;
        let w = self.window();
        w.set_card_selected(at);
        w.invoke_show_card_selected();
    }

    /// The top layer's selected row.
    fn card_selected(&self) -> usize {
        if let Some(p) = &self.card.picker {
            p.selected
        } else if let Some(a) = &self.card.ambiguous {
            a.selected
        } else {
            self.card.sources.as_ref().map_or(0, |c| c.selected)
        }
    }

    /// Esc, or a click outside: the top layer closes. The ambiguous list
    /// closing goes back to Settings › EPG, its counts read again.
    fn close_card(&mut self) {
        if self.card.picker.take().is_some() {
            // Back to the layer below.
        } else if self.card.ambiguous.take().is_some() {
            self.load_epg();
            self.show_settings();
        } else {
            self.card.sources = None;
        }
        self.show_card();
    }

    /// Enter on the card's top layer: a source opens the picker on it, a
    /// channel of the ambiguous list too, a picker row maps.
    fn card_activate(&mut self) {
        if self.card.picker.is_some() {
            return self.pick_channel();
        }
        let provider = self.provider().clone();
        let target = if let Some(a) = &self.card.ambiguous {
            a.shown.get(a.selected).map(|&i| {
                (
                    a.source,
                    a.label.clone(),
                    a.all[i].0,
                    a.all[i].1.clone(),
                    None,
                )
            })
        } else if let Some(c) = &self.card.sources {
            let Some(s) = c.sources.get(c.selected) else {
                return;
            };
            if s.ident.is_empty() {
                return self.set_status(&format!(
                    "{}'s guide matches by the channel's guide id only",
                    provider.name
                ));
            }
            Some((
                s.id,
                guide_label(&provider, &s.ident),
                c.stream_id,
                c.name.clone(),
                s.mapped.clone(),
            ))
        } else {
            None
        };
        let Some((source, label, stream_id, name, mapped)) = target else {
            return;
        };
        let query = epg_sources::epg_norm(&name);
        self.card.picker = Some(Picker {
            source,
            label,
            stream_id,
            name,
            mapped,
            query,
            found: Vec::new(),
            selected: 0,
        });
        self.picker_search();
    }

    /// The picker's query changed: searched again, back at the top.
    fn picker_search(&mut self) {
        let Some(p) = &mut self.card.picker else {
            return;
        };
        p.found = store::find_source_channels(&self.conn, p.source, &p.query, PICKER_ROWS)
            .unwrap_or_else(|e| {
                log::error!("searching a guide source: {e}");
                Vec::new()
            });
        // On the mapped channel when it is listed, else the first found.
        p.selected = p
            .found
            .iter()
            .position(|c| p.mapped.as_ref() == Some(&c.id))
            .map_or(p.mapped.is_some() as usize, |i| i + 1)
            .min(p.len().saturating_sub(1));
        self.window().invoke_card_to_top();
        self.show_card();
    }

    /// Enter on a picker row: the channel mapped to it, or with
    /// "Automatic" its mapping forgotten; back to the layer below.
    fn pick_channel(&mut self) {
        let Some(p) = self.card.picker.take() else {
            return;
        };
        let at = p.selected.checked_sub(p.mapped.is_some() as usize);
        let channel = match at {
            Some(i) => match p.found.get(i) {
                Some(c) => Some(c.id.clone()),
                None => {
                    self.card.picker = Some(p);
                    return;
                }
            },
            None => None,
        };
        let status = match &channel {
            Some(_) => format!(
                "Mapped {} in {}: its programmes come with the next refresh",
                p.name, p.label
            ),
            None => format!("{} in {} is matched automatically again", p.name, p.label),
        };
        self.write_guide(GuideEdit::Map {
            source: p.source,
            stream_id: p.stream_id,
            channel,
        });
        self.set_status(&status);
        self.show_card();
    }

    /// `p` on a source in the `E` card: the channel pinned to it, or
    /// unpinned.
    fn card_pin(&mut self) {
        let Some(c) = &self.card.sources else {
            return;
        };
        let Some(s) = c.sources.get(c.selected) else {
            return;
        };
        let pin = s.pin.is_none().then_some((s.id, 0));
        let label = guide_label(self.provider(), &s.ident);
        let status = match pin {
            Some(_) if !s.enabled => {
                format!("Pinned to {label}, which is off: it follows the order until it is on")
            }
            Some(_) => format!("Pinned to {label}"),
            None => format!("Unpinned: {} follows the order", c.name),
        };
        let stream_id = c.stream_id;
        self.write_guide(GuideEdit::Pin(stream_id, pin));
        self.set_status(&status);
    }

    /// `h` / `l` on the pinned source: its shift 5 minutes less or more.
    fn card_shift(&mut self, d: isize) {
        let Some(c) = &self.card.sources else {
            return;
        };
        let Some(s) = c.sources.get(c.selected) else {
            return;
        };
        let Some(shift) = s.pin else {
            return self.set_status("p pins the channel to this source first");
        };
        let pin = Some((s.id, shift + 5 * d as i64));
        let stream_id = c.stream_id;
        self.write_guide(GuideEdit::Pin(stream_id, pin));
    }

    /// Sends a guide change to the writer for the provider shown.
    fn write_guide(&self, edit: GuideEdit) {
        let provider_id = self.provider().id.clone();
        let _ = self.write_tx.send(Write::Guide { provider_id, edit });
    }

    /// A guide change written: the card, the panes, the grid and Settings
    /// read it again, and the recorder its schedules.
    fn on_guide_written(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            log::error!("{e}");
            return self.set_error(&e);
        }
        let _ = self.recorder_tx.send(recorder::Command::Changed);
        self.load_card();
        self.show_card();
        self.refresh_times();
        if self.guide_open {
            self.load_guide(false);
        }
        if self.settings_view.is_some() {
            self.load_epg();
            self.show_settings();
        }
    }

    /// A key while the card is open. The picker and the ambiguous list
    /// type into their filter as search does; the sources move with
    /// `j`/`k`, map with Enter, pin with `p` and shift with `h`/`l`. The
    /// card is modal: other keys do nothing.
    fn card_key(&mut self, text: &str, ctrl: bool) -> bool {
        let at = self.card_selected() as isize;
        let page = 8;
        let nav = |this: &mut Self, a: &Action| match a {
            Action::Move(d) => this.card_select((at + d).max(0) as usize),
            Action::Page(d) | Action::HalfPage(d) => {
                this.card_select((at + d * page).max(0) as usize)
            }
            Action::Top => this.card_select(0),
            Action::Bottom => this.card_select(usize::MAX),
            #[cfg(feature = "devtools")]
            Action::Snapshot => crate::devtools::snapshot(&this.window()),
            _ => {}
        };
        if self.card.picker.is_some() || self.card.ambiguous.is_some() {
            match search_key(text, ctrl) {
                Some(SearchKey::Nav(a)) => nav(self, &a),
                Some(SearchKey::Keep) => self.card_activate(),
                Some(SearchKey::Clear) => self.close_card(),
                Some(key @ (SearchKey::Type(_) | SearchKey::Backspace)) => {
                    let query = if let Some(p) = &mut self.card.picker {
                        &mut p.query
                    } else if let Some(a) = &mut self.card.ambiguous {
                        &mut a.query
                    } else {
                        return true;
                    };
                    match key {
                        SearchKey::Type(t) => query.push_str(&t),
                        _ => drop(query.pop()),
                    }
                    if self.card.picker.is_some() {
                        self.picker_search();
                    } else if let Some(a) = &mut self.card.ambiguous {
                        a.selected = 0;
                        a.filter();
                        self.window().invoke_card_to_top();
                        self.show_card();
                    }
                }
                None => {}
            }
            return true;
        }
        match action(text, ctrl) {
            Some(Action::Play) => self.card_activate(),
            Some(Action::Providers) => self.card_pin(),
            Some(Action::Category(d)) => self.card_shift(d),
            Some(Action::ClearFilter | Action::GuideSources | Action::Quit) => self.close_card(),
            Some(a) => nav(self, &a),
            None => {}
        }
        true
    }

    /// A click on the card's row `i`: selected, and on a second click of
    /// a double click, as Enter.
    fn card_click(&mut self, i: usize, activate: bool) {
        self.card_select(i);
        if activate {
            self.card_activate();
        }
    }

    /// The Pin / Unpin button of the `E` card's row `i`.
    fn card_pin_clicked(&mut self, i: usize) {
        self.card_select(i);
        self.card_pin();
    }

    /// The source form's "Ambiguous channels…" row: the provider's
    /// channels that matched several of the source's, each opening the
    /// picker.
    fn epg_ambiguous(&mut self) {
        let provider = self.provider().clone();
        let Some(index) = self
            .settings_view
            .as_ref()
            .and_then(|v| v.epg.as_ref())
            .and_then(|e| e.form.as_ref())
            .and_then(|f| f.index)
        else {
            return;
        };
        let order = config::epg_order(&provider);
        let Some(ident) = order.get(index).map(EpgSource::ident) else {
            return;
        };
        let source = match store::source_id(&self.conn, &provider.id, &ident) {
            Ok(Some(id)) => id,
            Ok(None) => return,
            Err(e) => {
                log::error!("reading guide sources: {e}");
                return;
            }
        };
        self.card.ambiguous = Some(AmbiguousCard {
            source,
            label: config::epg_labels(&provider)[index].clone(),
            all: Vec::new(),
            query: String::new(),
            shown: Vec::new(),
            selected: 0,
        });
        self.load_card();
        self.window().invoke_card_to_top();
        self.show_card();
    }

    /// "Check and save" (onboarding ticket 01): the form's values checked
    /// here, then the account checked through the new provider's own
    /// rate-limited client (SPEC 6.2). A value missing selects its field
    /// and says so. "Save changes" (ticket 02) checks on a client of its
    /// own too, unless only the name changed.
    fn check_provider(&mut self) {
        let taken: Vec<&str> = self
            .sources
            .iter()
            .map(|s| s.provider.id.as_str())
            .collect();
        let Some(view) = &mut self.settings_view else {
            return;
        };
        let Some(form) = &mut view.form else {
            return;
        };
        let missing = [
            "Must give a name",
            "Must give the server's address",
            "Must give a username",
            "Must give a password",
        ];
        if let Some(i) = form.values.iter().position(String::is_empty) {
            view.field = i;
            view.error = Some(missing[i].into());
            return self.show_settings();
        }
        let [name, url, username, password] = form.values.clone();
        // The form does not edit guide sources; an edit keeps them.
        let epg = self
            .sources
            .iter()
            .find(|s| form.id.as_ref() == Some(&s.provider.id))
            .map(|s| s.provider.epg.clone())
            .unwrap_or_default();
        let provider = Provider {
            id: form
                .id
                .clone()
                .unwrap_or_else(|| config::slug(&name, &taken)),
            name,
            url,
            username,
            password,
            epg,
        };
        // Only the name changed: saved without a check.
        if let Some(source) = self
            .sources
            .iter()
            .find(|s| form.id.as_ref() == Some(&s.provider.id))
        {
            let p = &source.provider;
            if (&p.url, &p.username, &p.password)
                == (&provider.url, &provider.username, &provider.password)
            {
                let client = source.client.clone();
                return self.save_provider(Checked {
                    provider,
                    client,
                    info: None,
                });
            }
        }
        self.checks += 1;
        form.check = FormCheck::Checking(self.checks);
        view.error = None;
        let client = Arc::new(Client::new(provider.clone()));
        let (n, tx) = (self.checks, self.checked_tx.clone());
        self.starter.rt.spawn(async move {
            let check = client.check().await;
            let checked = Checked {
                provider,
                client,
                info: None,
            };
            let _ = tx.send((n, checked, check));
        });
        self.show_settings();
    }

    /// The form's account check answered: saved if it logged in to an
    /// active account; wrong credentials refused; an expired or disabled
    /// account, no answer or not Xtream warned, with `Save anyway`. An
    /// answer for a form since changed or closed is dropped.
    fn provider_checked(&mut self, n: u64, mut checked: Checked, check: xtream::Check) {
        let Some(view) = &mut self.settings_view else {
            return;
        };
        let Some(form) = &mut view.form else {
            return;
        };
        if !matches!(form.check, FormCheck::Checking(m) if m == n) {
            return;
        }
        // Errors start with the provider's name; the form names it already.
        let prefix = format!("{}: ", checked.provider.name);
        let why = |e: &str| e.strip_prefix(&prefix).unwrap_or(e).to_owned();
        let now = import::unix_now();
        let warning = match check {
            xtream::Check::Account(a) => {
                let blocked = a.user_info.blocked(now);
                let info = AccountInfo {
                    user: a.user_info,
                    server: a.server_info,
                    blocked,
                    checked: now,
                };
                let warning = info.blocked.is_some().then(|| account_text(&info, now).0);
                checked.info = Some(info);
                warning
            }
            xtream::Check::Rejected => {
                form.check = FormCheck::None;
                view.field = FORM_FIELDS.len();
                view.error = Some("The provider refused this username and password".into());
                return self.show_settings();
            }
            xtream::Check::Unreachable(e) => Some(format!("Cannot reach the server: {}", why(&e))),
            xtream::Check::Forbidden(s) => Some(format!(
                "The server refused access (HTTP {}): check the address",
                s.as_u16()
            )),
            xtream::Check::NotXtream(e) => Some(format!("Not an Xtream server: {}", why(&e))),
        };
        match warning {
            Some(w) => {
                form.check = FormCheck::Warned(w, Box::new(checked));
                self.show_settings();
            }
            None => self.save_provider(checked),
        }
    }

    /// `Save anyway`, or Enter, on the form's warning.
    fn save_anyway(&mut self) {
        let Some(form) = self.settings_view.as_mut().and_then(|v| v.form.as_mut()) else {
            return;
        };
        if let FormCheck::Warned(_, checked) = std::mem::take(&mut form.check) {
            self.save_provider(*checked);
        }
    }

    /// Writes the checked provider to `credentials.toml` and starts it, so
    /// it is in the switcher at once, its check shown until its own runs;
    /// an edited one is changed in place, its client using it from the
    /// next request (ticket 02). A refusal shows under the check-and-save
    /// row.
    fn save_provider(&mut self, checked: Checked) {
        let Some(view) = &mut self.settings_view else {
            return;
        };
        let edited = view.form.as_ref().and_then(|f| f.id.clone());
        let saved = match edited {
            Some(_) => config::edit_provider(&self.credentials_path, &checked.provider),
            None => config::add_provider(&self.credentials_path, &checked.provider),
        };
        if let Err(e) = saved {
            log::error!("{e}");
            if let Some(form) = &mut view.form {
                form.check = FormCheck::None;
            }
            view.field = FORM_FIELDS.len();
            view.error = Some(e);
            return self.show_settings();
        }
        let Checked {
            provider,
            client,
            info,
        } = checked;
        let name = provider.name.clone();
        if edited.is_some() {
            let Some(i) = self
                .sources
                .iter()
                .position(|s| s.provider.id == provider.id)
            else {
                return;
            };
            let source = &mut self.sources[i];
            source.client.set_provider(provider.clone());
            source.provider = provider;
            if info.is_some() {
                source.info = info;
            }
            view.form = None;
            view.error = None;
            view.field = i;
            self.show_settings();
            if i == self.active {
                self.show_source_status();
            }
            return self.set_status(&format!("Saved {name}"));
        }
        let first_run = first_run(&self.conn, &provider.id).unwrap_or(true);
        let max_age = i64::from(self.settings.refresh_hours) * 3600;
        let mut source = self.starter.start(provider, client, max_age, first_run);
        source.info = info;
        self.sources.push(source);
        if self.setup.is_some() {
            self.active = self.sources.len() - 1;
            self.leave_setup();
            return self.set_status(&format!("Added {name}"));
        }
        view.form = None;
        view.error = None;
        view.field = self.sources.len() - 1;
        self.show_settings();
        self.set_status(&format!("Added {name}: p switches to it"));
    }

    /// Enter on a provider row (onboarding ticket 02): the form, filled in
    /// with what is saved, to edit or remove it.
    fn edit_provider_form(&mut self, i: usize) {
        let (Some(view), Some(source)) = (&mut self.settings_view, self.sources.get(i)) else {
            return;
        };
        let p = &source.provider;
        view.form = Some(ProviderForm {
            values: [
                p.name.clone(),
                p.url.clone(),
                p.username.clone(),
                p.password.clone(),
            ],
            id: Some(p.id.clone()),
            ..Default::default()
        });
        view.field = 0;
        view.error = None;
        self.show_settings();
    }

    /// "Remove provider…": the confirm, with what goes and what stays,
    /// opening on its Remove row.
    fn ask_remove(&mut self) {
        let Some(view) = &mut self.settings_view else {
            return;
        };
        let Some(form) = &mut view.form else {
            return;
        };
        let Some(source) = self
            .sources
            .iter()
            .find(|s| form.id.as_ref() == Some(&s.provider.id))
        else {
            return;
        };
        match store::provider_data(&self.conn, &source.provider.id) {
            Ok(data) => {
                form.remove = Some(RemoveConfirm {
                    name: source.provider.name.clone(),
                    data,
                    personal: false,
                });
                view.field = 1;
                view.error = None;
            }
            Err(e) => {
                log::error!("{e}");
                view.error = Some(format!("Cannot count its data: {e}"));
            }
        }
        self.show_settings();
    }

    /// The confirm's Remove row: the provider leaves `credentials.toml`
    /// and the app at once, a pinned `active_provider` naming it cleared;
    /// the recorder stops its recordings and download, then deletes its
    /// data ([`recorder::Command::RemoveProvider`]). Removing the provider
    /// shown shows the next one, and removing the last the setup screen.
    fn remove_provider(&mut self) {
        let Some(view) = &mut self.settings_view else {
            return;
        };
        let Some(form) = &mut view.form else {
            return;
        };
        let (Some(id), Some(confirm)) = (form.id.clone(), &form.remove) else {
            return;
        };
        let (name, personal) = (confirm.name.clone(), confirm.personal);
        let Some(i) = self.sources.iter().position(|s| s.provider.id == id) else {
            return;
        };
        if let Err(e) = config::remove_provider(&self.credentials_path, &id) {
            log::error!("{e}");
            view.error = Some(e);
            return self.show_settings();
        }
        view.form = None;
        view.error = None;
        // Otherwise the next start refuses a provider that is not there.
        if self.settings.active_provider.as_deref() == Some(id.as_str()) {
            self.save_setting("active_provider", None);
        }
        let source = self.sources.remove(i);
        source.refresher.abort();
        self.starter.clients.remove(&id);
        let playing = self.now_playing.as_ref().map(|n| &n.provider_id) == Some(&id)
            || self.movie.as_ref().map(|m| &m.provider_id) == Some(&id);
        // With the last gone, a recording playing stops too.
        if playing || (self.sources.is_empty() && self.mpv.is_some()) {
            self.control(Control::Stop);
        }
        let _ = self.recorder_tx.send(recorder::Command::RemoveProvider {
            provider_id: id,
            personal,
        });
        if self.sources.is_empty() {
            return self.enter_setup(config::Setup::Empty);
        }
        if i < self.active {
            self.active -= 1;
        } else if i == self.active {
            self.active = i.min(self.sources.len() - 1);
            self.load_active();
        }
        if let Some(view) = &mut self.settings_view {
            view.field = i.min(self.sources.len() - 1);
        }
        self.show_settings();
        self.set_status(&format!("Removed {name}"));
    }

    /// Shows the setup screen for `setup` (onboarding ticket 03): over
    /// everything, the add-provider form when the file is missing or names
    /// no provider, else what is wrong with it and its rows.
    fn enter_setup(&mut self, setup: config::Setup) {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let (title, text) = setup_text(&setup, &home);
        let form = matches!(setup, config::Setup::Missing | config::Setup::Empty);
        let providers_managed = form
            .then(|| config::managed(&self.credentials_path))
            .flatten();
        self.settings_view = Some(SettingsView {
            section: SETTINGS_SECTIONS
                .iter()
                .position(|&s| s == "Providers")
                .unwrap_or(0),
            in_fields: true,
            form: (form && providers_managed.is_none()).then(ProviderForm::default),
            providers_managed,
            ..Default::default()
        });
        self.setup = Some(setup);
        let w = self.window();
        w.set_setup_title(title.into());
        w.set_setup_text(text.into());
        w.set_setup_open(true);
        w.set_settings_open(true);
        self.show_settings();
    }

    /// Enter on a setup screen row: `Fix permissions` sets the modes, then
    /// like `Check again` reads the file afresh. Providers in it start and
    /// the main screen shows; else the screen shows what is wrong now,
    /// saying so if nothing changed.
    fn setup_row(&mut self, i: usize) {
        let fix = matches!(self.setup, Some(config::Setup::Loose { fix: true, .. }));
        if fix
            && i == 0
            && let Err(e) = config::fix_permissions(&self.credentials_path)
        {
            log::error!("{e}");
            if let Some(view) = &mut self.settings_view {
                view.error = Some(e);
            }
            return self.show_settings();
        }
        match config::load_credentials(&self.credentials_path) {
            Ok(providers) => {
                let max_age = i64::from(self.settings.refresh_hours) * 3600;
                for provider in providers {
                    let first_run = first_run(&self.conn, &provider.id).unwrap_or(true);
                    let client = Arc::new(Client::new(provider.clone()));
                    let source = self.starter.start(provider, client, max_age, first_run);
                    self.sources.push(source);
                }
                let providers: Vec<Provider> =
                    self.sources.iter().map(|s| s.provider.clone()).collect();
                let last_used = store::last_provider(&self.conn).ok().flatten();
                self.active =
                    config::start_provider(&providers, &self.settings, last_used.as_deref())
                        .unwrap_or_else(|e| {
                            log::error!("{e}");
                            0
                        });
                self.leave_setup();
            }
            Err(setup) => {
                let same = self.setup.as_ref() == Some(&setup);
                self.enter_setup(setup);
                if same && let Some(view) = &mut self.settings_view {
                    view.field = i;
                    view.error = Some("Still the same: change the file, then check again".into());
                    self.show_settings();
                }
            }
        }
    }

    /// The setup screen closes on the provider `active` names, which
    /// starts its first refresh if it has none yet.
    fn leave_setup(&mut self) {
        self.setup = None;
        self.window().set_setup_open(false);
        self.close_settings();
        self.load_active();
    }

    /// Settings just saved, applied: a new `refresh_hours` re-arms every
    /// provider's check; the next mpv spawn, recording and download use
    /// the rest, and what runs keeps going (settings ticket 02).
    fn apply_settings(&mut self, settings: config::Settings) {
        if settings.refresh_hours != self.settings.refresh_hours {
            let age = i64::from(settings.refresh_hours) * 3600;
            for source in &self.sources {
                let _ = source.refresh_tx.send(Request::MaxAge(age));
            }
        }
        let _ = self
            .player_tx
            .send(PlayerCommand::Launch(launch(&settings)));
        let _ = self
            .recorder_tx
            .send(recorder::Command::Setup(recorder_setup(&settings)));
        self.recordings_dir = settings.recordings_dir.clone();
        self.downloads_dir = settings.downloads_dir.clone();
        self.record_after = i64::from(settings.record_after) * 60;
        self.settings = settings;
    }

    /// A key while the Settings view is open, which is modal: Tab between
    /// sections and fields, `j`/`k` move, Enter edits, Esc cancels an edit
    /// or closes, as `,` and `q` do.
    fn settings_key(&mut self, text: &str, ctrl: bool) -> bool {
        let items = self.settings_items();
        let Some(view) = &mut self.settings_view else {
            return true;
        };
        if view.editing.is_some() {
            self.settings_edit_key(text, ctrl);
            return true;
        }
        if ctrl && matches!(text, "r" | "R" | "\u{12}") {
            self.settings_reveal();
            return true;
        }
        // The setup screen has nothing to go back to: `q` quits the app.
        if self.setup.is_some() {
            match action(text, ctrl) {
                Some(Action::Quit) => {
                    let _ = slint::quit_event_loop();
                    return true;
                }
                Some(Action::ClearFilter | Action::Settings | Action::Cycle(_)) => return true,
                _ => {}
            }
        }
        if view.epg.is_some() && self.epg_key(action(text, ctrl)) {
            return true;
        }
        let Some(view) = &mut self.settings_view else {
            return true;
        };
        let step = |at: usize, d: isize, n: usize| {
            (at as isize + d).clamp(0, n.saturating_sub(1) as isize) as usize
        };
        match (view.choosing, action(text, ctrl)) {
            // Esc leaves the confirm for the form, on "Remove provider…".
            (None, Some(Action::ClearFilter))
                if view.form.as_ref().is_some_and(|f| f.remove.is_some()) =>
            {
                if let Some(form) = &mut view.form {
                    form.remove = None;
                }
                view.error = None;
                view.field = FORM_FIELDS.len() + 1;
                self.show_settings();
            }
            // Esc leaves the form for the providers, on the provider edited
            // or "Add provider…".
            (None, Some(Action::ClearFilter)) if view.form.is_some() => {
                let id = view.form.take().and_then(|f| f.id);
                view.error = None;
                view.field = self
                    .sources
                    .iter()
                    .position(|s| Some(&s.provider.id) == id.as_ref())
                    .unwrap_or(self.sources.len());
                self.show_settings();
            }
            (_, Some(Action::Settings | Action::Quit)) | (None, Some(Action::ClearFilter)) => {
                self.close_settings()
            }
            (Some(_), Some(Action::ClearFilter)) => {
                view.choosing = None;
                self.show_settings();
            }
            (Some(c), Some(Action::Move(d))) => {
                let n = items.get(view.field).map_or(0, |i| i.options.row_count());
                view.choosing = Some(step(c, d, n));
                self.show_settings();
            }
            (Some(c), Some(Action::Play)) => self.settings_choose(c),
            (None, Some(Action::Cycle(_))) => {
                view.in_fields = !view.in_fields && !items.is_empty();
                self.show_settings();
            }
            (None, Some(Action::Move(d))) if view.in_fields => {
                view.field = step(view.field, d, items.len());
                view.error = None;
                self.show_settings();
            }
            (None, Some(Action::Category(d))) if view.in_fields => {
                let field = view.field;
                self.settings_step(field, d as i32)
            }
            (None, Some(Action::Move(d))) => {
                let section = step(view.section, d, SETTINGS_SECTIONS.len());
                self.settings_section(section);
            }
            (None, Some(Action::Play)) if !view.in_fields => {
                view.in_fields = !items.is_empty();
                self.show_settings();
            }
            (None, Some(Action::Play)) => self.settings_activate(),
            #[cfg(feature = "devtools")]
            (_, Some(Action::Snapshot)) => crate::devtools::snapshot(&self.window()),
            _ => {}
        }
        true
    }

    /// `v`: opens the Recordings view (recording ticket 04) on its first
    /// recording.
    fn open_recordings(&mut self) {
        self.recordings = Some(RecView {
            downloads: self.downloads_tab,
            ..Default::default()
        });
        self.load_recordings();
        self.window().set_recordings_open(true);
    }

    /// Shows the Recordings tab, or with `downloads` the Downloads tab
    /// (VOD ticket 10), on its first line; the view opens on it next time.
    fn recordings_tab(&mut self, downloads: bool) {
        self.disarm_cancel();
        self.disarm_delete();
        self.disarm_clear_recordings();
        self.downloads_tab = downloads;
        if let Some(view) = &mut self.recordings
            && view.downloads != downloads
        {
            *view = RecView {
                downloads,
                ..Default::default()
            };
            self.load_recordings();
            self.window().invoke_show_recording(0.0, 0.0);
        }
    }

    fn close_recordings(&mut self) {
        self.disarm_cancel();
        self.disarm_delete();
        self.disarm_clear_recordings();
        self.recordings = None;
        let w = self.window();
        w.set_recordings_open(false);
        w.set_recordings(ModelRc::default());
    }

    /// Re-reads the Recordings view, if open, the selection staying on its
    /// recording, or where it was if that one left.
    fn load_recordings(&mut self) {
        let Some(downloads) = self.recordings.as_ref().map(|v| v.downloads) else {
            return;
        };
        let lines = if downloads {
            store::downloads(&self.conn).map(download_lines)
        } else {
            store::recordings(&self.conn).map(recording_lines)
        };
        let lines = match lines {
            Ok(lines) => lines,
            Err(e) => {
                log::error!("reading recordings: {e}");
                return self.set_error(&format!("reading recordings: {e}"));
            }
        };
        let Some(view) = &mut self.recordings else {
            return;
        };
        let rec = view.selected().map(|r| r.id);
        let dl = view.selected_dl().map(|d| d.id);
        view.lines = lines;
        if let Some(i) = view.lines.iter().position(|l| match l {
            RecLine::Rec(r) => Some(r.id) == rec,
            RecLine::Dl(d) => Some(d.id) == dl,
            RecLine::Heading(_) => false,
        }) {
            view.selected = i;
        }
        view.step(0);
        self.show_recordings();
    }

    fn show_recordings(&self) {
        let Some(view) = &self.recordings else { return };
        let now = import::unix_now();
        // With none running, the queue's first waits for a connection.
        let waiting = !view.lines.contains(&RecLine::Heading("DOWNLOADING"));
        let first = view.lines.iter().position(|l| matches!(l, RecLine::Dl(_)));
        let items: Vec<RecordingItem> = view
            .lines
            .iter()
            .enumerate()
            .map(|(i, l)| match l {
                RecLine::Heading(h) => RecordingItem {
                    heading: (*h).into(),
                    ..Default::default()
                },
                RecLine::Rec(r) => recording_item(r, now),
                RecLine::Dl(d) => download_item(d, waiting && Some(i) == first),
            })
            .collect();
        let top: f32 = view.lines[..view.selected.min(view.lines.len())]
            .iter()
            .map(RecLine::height)
            .sum();
        let w = self.window();
        let last = if view.downloads { FINISHED } else { HISTORY };
        w.set_recordings_can_clear(view.lines.contains(&RecLine::Heading(last)));
        w.set_recordings_tab(i32::from(view.downloads));
        w.set_recordings(ModelRc::new(VecModel::from(items)));
        w.set_recordings_selected(view.selected as i32);
        w.invoke_show_recording(top, top + REC_ROW_HEIGHT);
    }

    /// Selects line `i` of the Recordings view, if it is a recording.
    fn select_recording(&mut self, i: usize) {
        self.disarm_cancel();
        self.disarm_delete();
        self.disarm_clear_recordings();
        if let Some(view) = &mut self.recordings
            && matches!(view.lines.get(i), Some(RecLine::Rec(_) | RecLine::Dl(_)))
        {
            view.selected = i;
            self.show_recordings();
        }
    }

    /// A key while the Recordings view is open, which is modal: `j`/`k`
    /// move, `x` cancels, `d` deletes, Enter plays, `o` opens the folder,
    /// Esc, `v` or `q` close.
    fn recordings_key(&mut self, text: &str, ctrl: bool) -> bool {
        if ctrl || text != "x" {
            self.disarm_cancel();
        }
        if ctrl || text != "d" {
            self.disarm_delete();
        }
        if ctrl || text != "c" {
            self.disarm_clear_recordings();
        }
        let downloads = self.recordings.as_ref().is_some_and(|v| v.downloads);
        match text {
            "x" if !ctrl && downloads => self.dl_cancel_pressed(),
            "x" if !ctrl => self.cancel_pressed(),
            "d" if !ctrl && downloads => self.dl_delete_pressed(),
            "d" if !ctrl => self.delete_pressed(),
            "c" if !ctrl => self.clear_recordings_pressed("Press c again"),
            "o" if !ctrl => self.open_recording_folder(),
            _ => {
                let step = |s: &mut Self, d: isize, from: Option<usize>| {
                    if let Some(view) = &mut s.recordings {
                        if let Some(from) = from {
                            view.selected = from;
                        }
                        view.step(d);
                        s.show_recordings();
                    }
                };
                match action(text, ctrl) {
                    Some(Action::Move(d)) => step(self, d, None),
                    Some(Action::Top) => step(self, 0, Some(0)),
                    Some(Action::Bottom) => step(self, 0, Some(usize::MAX)),
                    Some(Action::Play) => self.play_recording(),
                    Some(Action::Cycle(_)) => self.recordings_tab(!downloads),
                    Some(Action::ClearFilter | Action::Recordings | Action::Quit) => {
                        self.close_recordings();
                    }
                    #[cfg(feature = "devtools")]
                    Some(Action::Snapshot) => crate::devtools::snapshot(&self.window()),
                    _ => {}
                }
            }
        }
        true
    }

    /// `x` in the Recordings view: a first press arms the cancel of the
    /// selected scheduled or running recording, a second within
    /// [`CLEAR_CONFIRM`] cancels it, as `c` clears recently played.
    fn cancel_pressed(&mut self) {
        let Some(r) = self.recordings.as_ref().and_then(|v| v.selected()).cloned() else {
            return;
        };
        if !matches!(
            r.state,
            store::RecState::Scheduled | store::RecState::Recording
        ) {
            return self.set_status("x cancels a scheduled or running recording");
        }
        let w = self.window();
        if w.get_recordings_armed() == r.id as i32 {
            self.disarm_cancel();
            // One of what plays is mpv's to stop; the recorder, told first,
            // keeps it as cancelled when it ends.
            let playing = self
                .mpv
                .as_ref()
                .and_then(|m| m.recording.as_ref())
                .is_some_and(|(_, f)| r.file.as_deref().map(std::path::Path::new) == Some(f));
            let _ = self.recorder_tx.send(recorder::Command::Cancel(r.id));
            if playing {
                let _ = self.player_tx.send(PlayerCommand::Record(None));
            }
            return;
        }
        w.set_recordings_armed(r.id as i32);
        let verb = match r.state {
            store::RecState::Recording => "stop and cancel",
            _ => "cancel",
        };
        self.set_status(&format!("Press x again to {verb} {}", rec_what(&r)));
        let weak = self.window.clone();
        self.cancel_timer
            .start(slint::TimerMode::SingleShot, CLEAR_CONFIRM, move || {
                if let Some(w) = weak.upgrade() {
                    w.set_recordings_armed(-1);
                    w.set_status("".into());
                }
            });
    }

    /// `d` in the Recordings view (recording ticket 05): a first press arms
    /// the delete of the selected finished recording, a second within
    /// [`CLEAR_CONFIRM`] moves its file to the trash and forgets it. A file
    /// outside `recordings_dir` is refused at once.
    fn delete_pressed(&mut self) {
        let Some(r) = self.recordings.as_ref().and_then(|v| v.selected()).cloned() else {
            return;
        };
        let file = match recorder::deletable(&r, &self.recordings_dir) {
            Ok(file) => file,
            Err(refused) => {
                let (message, _) = deleted_message(&r, &refused, &self.recordings_dir);
                return self.set_status(&message);
            }
        };
        let w = self.window();
        if w.get_recordings_delete_armed() == r.id as i32 {
            self.disarm_delete();
            let _ = self.recorder_tx.send(recorder::Command::Delete(r.id));
            return;
        }
        w.set_recordings_delete_armed(r.id as i32);
        let message = match (file, &r.file) {
            (Some(file), _) => {
                let size = std::fs::metadata(&file).map_or(0, |m| m.len());
                format!(
                    "Press d again to move {} ({}) to the trash",
                    file_name(&file.to_string_lossy()),
                    file_size(size)
                )
            }
            (None, Some(_)) => format!(
                "Press d again to remove {} from the list; its file is gone",
                rec_what(&r)
            ),
            (None, None) => format!(
                "Press d again to remove {} from the list; it has no file",
                rec_what(&r)
            ),
        };
        self.set_status(&message);
        let weak = self.window.clone();
        self.cancel_timer
            .start(slint::TimerMode::SingleShot, CLEAR_CONFIRM, move || {
                if let Some(w) = weak.upgrade() {
                    w.set_recordings_delete_armed(-1);
                    w.set_status("".into());
                }
            });
    }

    /// `x` on the Downloads tab (VOD ticket 10): a first press arms the
    /// cancel of the selected queued or running download, a second within
    /// [`CLEAR_CONFIRM`] cancels it, removing its partial file.
    fn dl_cancel_pressed(&mut self) {
        let Some(d) = self
            .recordings
            .as_ref()
            .and_then(|v| v.selected_dl())
            .cloned()
        else {
            return;
        };
        if !matches!(
            d.state,
            store::DlState::Queued | store::DlState::Downloading
        ) {
            return self.set_status("x cancels a queued or running download");
        }
        let w = self.window();
        if w.get_recordings_armed() == d.id as i32 {
            self.disarm_cancel();
            let _ = self
                .recorder_tx
                .send(recorder::Command::CancelDownload(d.id));
            return;
        }
        w.set_recordings_armed(d.id as i32);
        self.set_status(&format!(
            "Press x again to cancel downloading {} and remove what it has so far",
            dl_what(&d)
        ));
        let weak = self.window.clone();
        self.cancel_timer
            .start(slint::TimerMode::SingleShot, CLEAR_CONFIRM, move || {
                if let Some(w) = weak.upgrade() {
                    w.set_recordings_armed(-1);
                    w.set_status("".into());
                }
            });
    }

    /// `d` on the Downloads tab: a first press arms the delete of the
    /// selected done or failed download, a second within [`CLEAR_CONFIRM`]
    /// moves a done one's file to the trash, or a failed one's partial file
    /// away, and forgets it. A file outside `downloads_dir` is refused.
    fn dl_delete_pressed(&mut self) {
        let Some(d) = self
            .recordings
            .as_ref()
            .and_then(|v| v.selected_dl())
            .cloned()
        else {
            return;
        };
        let file = match recorder::download_deletable(&d, &self.downloads_dir) {
            Ok(file) => file,
            Err(refused) => {
                let (message, _) = dl_deleted_message(&d, &refused, &self.downloads_dir);
                return self.set_status(&message);
            }
        };
        let w = self.window();
        if w.get_recordings_delete_armed() == d.id as i32 {
            self.disarm_delete();
            let _ = self
                .recorder_tx
                .send(recorder::Command::DeleteDownload(d.id));
            return;
        }
        w.set_recordings_delete_armed(d.id as i32);
        let message = match file {
            Some(file) => {
                let size = std::fs::metadata(&file).map_or(0, |m| m.len());
                format!(
                    "Press d again to move {} ({}) to the trash",
                    file_name(&file.to_string_lossy()),
                    file_size(size)
                )
            }
            None if d.state == store::DlState::Failed => format!(
                "Press d again to remove {} from the list, and its partial file",
                dl_what(&d)
            ),
            None => format!(
                "Press d again to remove {} from the list; its file is gone",
                dl_what(&d)
            ),
        };
        self.set_status(&message);
        let weak = self.window.clone();
        self.cancel_timer
            .start(slint::TimerMode::SingleShot, CLEAR_CONFIRM, move || {
                if let Some(w) = weak.upgrade() {
                    w.set_recordings_delete_armed(-1);
                    w.set_status("".into());
                }
            });
    }

    /// Drops a pending delete and its status message.
    fn disarm_delete(&mut self) {
        let w = self.window();
        if w.get_recordings_delete_armed() < 0 {
            return;
        }
        self.cancel_timer.stop();
        w.set_recordings_delete_armed(-1);
        self.set_status("");
    }

    /// `c` or Clear in the Recordings view: the first press arms, a second
    /// within [`CLEAR_CONFIRM`] forgets every recording but scheduled and
    /// running ones, keeping their files.
    fn clear_recordings_pressed(&mut self, again: &str) {
        let w = self.window();
        if !w.get_recordings_can_clear() {
            return self.set_status("Nothing to clear");
        }
        let downloads = self.recordings.as_ref().is_some_and(|v| v.downloads);
        if w.get_recordings_clear_armed() {
            self.disarm_clear_recordings();
            let clear = match downloads {
                true => recorder::Command::ClearDownloads,
                false => recorder::Command::ClearFinished,
            };
            let _ = self.recorder_tx.send(clear);
            return;
        }
        w.set_recordings_clear_armed(true);
        self.set_status(&match downloads {
            true => format!(
                "{again} to clear done and failed downloads from the list (files are kept)"
            ),
            false => format!(
                "{again} to clear done, failed, missed and cancelled recordings from the list (files are kept)"
            ),
        });
        let weak = self.window.clone();
        self.cancel_timer
            .start(slint::TimerMode::SingleShot, CLEAR_CONFIRM, move || {
                if let Some(w) = weak.upgrade() {
                    w.set_recordings_clear_armed(false);
                    w.set_status("".into());
                }
            });
    }

    /// Drops a pending clear of the Recordings view and its status message.
    fn disarm_clear_recordings(&mut self) {
        let w = self.window();
        if !w.get_recordings_clear_armed() {
            return;
        }
        self.cancel_timer.stop();
        w.set_recordings_clear_armed(false);
        self.set_status("");
    }

    /// Drops a pending cancel and its status message.
    fn disarm_cancel(&mut self) {
        let w = self.window();
        if w.get_recordings_armed() < 0 {
            return;
        }
        self.cancel_timer.stop();
        w.set_recordings_armed(-1);
        self.set_status("");
    }

    /// Enter in the Recordings view: plays a finished recording's file in
    /// mpv. A file is no channel and takes no connection.
    fn play_recording(&mut self) {
        if let Some(d) = self
            .recordings
            .as_ref()
            .and_then(|v| v.selected_dl())
            .cloned()
        {
            if d.state != store::DlState::Done {
                return self.set_status("Enter plays a finished download");
            }
            let Some(file) = d.file.filter(|f| std::path::Path::new(f).exists()) else {
                return self.set_status("This download's file is gone");
            };
            return self.play_file(file);
        }
        let Some(r) = self.recordings.as_ref().and_then(|v| v.selected()).cloned() else {
            return;
        };
        if matches!(
            r.state,
            store::RecState::Scheduled | store::RecState::Recording
        ) {
            return self.set_status("Enter plays a finished recording");
        }
        let Some(file) = r.file.filter(|f| std::path::Path::new(f).exists()) else {
            return self.set_status("This recording has no file");
        };
        self.play_file(file);
    }

    /// Plays `file` in mpv: no channel, so no NOW PLAYING card, and no
    /// connection.
    fn play_file(&mut self, file: String) {
        if self.replaying {
            return;
        }
        let title = std::path::Path::new(&file)
            .file_stem()
            .map_or_else(|| file.clone(), |s| s.to_string_lossy().into_owned());
        log::info!("play recording {file}");
        let _ = self.recorder_tx.send(recorder::Command::Watching(None));
        self.keep_progress(false);
        self.send_play(file, title.clone(), None, Instant::now());
        self.now_playing = None;
        self.movie = None;
        self.set_status(&format!("Playing {title}"));
        self.show_playing();
    }

    /// `o` in the Recordings view: opens the selected recording's folder,
    /// or `recordings_dir` for one with no file, with `xdg-open`.
    fn open_recording_folder(&self) {
        let view = self.recordings.as_ref();
        let file = view.and_then(|v| match v.downloads {
            true => v.selected_dl()?.file.clone(),
            false => v.selected()?.file.clone(),
        });
        let home = match view.is_some_and(|v| v.downloads) {
            true => &self.downloads_dir,
            false => &self.recordings_dir,
        };
        let dir = file
            .and_then(|f| std::path::Path::new(&f).parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| home.clone());
        if !dir.is_dir() {
            return self.set_status(&format!("{} does not exist yet", dir.display()));
        }
        if self.replaying {
            return;
        }
        let spawned = std::process::Command::new("xdg-open")
            .arg(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match spawned {
            Ok(mut child) => {
                // Reaped off the UI thread.
                std::thread::spawn(move || child.wait());
                self.set_status(&format!("Opened {}", dir.display()));
            }
            Err(e) => self.set_error(&format!("Can't open {}: xdg-open: {e}", dir.display())),
        }
    }

    /// Shows provider `i` from its cached data (ADR 0003), and remembers it
    /// for the next launch.
    fn switch_provider(&mut self, i: usize) {
        self.close_picker();
        if i == self.active || i >= self.sources.len() {
            return;
        }
        self.active = i;
        self.load_active();
    }

    /// Shows the provider `active` names, as [`Self::switch_provider`]
    /// does, or after the one shown was removed (onboarding ticket 02).
    fn load_active(&mut self) {
        let t = Instant::now();
        let id = self.provider().id.clone();
        let _ = self.write_tx.send(Write::LastProvider(id.clone()));
        // Row keys, the filter and the index belong to the old provider.
        self.categories.clear();
        self.query.clear();
        self.searching = false;
        self.wide = None;
        for kind in Kind::ALL {
            self.reindex(kind);
        }
        self.show_source_status();
        match self.reload() {
            Ok(()) => {
                self.set_status("");
            }
            Err(e) => {
                log::error!("{e}");
                self.set_error(&e);
            }
        }
        log::debug!(
            "switched to {:?} in {} µs",
            self.provider().id,
            t.elapsed().as_micros()
        );
    }

    /// A key while the picker is open. j/k and arrows move, Enter switches,
    /// Esc or `p` closes, `1`–`9` switch directly.
    fn picker_key(&mut self, text: &str, ctrl: bool) -> bool {
        let Some(at) = self.picking else {
            return false;
        };
        if let Some(n) = text.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
            self.switch_provider(n - 1);
            return true;
        }
        match action(text, ctrl) {
            Some(Action::Move(d)) => self.pick(at.saturating_add_signed(d)),
            Some(Action::Top) => self.pick(0),
            Some(Action::Bottom) => self.pick(usize::MAX),
            Some(Action::Play) => self.pick_provider(at),
            Some(Action::ClearFilter | Action::Providers | Action::Quit) => self.close_picker(),
            _ => {}
        }
        // The picker is modal: other keys do nothing.
        true
    }

    fn edit_search(&mut self, key: SearchKey) {
        match key {
            SearchKey::Type(text) => self.query.push_str(&text),
            SearchKey::Backspace => {
                self.query.pop();
            }
            SearchKey::Keep => {
                self.searching = false;
                self.window().set_searching(false);
                self.channels.set_searching(false);
                self.select(self.selected);
                return;
            }
            SearchKey::Clear => {
                // Stay on the selected channel in the full list.
                let key = self.channels.keys.borrow().get(self.selected).copied();
                self.searching = false;
                self.query.clear();
                self.wide = None;
                self.show_keys(true);
                if let Some(i) = key.and_then(|k| self.base.iter().position(|&b| b == k)) {
                    self.select(i);
                }
                return;
            }
            SearchKey::Nav(_) => unreachable!("handled by handle_key"),
        }
        self.show_keys(true);
    }

    /// The one key handler, for real and replayed input. True if handled.
    fn handle_key(&mut self, text: &str, ctrl: bool) -> bool {
        let clearing = self.picking.is_none()
            && !self.card.is_open()
            && self.dashboard.is_none()
            && !self.keys_open
            && self.recordings.is_none()
            && self.settings_view.is_none()
            && !self.searching
            && self.manage.is_none()
            && action(text, ctrl) == Some(Action::ClearHistory);
        if !clearing {
            self.disarm_clear();
        }
        if self.picking.is_some() {
            return self.picker_key(text, ctrl);
        }
        if self.card.is_open() {
            return self.card_key(text, ctrl);
        }
        if self.dashboard.is_some() {
            return self.dashboard_key(text, ctrl);
        }
        if self.keys_open {
            // Modal: Esc, `?` or `q` close, the moves scroll it when it
            // outgrows the window, other keys do nothing.
            let scroll =
                |across, rows, pages| self.window().invoke_scroll_keys(across, rows, pages);
            match action(text, ctrl) {
                Some(Action::ClearFilter | Action::Keys | Action::Quit) => {
                    self.show_keys_panel(false)
                }
                Some(Action::Move(d)) => scroll(0.0, d as f32, 0.0),
                Some(Action::HalfPage(d)) => scroll(0.0, 0.0, d as f32 / 2.0),
                Some(Action::Page(d)) => scroll(0.0, 0.0, d as f32),
                Some(Action::Top) => scroll(0.0, 0.0, -1000.0),
                Some(Action::Bottom) => scroll(0.0, 0.0, 1000.0),
                Some(Action::Category(d)) => scroll(d as f32, 0.0, 0.0),
                _ => {}
            }
            return true;
        }
        if self.recordings.is_some() {
            return self.recordings_key(text, ctrl);
        }
        if self.settings_view.is_some() {
            return self.settings_key(text, ctrl);
        }
        // The refresh panel closes before anything else Esc does.
        if self.window().get_refresh_panel_open() && action(text, ctrl) == Some(Action::ClearFilter)
        {
            self.refresh_panel(PanelEvent::Dismissed, false);
            return true;
        }
        if self.manage.is_some() {
            return self.manage_key(text, ctrl);
        }
        if self.next.is_some() && action(text, ctrl) == Some(Action::ClearFilter) {
            self.cancel_next();
            return true;
        }
        let action = if self.guide_open && self.guide_searching {
            match search_key(text, ctrl) {
                Some(SearchKey::Nav(a)) => a,
                Some(key) => {
                    self.edit_guide_search(key);
                    return true;
                }
                None => return false,
            }
        } else if self.searching {
            if text.chars().eq([char::from(slint::platform::Key::Tab)]) {
                self.toggle_wide();
                return true;
            }
            match search_key(text, ctrl) {
                Some(SearchKey::Nav(a)) => a,
                Some(key) => {
                    self.edit_search(key);
                    return true;
                }
                None => return false,
            }
        } else {
            match action(text, ctrl) {
                Some(a) => a,
                None => return false,
            }
        };
        if self.series.focused && self.episodes_key(&action) {
            return true;
        }
        if self.alternates.focused && self.alternates_key(&action) {
            return true;
        }
        // Jumps through time, and `[` and `]`, are the grid's alone.
        if !self.guide_open && matches!(action, Action::Jump(_) | Action::GuideCategory(_)) {
            return false;
        }
        // Not in the grid: these belong to the list. `h`/`l` move between
        // programmes there, and `/` and Esc search its own channels.
        if self.guide_open
            && matches!(
                action,
                Action::MoveFavourite(_)
                    | Action::ClearHistory
                    | Action::Watched
                    | Action::Download
                    | Action::DownloadSeason
                    | Action::ToggleOrder
                    | Action::Manage
                    | Action::Alternates
            )
        {
            return true;
        }
        let page = self.window().get_page_rows().max(1) as isize;
        let sel = if self.guide_open {
            self.guide_selected
        } else {
            self.selected
        } as isize;
        let clamp = |i: isize| i.max(0) as usize;
        match action {
            Action::Move(d) => self.move_to(clamp(sel + d)),
            Action::HalfPage(d) => self.move_to(clamp(sel + d * (page / 2).max(1))),
            Action::Page(d) => self.move_to(clamp(sel + d * page)),
            Action::Top => self.move_to(0),
            Action::Bottom => self.move_to(usize::MAX),
            Action::Category(d) if self.guide_open => self.guide_step(d),
            Action::Jump(j) => self.jump_guide(j),
            Action::GuideCategory(d) | Action::Cycle(d) if self.guide_open => {
                self.guide_category(d)
            }
            Action::GuideCategory(_) => {}
            Action::Cycle(d) => self.cycle_kind(d),
            Action::Category(d) => {
                if let Err(e) = self.set_category(clamp(self.category as isize + d)) {
                    log::error!("{e}");
                    self.set_error(&e);
                }
            }
            Action::Play if self.guide_open => self.guide_play(),
            Action::Play if self.listing() == Kind::Movie => self.play_movie(self.selected, false),
            Action::Play if self.listing() == Kind::Series => self.play(self.selected),
            Action::Play => {
                if let Some(key) = self.selected_key() {
                    self.play_key(key);
                }
            }
            Action::Guide
                if !self.guide_open
                    && (self.kind != Kind::Live
                        || self.categories[self.category] == Category::Continue) =>
            {
                self.set_status("The guide grid is for live channels")
            }
            Action::Guide => self.toggle_guide(),
            Action::Search if self.guide_open => {
                self.guide_searching = true;
                self.window().set_guide_searching(true);
            }
            Action::ClearFilter if self.guide_open => self.edit_guide_search(SearchKey::Clear),
            Action::Search => {
                self.searching = true;
                self.window().set_searching(true);
                self.channels.set_searching(true);
                // Rows are taller in search mode.
                self.select(self.selected);
            }
            Action::ClearFilter => self.edit_search(SearchKey::Clear),
            Action::Favourite if !self.guide_open && self.listing() != Kind::Live => {
                if let Some(row) = self.channels.title(self.selected) {
                    let kind = self.listing();
                    self.save_favourite(FavCommand::Toggle(kind, row.stream_id));
                }
            }
            Action::Favourite => {
                let row = self
                    .selected_key()
                    .and_then(|key| read_channel(&self.conn, key, import::unix_now()));
                if let Some(row) = row {
                    self.save_favourite(FavCommand::Toggle(row.kind, row.stream_id));
                }
            }
            Action::MoveFavourite(d) => {
                if !matches!(self.categories[self.category], Category::Favourites) {
                    self.set_status("J and K reorder the Favourites list");
                } else if let Some(row) = self.channels.title(self.selected) {
                    self.save_favourite(FavCommand::Move(self.listing(), row.stream_id, d));
                } else if let Some(row) = self.channels.row(self.selected, import::unix_now()) {
                    self.save_favourite(FavCommand::Move(row.kind, row.stream_id, d));
                }
            }
            Action::Refresh => {
                let _ = self.sources[self.active].refresh_tx.send(Request::Refresh);
                self.refresh_panel(PanelEvent::Asked, true);
            }
            Action::Providers => self.open_picker(),
            Action::Control(Control::Live) => self.go_live(),
            Action::Control(c) => self.control(c),
            Action::Record if self.guide_open => self.record_programme(),
            Action::Record => self.record(),
            Action::StartOver if self.guide_open => self.guide_start_over(),
            Action::StartOver if self.listing() == Kind::Movie => {
                self.play_movie(self.selected, true)
            }
            Action::StartOver if self.listing() == Kind::Series => {
                if self.categories[self.category] == Category::Continue {
                    self.play_up_next(true);
                } else {
                    self.set_status("S plays an episode from the start: Enter, then S on it");
                }
            }
            Action::StartOver => self.start_over_playing(),
            Action::LastChannel => self.last_channel(),
            Action::Alternates => self.open_alternates(),
            Action::ClearHistory => self.clear_pressed("Press c again"),
            Action::Watched => self.toggle_watched(),
            Action::Download => self.download_selected(),
            Action::DownloadSeason => self.download_season(),
            Action::ToggleOrder => self.toggle_order(),
            Action::Manage => self.open_manage(),
            Action::GuideSources => self.open_guide_card(),
            Action::Account => self.open_dashboard(),
            Action::Keys => self.show_keys_panel(true),
            Action::Recordings => self.open_recordings(),
            Action::Settings => self.open_settings(),
            Action::Quit => {
                let _ = slint::quit_event_loop();
            }
            #[cfg(feature = "devtools")]
            Action::Snapshot => crate::devtools::snapshot(&self.window()),
        }
        true
    }
}

/// A logo, or with `poster` a poster, for recently played or NOW PLAYING.
/// One not ready yet goes in `shown`, so they are redrawn when it arrives.
fn shown_logo(
    logos: &Logos,
    shown: &mut HashSet<String>,
    url: Option<&str>,
    size: f32,
    poster: bool,
) -> slint::Image {
    let logo = logos.image(url, size, poster);
    if let Some(url) = url.filter(|_| logo.size().width == 0) {
        shown.insert(url.to_owned());
    }
    logo
}

/// Reads a channel row, logging a failure.
fn read_channel(conn: &Connection, key: i64, now: i64) -> Option<ChannelRow> {
    store::channel(conn, key, now)
        .inspect_err(|e| log::error!("reading channel: {e}"))
        .ok()
        .flatten()
}

/// UI to player task.
enum PlayerCommand {
    Play(PlayRequest),
    Control(Control),
    /// Record what plays to this file, or with `None` stop.
    Record(Option<PathBuf>),
    /// Ask mpv where the picture is, for catch-up and movies.
    Position,
    /// Ask mpv how long what plays is, for a movie.
    Length,
    /// Show this on mpv's OSD for a while.
    Show(String),
    /// How the next mpv starts: the settings changed.
    Launch(Launch),
}

/// How mpv starts, from the settings.
fn launch(settings: &config::Settings) -> Launch {
    Launch {
        program: settings.mpv_path.clone(),
        extra_args: settings.extra_mpv_args.clone(),
        socket: config::mpv_socket_path(),
    }
}

/// How recordings and downloads are made, from the settings.
fn recorder_setup(settings: &config::Settings) -> recorder::Setup {
    recorder::Setup {
        ffmpeg: settings.ffmpeg_path.clone(),
        dir: settings.recordings_dir.clone(),
        trash: crate::trash::home_trash(),
        before: i64::from(settings.record_before) * 60,
        after: i64::from(settings.record_after) * 60,
        downloads: settings.downloads_dir.clone(),
    }
}

/// What to play and when the user asked for it.
struct PlayRequest {
    /// Counts the plays sent, for [`Event::Handed`].
    id: u64,
    url: String,
    title: String,
    /// Seconds in to start from: a movie or episode resuming.
    start: Option<f64>,
    pressed: Instant,
}

pub fn run(
    rt: &Handle,
    credentials: Result<Vec<Provider>, config::Setup>,
    credentials_path: PathBuf,
    settings: &config::Settings,
    opts: Options,
) -> Result<(), String> {
    let db_path = config::database_path();
    let open =
        || store::open(&db_path).map_err(|e| format!("cannot open {}: {e}", db_path.display()));
    let conn = Rc::new(open()?);
    // The worker runtime's single writer; `conn` is the UI's reader.
    let writer: import::Writer = Arc::new(Mutex::new(open()?));
    let last_used = store::last_provider(&conn).map_err(|e| format!("reading app state: {e}"))?;
    let order = store::order(&conn).map_err(|e| format!("reading app state: {e}"))?;
    let (providers, setup) = match credentials {
        Ok(providers) => (providers, None),
        Err(setup) => (Vec::new(), Some(setup)),
    };
    let active = match setup {
        None => config::start_provider(&providers, settings, last_used.as_deref())?,
        Some(_) => 0,
    };

    // Favourites, history, the last provider and the order are written on the writer too (SPEC
    // 3), so a change made during an import waits for it rather than
    // blocking the UI.
    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<Write>();
    let (written_tx, mut written_rx) = mpsc::unbounded_channel();
    {
        let writer = writer.clone();
        rt.spawn(async move {
            while let Some(write) = write_rx.recv().await {
                let writer = writer.clone();
                let done = tokio::task::spawn_blocking(move || {
                    let mut conn = writer.lock().unwrap_or_else(|e| e.into_inner());
                    match write {
                        Write::Favourite { provider_id, cmd } => {
                            let result = match cmd {
                                FavCommand::Toggle(kind, id) => {
                                    store::toggle_favourite(&conn, &provider_id, kind, id)
                                }
                                FavCommand::Move(kind, id, d) => {
                                    store::move_favourite(&mut conn, &provider_id, kind, id, d)
                                }
                            };
                            let result = result.map_err(|e| format!("saving favourites: {e}"));
                            Some(Written::Favourite(cmd, result))
                        }
                        Write::ClearContinue(provider_id) => Some(Written::ContinueCleared(
                            store::clear_continue(&conn, &provider_id, import::unix_now())
                                .map_err(|e| format!("clearing Continue watching: {e}")),
                        )),
                        Write::ClearHistory(provider_id) => Some(Written::HistoryCleared(
                            store::clear_history(&conn, &provider_id)
                                .map_err(|e| format!("clearing recently played: {e}")),
                        )),
                        Write::LastProvider(id) => {
                            if let Err(e) = store::set_last_provider(&conn, &id) {
                                log::error!("saving the last provider: {e}");
                            }
                            None
                        }
                        Write::Order(order) => {
                            if let Err(e) = store::set_order(&conn, order) {
                                log::error!("saving the list order: {e}");
                            }
                            None
                        }
                        Write::Library { provider_id, edit } => {
                            let result = match &edit {
                                LibraryEdit::Category(kind, id) => {
                                    store::toggle_category(&mut conn, &provider_id, *kind, id)
                                        .map(drop)
                                }
                                LibraryEdit::Rule(kind, pattern) => store::toggle_category_rule(
                                    &mut conn,
                                    &provider_id,
                                    *kind,
                                    pattern,
                                )
                                .map(drop),
                                LibraryEdit::NameRule(pattern) => {
                                    store::toggle_channel_rule(&mut conn, &provider_id, pattern)
                                        .map(drop)
                                }
                                LibraryEdit::KeptTag(tag) => {
                                    store::toggle_kept_tag(&mut conn, &provider_id, tag).map(drop)
                                }
                            };
                            Some(Written::Library(
                                result.map_err(|e| format!("saving categories: {e}")),
                            ))
                        }
                        Write::Guide { provider_id, edit } => {
                            let result = match &edit {
                                GuideEdit::Pin(stream_id, pin) => {
                                    store::set_pin(&mut conn, &provider_id, *stream_id, *pin)
                                }
                                GuideEdit::Map {
                                    source,
                                    stream_id,
                                    channel,
                                } => store::set_mapping(
                                    &mut conn,
                                    &provider_id,
                                    *source,
                                    *stream_id,
                                    channel.as_deref(),
                                ),
                                GuideEdit::Forget(ident) => {
                                    store::forget_mappings(&conn, &provider_id, ident).map(drop)
                                }
                            };
                            Some(Written::Guide(
                                result.map_err(|e| format!("saving the guide choice: {e}")),
                            ))
                        }
                        Write::Played {
                            provider_id,
                            kind,
                            stream_id,
                            at,
                        } => match store::record_play(&conn, &provider_id, kind, stream_id, at) {
                            Ok(()) => Some(Written::Played),
                            Err(e) => {
                                log::error!("saving recently played: {e}");
                                None
                            }
                        },
                        Write::Watched {
                            provider_id,
                            items,
                            watched,
                        } => {
                            let at = import::unix_now();
                            match store::set_watched(&mut conn, &provider_id, &items, watched, at) {
                                Ok(()) => Some(Written::Progress),
                                Err(e) => {
                                    log::error!("saving watched: {e}");
                                    None
                                }
                            }
                        }
                        Write::Progress(save) => match save.write(&conn) {
                            Ok(()) => Some(Written::Progress),
                            Err(e) => {
                                log::error!("saving where it stopped: {e}");
                                None
                            }
                        },
                    }
                })
                .await;
                match done {
                    Ok(Some(written)) => {
                        let _ = written_tx.send(written);
                    }
                    Ok(None) => {}
                    Err(e) => log::error!("writes task failed: {e}"),
                }
            }
        });
    }
    let window = AppWindow::new().map_err(|e| e.to_string())?;
    // Wayland app id for compositor window rules; mpv's is `vidicon-mpv`.
    // Must be set before the window is shown.
    slint::set_xdg_app_id("vidicon").map_err(|e| e.to_string())?;
    let (key_columns, key_notes) = key_panel();
    window.set_key_columns(ModelRc::new(VecModel::from(key_columns)));
    window.set_key_notes(ModelRc::new(VecModel::from(key_notes)));
    let sections: Vec<slint::SharedString> = SETTINGS_SECTIONS.map(Into::into).into();
    window.set_settings_sections(ModelRc::new(VecModel::from(sections)));

    let (player_tx, mut player_rx) = mpsc::unbounded_channel::<PlayerCommand>();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let launch = launch(settings);
    rt.spawn(async move {
        let mut player = Player::new(launch, event_tx.clone());
        loop {
            let cmd = tokio::select! {
                cmd = player_rx.recv() => cmd,
                wake = player.wait() => {
                    player.wake(wake).await;
                    continue;
                }
            };
            let Some(cmd) = cmd else {
                break;
            };
            match cmd {
                PlayerCommand::Play(req) => {
                    let _ = event_tx.send(Event::Handed(req.id));
                    let played = player
                        .play_at(&req.url, &req.title, req.start, req.pressed)
                        .await;
                    if let Err(reason) = played {
                        let _ = event_tx.send(Event::Failed {
                            title: req.title,
                            reason,
                        });
                    }
                }
                PlayerCommand::Control(c) => {
                    if let Err(e) = player.control(c).await {
                        log::error!("{e}");
                    }
                }
                PlayerCommand::Record(file) => {
                    if let Err(e) = player.record(file.as_deref()).await {
                        log::error!("{e}");
                    }
                }
                PlayerCommand::Position => player.position().await,
                PlayerCommand::Length => player.length().await,
                PlayerCommand::Show(text) => player.show(&text, OSD_SHOW).await,
                PlayerCommand::Launch(launch) => player.set_launch(launch),
            }
        }
    });

    // One refresh scheduler per provider, each with its own rate-limited
    // client (SPEC 6.2 limits are per provider).
    let (updates_tx, mut updates_rx) = mpsc::unbounded_channel();
    let starter = Starter {
        rt: rt.clone(),
        writer: writer.clone(),
        clients: xtream::Clients::default(),
        updates: updates_tx,
    };
    let mut sources = Vec::new();
    for provider in providers {
        let first_run =
            first_run(&conn, &provider.id).map_err(|e| format!("reading provider: {e}"))?;
        let client = Arc::new(Client::new(provider.clone()));
        let max_age = i64::from(settings.refresh_hours) * 3600;
        sources.push(starter.start(provider, client, max_age, first_run));
    }

    // Scheduled recordings, each on a connection of its own (recording
    // ticket 02).
    let (recorder_tx, recorder_rx) = mpsc::unbounded_channel();
    let (recorded_tx, mut recorded_rx) = mpsc::unbounded_channel();
    rt.spawn(Recorder::run(
        writer.clone(),
        starter.clients.clone(),
        recorder_setup(settings),
        recorded_tx,
        recorder_rx,
    ));

    // The guide's older days, fetched as grid rows ask (catch-up ticket 06).
    let (tables_tx, tables_rx) = mpsc::unbounded_channel();
    let (filled_tx, mut filled_rx) = mpsc::unbounded_channel();
    rt.spawn(tables::run(
        starter.clients.clone(),
        writer.clone(),
        tables_rx,
        filled_tx,
    ));

    // A movie's info, fetched as it is selected (VOD ticket 04).
    let (info_tx, info_rx) = mpsc::unbounded_channel();
    let (info_filled_tx, mut info_filled_rx) = mpsc::unbounded_channel();
    rt.spawn(info::run(
        starter.clients.clone(),
        writer.clone(),
        info_rx,
        info_filled_tx,
    ));

    let (logo_tx, mut logo_rx) = mpsc::unbounded_channel();
    let logo_requests = {
        let _guard = rt.enter();
        logos::spawn(
            config::logos_dir(),
            config::posters_dir(),
            starter.clients.clone(),
            xtream::SPEC_LIMITS,
            logo_tx,
        )
    };
    let logos = Rc::new(Logos {
        window: window.as_weak(),
        tx: logo_requests,
        images: RefCell::default(),
        failed: RefCell::default(),
        clock: Cell::new(0),
    });
    let channels = Rc::new(Channels {
        conn: conn.clone(),
        kind: Cell::new(Kind::Live),
        mixed: Cell::new(false),
        labelled: Cell::new(false),
        with_channels: Cell::new(false),
        carry_on: Cell::new(false),
        keys: RefCell::default(),
        notify: ModelNotify::default(),
        logos: logos.clone(),
        logo_size: Cell::new(ROW_LOGO),
        waiting: RefCell::default(),
    });
    window.set_channels(ModelRc::from(
        channels.clone() as Rc<dyn Model<Data = ChannelItem>>
    ));
    let guide = Rc::new(Guide {
        conn: conn.clone(),
        provider_id: RefCell::default(),
        keys: RefCell::default(),
        page: Cell::new((0, 0)),
        origin: Cell::new(0),
        blocks: RefCell::default(),
        sel: RefCell::default(),
        marks: RefCell::default(),
        notify: ModelNotify::default(),
        logos: logos.clone(),
        waiting: RefCell::default(),
        tables: tables_tx,
        asked: RefCell::default(),
        offsets: RefCell::default(),
    });
    window.set_guide_rows(ModelRc::from(
        guide.clone() as Rc<dyn Model<Data = GuideItem>>
    ));
    let (index_tx, mut index_rx) = mpsc::unbounded_channel();
    let rebuild_index = {
        let (rt, db_path) = (rt.clone(), db_path.clone());
        move |provider_id: String, kind: Kind| {
            let (tx, db_path) = (index_tx.clone(), db_path.clone());
            rt.spawn_blocking(move || {
                let t = Instant::now();
                match store::open_reader(&db_path)
                    .and_then(|c| search::Index::build(&c, &provider_id, kind))
                {
                    Ok(index) => {
                        log::debug!(
                            "search index: {} {} names in {} ms",
                            index.len(),
                            kind.as_str(),
                            t.elapsed().as_millis()
                        );
                        let _ = tx.send((provider_id, index));
                    }
                    Err(e) => log::error!("building search index: {e}"),
                }
            });
        }
    };
    let later = Rc::new(VecModel::default());
    window.set_detail_later(ModelRc::from(later.clone()));
    let manage_rows = Rc::new(VecModel::default());
    window.set_manage_rows(ModelRc::from(manage_rows.clone()));
    let settings_rows = Rc::new(VecModel::default());
    window.set_settings_fields(ModelRc::from(settings_rows.clone()));
    let (checked_tx, mut checked_rx) = mpsc::unbounded_channel();
    let (epg_tx, mut epg_rx) = mpsc::unbounded_channel();
    let screen = Rc::new(RefCell::new(Screen {
        window: window.as_weak(),
        conn,
        sources,
        active,
        setup: None,
        picking: None,
        dashboard: None,
        keys_open: false,
        card: Card::default(),
        recordings: None,
        settings_view: None,
        settings: settings.clone(),
        settings_path: config::settings_path(),
        credentials_path,
        starter,
        checked_tx,
        checks: 0,
        db_path: db_path.clone(),
        epg_tx,
        epg_reads: 0,
        cancel_timer: slint::Timer::default(),
        player_tx,
        recorder_tx: recorder_tx.clone(),
        write_tx,
        kind: Kind::Live,
        categories: Vec::new(),
        category: 0,
        order,
        hidden: false,
        manage: None,
        manage_rows,
        settings_rows,
        base: Vec::new(),
        channels,
        selected: 0,
        guide_open: false,
        guide,
        guide_selected: 0,
        guide_anchor: None,
        guide_on_now: true,
        guide_view: 0,
        guide_drag_from: 0,
        guide_base: Vec::new(),
        guide_back: guide_back(0),
        guide_query: String::new(),
        guide_searching: false,
        now_playing: None,
        movie: None,
        plays: 0,
        handed: 0,
        info_tx,
        info_asked: HashMap::new(),
        series: SeriesPanel::default(),
        alternates: AlternatesPanel::default(),
        mpv: None,
        recordings_dir: settings.recordings_dir.clone(),
        downloads_dir: settings.downloads_dir.clone(),
        downloads_tab: false,
        dl_asked: (String::new(), false),
        record_after: i64::from(settings.record_after) * 60,
        behind_timer: slint::Timer::default(),
        rec_timer: slint::Timer::default(),
        pos_timer: slint::Timer::default(),
        next: None,
        next_timer: slint::Timer::default(),
        recent: Vec::new(),
        clear_timer: slint::Timer::default(),
        panel_timer: slint::Timer::default(),
        stage_timer: slint::Timer::default(),
        later,
        logos,
        detail_logo: None,
        shown_logos: HashSet::new(),
        number: NumberEntry::default(),
        query: String::new(),
        searching: false,
        wide: None,
        search: search::Search::new(),
        rebuild_index: Box::new(rebuild_index),
        replaying: false,
    }));
    if setup.is_none() {
        let s = screen.borrow();
        for kind in Kind::ALL {
            (s.rebuild_index)(s.provider().id.clone(), kind);
        }
    }
    {
        let screen = screen.clone();
        slint::spawn_local(async move {
            while let Some(written) = written_rx.recv().await {
                let mut s = screen.borrow_mut();
                // What the setup screen covers needs a provider shown.
                if s.setup.is_some() {
                    continue;
                }
                match written {
                    Written::Favourite(cmd, result) => s.on_favourite(cmd, result),
                    Written::Played => s.on_played(),
                    Written::Progress => s.on_progress(),
                    Written::HistoryCleared(result) => s.on_history_cleared(result),
                    Written::ContinueCleared(result) => s.on_continue_cleared(result),
                    Written::Library(result) => s.on_library(result),
                    Written::Guide(result) => s.on_guide_written(result),
                }
            }
        })
        .map_err(|e| e.to_string())?;
    }
    {
        let screen = screen.clone();
        slint::spawn_local(async move {
            while let Some(event) = recorded_rx.recv().await {
                let mut s = screen.borrow_mut();
                if s.setup.is_none() {
                    s.on_recorder(event);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    }
    {
        let screen = screen.clone();
        slint::spawn_local(async move {
            while let Some(filled) = filled_rx.recv().await {
                let mut s = screen.borrow_mut();
                if s.setup.is_none() {
                    s.on_table(filled);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    }
    {
        let screen = screen.clone();
        slint::spawn_local(async move {
            while let Some(filled) = info_filled_rx.recv().await {
                let mut s = screen.borrow_mut();
                if s.setup.is_none() {
                    s.on_info(filled);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    }
    {
        let screen = screen.clone();
        slint::spawn_local(async move {
            while let Some(logo) = logo_rx.recv().await {
                let mut s = screen.borrow_mut();
                if s.setup.is_none() {
                    s.on_logo(logo);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    }
    {
        let screen = screen.clone();
        slint::spawn_local(async move {
            while let Some((provider_id, index)) = index_rx.recv().await {
                let mut s = screen.borrow_mut();
                if s.setup.is_none() {
                    s.set_index(&provider_id, index);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    }
    {
        let mut s = screen.borrow_mut();
        match setup {
            None => {
                s.reload()?;
                s.show_source_status();
            }
            Some(setup) => s.enter_setup(setup),
        }
    }

    {
        let screen = screen.clone();
        // Restarted by each digit typed in mpv.
        let number_timer = slint::Timer::default();
        slint::spawn_local(async move {
            while let Some(event) = event_rx.recv().await {
                match event {
                    Event::Remote(_) if screen.borrow().setup.is_some() => {}
                    Event::Remote(r) => {
                        let digit = screen.borrow_mut().on_remote(r);
                        tick_next(&screen);
                        if digit {
                            let screen = screen.clone();
                            number_timer.start(
                                slint::TimerMode::SingleShot,
                                NUMBER_IDLE,
                                move || {
                                    screen.borrow_mut().play_number();
                                },
                            );
                        }
                    }
                    Event::Failed { title, reason } => {
                        let reason = xtream::redact(&reason);
                        log::warn!("{title}: {reason}");
                        screen
                            .borrow()
                            .set_error(&format!("Can't play {title}: {reason}"));
                    }
                    event => {
                        screen.borrow_mut().on_player(event);
                        tick_behind(&screen);
                        tick_recording(&screen);
                        tick_position(&screen);
                        tick_next(&screen);
                    }
                }
            }
        })
        .map_err(|e| e.to_string())?;
    }

    {
        let screen = screen.clone();
        slint::spawn_local(async move {
            while let Some((n, checked, check)) = checked_rx.recv().await {
                screen.borrow_mut().provider_checked(n, checked, check);
            }
        })
        .map_err(|e| e.to_string())?;
    }
    {
        let screen = screen.clone();
        slint::spawn_local(async move {
            while let Some((n, provider_id, counts)) = epg_rx.recv().await {
                screen.borrow_mut().epg_counted(n, &provider_id, counts);
            }
        })
        .map_err(|e| e.to_string())?;
    }
    {
        let screen = screen.clone();
        slint::spawn_local(async move {
            while let Some((id, update)) = updates_rx.recv().await {
                let mut s = screen.borrow_mut();
                if let Some(i) = s.sources.iter().position(|x| x.provider.id == id) {
                    s.on_refresh(i, update);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    }
    tick_every_minute(screen.clone());

    {
        let screen = screen.clone();
        window.on_key(move |text, ctrl| screen.borrow_mut().handle_key(&text, ctrl));
    }
    {
        let screen = screen.clone();
        window.on_select_row(move |i| {
            let mut s = screen.borrow_mut();
            s.focus_episodes(false);
            s.select(i as usize);
        });
    }
    {
        let screen = screen.clone();
        window.on_pick_season(move |i| screen.borrow_mut().pick_season(i as usize));
    }
    {
        let screen = screen.clone();
        window.on_select_episode(move |i| {
            let mut s = screen.borrow_mut();
            s.focus_episodes(true);
            s.select_episode(i as usize);
        });
    }
    {
        let screen = screen.clone();
        window.on_play_episode(move |i| {
            let mut s = screen.borrow_mut();
            s.focus_episodes(true);
            s.select_episode(i as usize);
            s.play_episode(false);
        });
    }
    {
        let screen = screen.clone();
        window.on_play_row(move |i| {
            let mut s = screen.borrow_mut();
            s.select(i as usize);
            s.play(i as usize);
        });
    }
    {
        let screen = screen.clone();
        window.on_guide_click(move |i, x| screen.borrow_mut().guide_click(i as usize, x));
    }
    {
        let screen = screen.clone();
        window.on_guide_press(move || {
            let mut s = screen.borrow_mut();
            s.guide_drag_from = s.guide_view;
        });
    }
    window.on_guide_drag_axis(|dx, dy| match drag_axis(dx, dy) {
        None => 0,
        Some(DragAxis::Vertical) => 1,
        Some(DragAxis::Horizontal) => 2,
    });
    {
        let screen = screen.clone();
        window.on_guide_pan(move |dx| {
            let mut s = screen.borrow_mut();
            let from = s.guide_drag_from;
            s.guide_pan(from, dx);
        });
    }
    {
        let screen = screen.clone();
        window.on_guide_scroll(move |dx| {
            let mut s = screen.borrow_mut();
            let from = s.guide_view;
            s.guide_pan(from, dx);
        });
    }
    {
        let screen = screen.clone();
        window.on_guide_category(move |d| screen.borrow_mut().guide_category(d as isize));
    }
    {
        let screen = screen.clone();
        window.on_guide_play(move |i| {
            let mut s = screen.borrow_mut();
            s.guide_select(i as usize);
            s.guide_play();
        });
    }
    {
        let screen = screen.clone();
        window.on_play_recent(move |i| screen.borrow_mut().play_recent(i as usize));
    }
    {
        let screen = screen.clone();
        window.on_play_alternate(move |i| screen.borrow_mut().play_alternate(i as usize));
    }
    {
        let screen = screen.clone();
        window.on_open_alternates(move || screen.borrow_mut().open_alternates());
    }
    {
        let screen = screen.clone();
        window.on_control(move |c| {
            let control = match c {
                PlayerControl::Record => return screen.borrow_mut().record(),
                PlayerControl::Pause => Control::Pause,
                PlayerControl::Live => return screen.borrow_mut().go_live(),
                PlayerControl::Stop => Control::Stop,
                PlayerControl::Mute => Control::Mute,
                PlayerControl::Fullscreen => Control::Fullscreen,
            };
            screen.borrow().control(control);
        });
    }
    {
        let screen = screen.clone();
        window.on_pick_provider(move |i| screen.borrow_mut().pick_provider(i as usize));
    }
    {
        let screen = screen.clone();
        window.on_close_picker(move || screen.borrow_mut().close_picker());
    }
    {
        let screen = screen.clone();
        window.on_card_clicked(move |i| screen.borrow_mut().card_click(i as usize, false));
    }
    {
        let screen = screen.clone();
        window.on_card_activated(move |i| screen.borrow_mut().card_click(i as usize, true));
    }
    {
        let screen = screen.clone();
        window.on_card_action(move |i| screen.borrow_mut().card_pin_clicked(i as usize));
    }
    {
        let screen = screen.clone();
        window.on_close_card(move || screen.borrow_mut().close_card());
    }
    {
        let screen = screen.clone();
        window.on_open_picker(move || screen.borrow_mut().open_picker());
    }
    {
        let screen = screen.clone();
        window.on_close_keys(move || screen.borrow_mut().show_keys_panel(false));
    }
    {
        let screen = screen.clone();
        window.on_open_account(move || screen.borrow_mut().open_dashboard());
    }
    {
        let screen = screen.clone();
        window.on_open_refresh(move || {
            let screen = screen.borrow();
            let has_stages = !screen.sources[screen.active].stages.is_empty();
            if has_stages {
                screen.refresh_panel(PanelEvent::Asked, true);
            }
        });
    }
    {
        let screen = screen.clone();
        window
            .on_close_refresh(move || screen.borrow().refresh_panel(PanelEvent::Dismissed, false));
    }
    {
        let screen = screen.clone();
        window.on_close_account(move || screen.borrow_mut().close_dashboard());
    }
    {
        let screen = screen.clone();
        window.on_reveal(move |i| screen.borrow_mut().reveal(i as usize));
    }
    {
        let screen = screen.clone();
        window.on_close_recordings(move || screen.borrow_mut().close_recordings());
    }
    {
        let screen = screen.clone();
        window.on_open_settings(move || screen.borrow_mut().open_settings());
    }
    {
        let screen = screen.clone();
        window.on_close_settings(move || screen.borrow_mut().close_settings());
    }
    {
        let screen = screen.clone();
        window
            .on_settings_select_section(move |i| screen.borrow_mut().settings_section(i as usize));
    }
    {
        let screen = screen.clone();
        window.on_settings_select_field(move |i| {
            let mut s = screen.borrow_mut();
            s.settings_field(i as usize);
            s.settings_activate();
        });
    }
    {
        let screen = screen.clone();
        window.on_settings_step(move |field, d| {
            let mut s = screen.borrow_mut();
            s.settings_field(field as usize);
            s.settings_step(field as usize, d);
        });
    }
    {
        let screen = screen.clone();
        window.on_settings_button(move || screen.borrow_mut().settings_button());
    }
    {
        let screen = screen.clone();
        window.on_settings_reveal(move || screen.borrow_mut().settings_reveal());
    }
    {
        let screen = screen.clone();
        window.on_settings_drag(move |i, rows| {
            screen.borrow_mut().epg_drag(i as usize, rows as isize);
        });
    }
    {
        let screen = screen.clone();
        window.on_settings_choose(move |field, option| {
            let mut s = screen.borrow_mut();
            s.settings_field(field as usize);
            s.settings_choose(option as usize);
        });
    }
    {
        let screen = screen.clone();
        window.on_clear_recordings(move || {
            let mut s = screen.borrow_mut();
            s.disarm_cancel();
            s.disarm_delete();
            s.clear_recordings_pressed("Click Clear again");
        });
    }
    {
        let screen = screen.clone();
        window.on_recordings_select(move |i| screen.borrow_mut().select_recording(i as usize));
    }
    {
        let screen = screen.clone();
        window.on_recordings_select_tab(move |i| screen.borrow_mut().recordings_tab(i == 1));
    }
    {
        let screen = screen.clone();
        window.on_recordings_play(move |i| {
            let mut s = screen.borrow_mut();
            s.select_recording(i as usize);
            s.play_recording();
        });
    }
    {
        let screen = screen.clone();
        window.on_clear_history(move || screen.borrow_mut().clear_pressed("Click Clear again"));
    }
    {
        let screen = screen.clone();
        window.on_toggle_order(move || screen.borrow_mut().toggle_order());
    }
    {
        let screen = screen.clone();
        window.on_open_manage(move || screen.borrow_mut().open_manage());
    }
    {
        let screen = screen.clone();
        window.on_close_manage(move || screen.borrow_mut().close_manage());
    }
    {
        let screen = screen.clone();
        window.on_manage_select(move |i| screen.borrow_mut().manage_select(i as usize));
    }
    {
        let screen = screen.clone();
        window.on_manage_select_kind(move |i| {
            if let Some(&kind) = Kind::ALL.get(i as usize) {
                screen.borrow_mut().manage_kind(kind);
            }
        });
    }
    {
        let screen = screen.clone();
        window.on_manage_toggle(move |i| screen.borrow_mut().manage_toggle(i as usize));
    }
    {
        let screen = screen.clone();
        window.on_manage_rule(move || screen.borrow_mut().manage_rule());
    }
    {
        let screen = screen.clone();
        window.on_remove_rule(move |i| screen.borrow_mut().remove_rule(i as usize));
    }
    {
        let screen = screen.clone();
        window.on_manage_name_rule(move || screen.borrow_mut().manage_name_rule());
    }
    {
        let screen = screen.clone();
        window.on_remove_name_rule(move |i| screen.borrow_mut().remove_name_rule(i as usize));
    }
    {
        let screen = screen.clone();
        window.on_manage_kept_tag(move || screen.borrow_mut().manage_kept_tag());
    }
    {
        let screen = screen.clone();
        window.on_remove_kept_tag(move |i| screen.borrow_mut().remove_kept_tag(i as usize));
    }
    {
        let screen = screen.clone();
        window.on_select_kind(move |i| {
            let mut s = screen.borrow_mut();
            if let Some(&kind) = Kind::ALL.get(i as usize)
                && let Err(e) = s.set_kind(kind)
            {
                log::error!("{e}");
                s.set_error(&e);
            }
        });
    }
    {
        let screen = screen.clone();
        window.on_select_category(move |i| {
            let mut s = screen.borrow_mut();
            if let Err(e) = s.set_category(i as usize) {
                log::error!("{e}");
                s.set_error(&e);
            }
        });
    }

    #[cfg(feature = "devtools")]
    {
        let keys = match opts.keys.as_deref() {
            Some(seq) => crate::devtools::parse_keys(seq)?,
            None => Vec::new(),
        };
        // The frame probe: selects the next row, wrapping at the end, or
        // with `VIDICON_FPS=time` in the grid, moves the time a minute,
        // turning at the kept window's ends.
        let step = std::env::var_os("VIDICON_FPS").map(|v| {
            let screen = screen.clone();
            let time = v == "time";
            let ahead = Cell::new(true);
            Rc::new(move || {
                let mut s = screen.borrow_mut();
                if s.guide_open && time {
                    let view = s.guide_view;
                    let d = if ahead.get() { 60 } else { -60 };
                    let now = import::unix_now();
                    let next = guide_clamp(view + d, now, s.guide_back);
                    if next == view {
                        ahead.set(!ahead.get());
                    }
                    s.set_guide_view(next);
                } else if s.guide_open {
                    let next = (s.guide_selected + 1) % s.guide.row_count().max(1);
                    s.guide_select(next);
                } else {
                    let next = (s.selected + 1) % s.channels.row_count().max(1);
                    s.select(next);
                }
            }) as Rc<dyn Fn()>
        });
        let screen = screen.clone();
        let suppress_play = !opts.play;
        let press = move |text: &str, ctrl| {
            let mut s = screen.borrow_mut();
            s.replaying = suppress_play;
            if !s.handle_key(text, ctrl) {
                log::warn!("--keys: {text:?} is not bound");
            }
            s.replaying = false;
        };
        crate::devtools::install(&window, opts.started, opts.screenshot, keys, press, step);
    }
    #[cfg(not(feature = "devtools"))]
    let Options {} = opts;

    let result = window.run().map_err(|e| e.to_string());
    // Where a movie or episode was, written here: the writes task is not
    // waited for.
    let save = screen.borrow_mut().progress_save(false);
    if let Some(save) = save {
        let conn = writer.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = save.write(&conn) {
            log::error!("saving where it stopped: {e}");
        }
    }
    // Quitting asks nothing: recordings stop and are kept as done.
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    if recorder_tx.send(recorder::Command::Quit(done_tx)).is_ok() {
        let _ = rt.block_on(async { tokio::time::timeout(Duration::from_secs(5), done_rx).await });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::Action::*;
    use super::action;
    use crate::player::Control::*;
    use slint::platform::Key;

    fn key(k: Key) -> String {
        char::from(k).to_string()
    }

    /// The next episode follows into the next season, and there is none
    /// after the last or before the first (VOD ticket 08).
    #[test]
    fn next_episode_crosses_seasons_and_stops_at_the_ends() {
        let ep = |season, num, stream_id| crate::store::Episode {
            season,
            num: Some(num),
            stream_id,
            title: format!("Harbour Lights - S{season:02}E{num:02}"),
            extension: Some("mkv".into()),
            length: Some(2900),
        };
        let eps = [ep(1, 1, 11), ep(1, 2, 12), ep(2, 1, 21)];
        let along = |id, d| super::neighbour(&eps, id, d).map(|e| e.stream_id);
        assert_eq!(along(11, 1), Some(12));
        assert_eq!(
            along(12, 1),
            Some(21),
            "a season's end: the next one's first"
        );
        assert_eq!(along(21, 1), None, "after the last");
        assert_eq!(along(21, -1), Some(12));
        assert_eq!(along(11, -1), None, "before the first");
        assert_eq!(along(99, 1), None, "no longer listed");
    }

    #[test]
    fn rec_indicator_counts_the_seconds() {
        let s = std::time::Duration::from_secs;
        assert_eq!(super::rec_elapsed(s(0)), "● REC 0:00");
        assert_eq!(super::rec_elapsed(s(59)), "● REC 0:59");
        assert_eq!(super::rec_elapsed(s(12 * 60 + 4)), "● REC 12:04");
        assert_eq!(super::rec_elapsed(s(3600 + 5 * 60 + 30)), "● REC 1:05:30");
    }

    #[test]
    fn keys_map_per_spec() {
        assert_eq!(action("j", false), Some(Move(1)));
        assert_eq!(action(&key(Key::UpArrow), false), Some(Move(-1)));
        assert_eq!(action("d", true), Some(HalfPage(1)));
        assert_eq!(action("\u{15}", true), Some(HalfPage(-1)));
        assert_eq!(action(&key(Key::PageDown), false), Some(Page(1)));
        assert_eq!(action("G", false), Some(Bottom));
        assert_eq!(action(&key(Key::Home), false), Some(Top));
        assert_eq!(action("h", false), Some(Category(-1)));
        assert_eq!(action(&key(Key::RightArrow), false), Some(Category(1)));
        assert_eq!(action("\n", false), Some(Play));
        assert_eq!(action("r", false), Some(Refresh));
        assert_eq!(action("p", false), Some(Providers));
        assert_eq!(action("f", false), Some(Favourite));
        assert_eq!(action("J", false), Some(MoveFavourite(1)));
        assert_eq!(action("K", false), Some(MoveFavourite(-1)));
        assert_eq!(action("/", false), Some(Search));
        assert_eq!(action(&key(Key::Escape), false), Some(ClearFilter));
        assert_eq!(action("q", false), Some(Quit));
        assert_eq!(action("a", false), Some(ToggleOrder));
        assert_eq!(action("e", false), Some(Manage));
        assert_eq!(action("i", false), Some(Account));
        assert_eq!(action("R", false), Some(Record));
        assert_eq!(action("?", false), Some(Keys));
        assert_eq!(action(&key(Key::Space), false), Some(Control(Pause)));
        assert_eq!(action("s", false), Some(Control(Stop)));
        assert_eq!(action("m", false), Some(Control(Mute)));
        assert_eq!(action("F", false), Some(Control(Fullscreen)));
        assert_eq!(action("L", false), Some(Control(Live)));
        assert_eq!(action("-", false), Some(LastChannel));
        assert_eq!(action("o", false), Some(Alternates));
        assert_eq!(action("c", false), Some(ClearHistory));
        assert_eq!(action("t", false), Some(Guide));
        assert_eq!(action("n", false), Some(Jump(super::Jump::Now)));
        assert_eq!(action("<", false), Some(Jump(super::Jump::Day(-1))));
        assert_eq!(action(">", false), Some(Jump(super::Jump::Day(1))));
        assert_eq!(action("P", false), Some(Jump(super::Jump::Prime)));
        assert_eq!(action(&key(Key::Tab), false), Some(Cycle(1)));
        assert_eq!(action(&key(Key::Backtab), false), Some(Cycle(-1)));
    }

    #[test]
    fn favourites_and_recently_played_ignore_the_order() {
        use super::Category as C;
        use crate::store::{Kind, Order, TitleView, View};
        assert!(matches!(
            C::Favourites.view(Order::Az),
            Some(View::Favourites)
        ));
        assert!(matches!(C::Recent.view(Order::Az), Some(View::Recent)));
        assert!(matches!(
            C::All(Kind::Live).view(Order::Az),
            Some(View::All(Order::Az))
        ));
        assert!(C::All(Kind::Movie).view(Order::Az).is_none());
        assert!(matches!(
            C::All(Kind::Movie).title_view(Order::Az),
            Some(TitleView::All(Order::Az))
        ));
        assert!(matches!(
            C::Added(Kind::Series).title_view(Order::Az),
            Some(TitleView::Added)
        ));
    }

    #[test]
    fn playing_heading_shows_behind_live_in_whole_minutes() {
        use super::playing_heading;
        use std::time::Duration;
        let s = Duration::from_secs;
        assert_eq!(playing_heading(false, s(0)), "NOW PLAYING");
        assert_eq!(playing_heading(true, s(0)), "PAUSED");
        assert_eq!(playing_heading(false, s(59)), "BEHIND LIVE · <1 min");
        assert_eq!(playing_heading(false, s(179)), "BEHIND LIVE · 2 min");
        assert_eq!(
            playing_heading(true, s(59)),
            "PAUSED",
            "every pause is behind; only a minute counts"
        );
        assert_eq!(playing_heading(true, s(120)), "PAUSED · 2 min behind");
    }

    #[test]
    fn initials_use_ascii_letters_after_the_prefix() {
        use super::initials;
        assert_eq!(initials("4K: NORTHWIND SPORT 1 ᵁᴴᴰ"), "NS");
        assert_eq!(initials("UK: HARBOUR ONE LONDON 4K ◉"), "HO");
        assert_eq!(initials("US| (EVENT) night"), "EN");
        assert_eq!(initials("VANTA 2"), "V");
        // Nothing after the prefix: the whole name.
        assert_eq!(initials("NOW 80s:"), "N");
        // No word starts with a letter: the first letter anywhere.
        assert_eq!(initials("=== 4K ᵁᴴᴰ ==="), "K");
        assert_eq!(initials("ᴮᴮᶜ 1"), "");
    }

    #[test]
    fn search_mode_types_letters_and_navigates_with_special_keys() {
        use super::SearchKey::*;
        use super::search_key;
        for typed in ["j", "k", "q", "/", " ", "é", "ᴴᴰ"] {
            assert_eq!(
                search_key(typed, false),
                Some(Type(typed.into())),
                "{typed}"
            );
        }
        assert_eq!(search_key(&key(Key::DownArrow), false), Some(Nav(Move(1))));
        assert_eq!(search_key(&key(Key::PageUp), false), Some(Nav(Page(-1))));
        assert_eq!(search_key(&key(Key::End), false), Some(Nav(Bottom)));
        assert_eq!(
            search_key(&key(Key::LeftArrow), false),
            Some(Nav(Category(-1)))
        );
        assert_eq!(search_key("d", true), Some(Nav(HalfPage(1))));
        assert_eq!(search_key(&key(Key::Return), false), Some(Keep));
        assert_eq!(search_key("\n", false), Some(Keep));
        assert_eq!(search_key(&key(Key::Escape), false), Some(Clear));
        assert_eq!(search_key(&key(Key::Backspace), false), Some(Backspace));
        assert_eq!(search_key(&key(Key::Tab), false), None);
        assert_eq!(search_key(&key(Key::F5), false), None);
        assert_eq!(search_key("", false), None);
    }

    #[test]
    fn typed_numbers_play_after_idle() {
        use super::{NUMBER_IDLE, NumberEntry};
        use std::time::{Duration, Instant};
        let ms = Duration::from_millis;
        let t0 = Instant::now();
        let mut n = NumberEntry::default();
        assert_eq!(n.take_due(t0 + ms(5000)), None, "nothing typed");
        assert_eq!(n.push(1, t0), "1");
        // Each digit restarts the wait.
        assert_eq!(n.push(0, t0 + ms(1400)), "10");
        assert_eq!(n.push(4, t0 + ms(2800)), "104");
        assert_eq!(n.take_due(t0 + ms(2800) + NUMBER_IDLE - ms(1)), None);
        assert_eq!(n.take_due(t0 + ms(2800) + NUMBER_IDLE), Some(104));
        assert_eq!(n.take_due(t0 + ms(9000)), None, "taken once");

        // A digit after a missed deadline starts a new number.
        n.push(5, t0);
        assert_eq!(n.push(7, t0 + ms(1600)), "7");
        // Runaway typing is capped rather than overflowing.
        let mut n = NumberEntry::default();
        for i in 0..20 {
            n.push(9, t0 + ms(i));
        }
        assert!(n.take_due(t0 + ms(20) + NUMBER_IDLE).is_some());
    }

    #[test]
    fn guide_blocks_are_placed_from_the_origin() {
        use super::{GUIDE_SPAN, guide_blocks, guide_start};
        use crate::store::Programme;
        assert_eq!(guide_start(36_000 + 1799), 36_000);
        assert_eq!(guide_start(36_000 + 1800), 37_800);
        assert_eq!(GUIDE_SPAN, 3 * 3600);
        // Half hours from the origin, so x and w come out in sixths.
        let p = |from: i64, to: i64| Programme {
            start: 36_000 + from * 1800,
            stop: 36_000 + to * 1800,
            title: format!("{from}"),
            archive: None,
        };
        let blocks = guide_blocks(
            &[
                // Began before the origin: placed before it, not cut.
                p(-2, 1),
                // Overlapped by the next: cut where it starts.
                p(1, 4),
                p(2, 4),
                // Long, past any span.
                p(4, 20),
            ],
            36_000,
            36_000 + 2 * 1800 + 60,
            &[],
            0,
        );
        let got: Vec<(String, f32, f32, bool, i32)> = blocks
            .iter()
            .map(|b| (b.title.to_string(), b.x * 6.0, b.w * 6.0, b.airing, b.start))
            .collect();
        assert_eq!(
            got,
            [
                ("-2".into(), -2.0, 3.0, false, -3600),
                ("1".into(), 1.0, 1.0, false, 1800),
                ("2".into(), 2.0, 2.0, true, 3600),
                ("4".into(), 4.0, 16.0, false, 7200),
            ]
        );
        assert!(guide_blocks(&[], 36_000, 36_000, &[], 0).is_empty());
    }

    /// A recording of Football Tonight on Northwind Sport 1, `id` and from
    /// `start`, in `state`.
    fn rec(id: i64, start: i64, state: crate::store::RecState) -> crate::store::Recording {
        crate::store::Recording {
            id,
            provider_id: "p".into(),
            stream_id: 1,
            channel: "Northwind Sport 1".into(),
            title: Some("Football Tonight".into()),
            start,
            stop: start + 3600,
            padded_start: start - 120,
            padded_stop: start + 3900,
            state,
            file: None,
            note: None,
            trashed: false,
        }
    }

    #[test]
    fn the_recordings_view_groups_and_orders_them() {
        use super::{RecLine, recording_lines};
        use crate::store::RecState::*;
        let h = 3600;
        let rows = vec![
            rec(1, 10 * h, Done),
            rec(2, 30 * h, Scheduled),
            rec(3, 20 * h, Scheduled),
            rec(4, 12 * h, Failed),
            rec(5, 15 * h, Recording),
            rec(6, 11 * h, Missed),
            rec(7, 14 * h, Recording),
            rec(8, 13 * h, Cancelled),
            // The same start: the later scheduled first among finished.
            rec(9, 10 * h, Done),
        ];
        let got: Vec<String> = recording_lines(rows)
            .iter()
            .map(|l| match l {
                RecLine::Heading(h) => h.to_string(),
                RecLine::Rec(r) => r.id.to_string(),
                RecLine::Dl(d) => d.id.to_string(),
            })
            .collect();
        assert_eq!(
            got,
            [
                "NOW RECORDING",
                "7",
                "5",
                "SCHEDULED",
                "3",
                "2",
                "HISTORY",
                "8",
                "4",
                "6",
                "9",
                "1"
            ]
        );
        // Empty groups are left out, and nothing at all is nothing.
        let got = recording_lines(vec![rec(1, 0, Missed)]);
        assert_eq!(got[0], RecLine::Heading("HISTORY"));
        assert_eq!(got.len(), 2);
        assert!(recording_lines(Vec::new()).is_empty());
    }

    #[test]
    fn clearing_says_whether_files_are_kept() {
        use super::cleared_message;
        // A cancelled schedule never had a file.
        assert_eq!(
            cleared_message(1, 0, "recording"),
            "Cleared 1 recording from the list"
        );
        assert_eq!(
            cleared_message(1, 1, "recording"),
            "Cleared 1 recording from the list; its file is kept"
        );
        assert_eq!(
            cleared_message(3, 3, "recording"),
            "Cleared 3 recordings from the list; their files are kept"
        );
        assert_eq!(
            cleared_message(3, 1, "recording"),
            "Cleared 3 recordings from the list; the 1 file is kept"
        );
        assert_eq!(
            cleared_message(4, 2, "recording"),
            "Cleared 4 recordings from the list; the 2 files are kept"
        );
    }

    #[test]
    fn deleting_says_what_happened_to_the_file() {
        use super::deleted_message;
        use crate::recorder::Deleted::*;
        use crate::store::RecState::Done;
        let dir = std::path::Path::new("/videos/Vidicon");
        let mut r = rec(1, 0, Done);
        r.file = Some("/videos/Vidicon/a.ts".into());
        let say = |r: &crate::store::Recording, d| deleted_message(r, &d, dir);
        assert_eq!(say(&r, Trashed), ("Moved a.ts to the trash".into(), false));
        assert_eq!(
            say(&r, NoFile).0,
            "Removed Football Tonight on Northwind Sport 1 from the list; its file was already gone"
        );
        assert_eq!(
            say(&r, Elsewhere).0,
            "a.ts is not in /videos/Vidicon, so d leaves it alone"
        );
        assert_eq!(
            say(&r, Failed("Permission denied (os error 13)".into())),
            (
                "Can't move a.ts to the trash: Permission denied (os error 13)".into(),
                true
            )
        );
        r.file = None;
        assert!(say(&r, NoFile).0.ends_with("; it had no file"));
    }

    #[test]
    fn the_recordings_view_moves_over_headings() {
        use super::{RecView, recording_lines};
        use crate::store::RecState::*;
        let mut v = RecView {
            lines: recording_lines(vec![rec(1, 0, Recording), rec(2, 9000, Scheduled)]),
            selected: 0,
            downloads: false,
        };
        // Opened on the heading: the first recording.
        v.step(0);
        assert_eq!(v.selected().map(|r| r.id), Some(1));
        v.step(1);
        assert_eq!((v.selected, v.selected().map(|r| r.id)), (3, Some(2)));
        v.step(1);
        assert_eq!(v.selected, 3, "stops at the end");
        v.step(-5);
        assert_eq!(v.selected, 1, "and at the start");
        let mut empty = RecView::default();
        empty.step(1);
        assert_eq!(empty.selected(), None);
    }

    #[test]
    fn recording_rows_say_their_state_or_size() {
        use super::{RecTone, file_size, recording_item};
        use crate::store::RecState::*;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.ts");
        std::fs::write(&file, vec![0; 2_600_000]).unwrap();
        let now = 1_000_000_000;
        let with = |state, file: Option<&std::path::Path>, note: Option<&str>| {
            let mut r = rec(1, now - 7200, state);
            r.file = file.map(|f| f.display().to_string());
            r.note = note.map(Into::into);
            let item = recording_item(&r, now);
            (item.state.to_string(), item.tone, item.note.to_string())
        };
        assert_eq!(
            with(Recording, Some(&file), None),
            ("● Recording".into(), RecTone::Recording, "".into())
        );
        assert_eq!(with(Scheduled, None, None).0, "Scheduled");
        assert_eq!(
            with(Done, Some(&file), Some("cut short: the app quit")),
            (
                "3 MB".into(),
                RecTone::None,
                "cut short: the app quit".into()
            )
        );
        let gone = dir.path().join("gone.ts");
        assert_eq!(
            with(Done, Some(&gone), None),
            ("File missing".into(), RecTone::Warn, "".into())
        );
        assert_eq!(
            with(Failed, None, Some("404 Not Found")),
            ("Failed".into(), RecTone::Warn, "404 Not Found".into())
        );
        assert_eq!(
            with(Missed, None, None),
            (
                "Missed".into(),
                RecTone::Warn,
                "the app was not running".into()
            )
        );
        assert_eq!(
            with(Cancelled, Some(&file), None),
            ("Cancelled · 3 MB".into(), RecTone::Dim, "".into())
        );
        assert_eq!(with(Cancelled, None, None).0, "Cancelled");
        assert_eq!(file_size(640_000), "640 KB");
        assert_eq!(file_size(1_234_000_000), "1.2 GB");

        // Programme, then channel and times; what plays without a guide is
        // titled by its channel, and open-ended until it stops.
        let r = rec(1, now - 7200, Done);
        let item = recording_item(&r, now);
        assert_eq!(item.title, "Football Tonight");
        assert!(item.sub.starts_with("Northwind Sport 1 · "), "{}", item.sub);
        let mut r = rec(1, now - 60, Recording);
        (r.title, r.stop) = (None, r.start);
        let item = recording_item(&r, now);
        assert_eq!(item.title, "Northwind Sport 1");
        assert!(item.sub.contains(" from "), "{}", item.sub);
    }

    #[test]
    fn record_marks_come_from_the_stored_states() {
        use super::{RecMark, guide_blocks};
        use crate::store::{Programme, RecState};
        let p = |start: i64| Programme {
            start,
            stop: start + 1800,
            title: format!("{start}"),
            archive: None,
        };
        let progs = [p(0), p(1800), p(3600), p(5400)];
        // Starts as scheduled: one recording, one scheduled, one for a
        // programme not in the row.
        let marks = [
            (1800, RecState::Recording),
            (5400, RecState::Scheduled),
            (9000, RecState::Scheduled),
        ];
        let got: Vec<RecMark> = guide_blocks(&progs, 0, 2000, &marks, 0)
            .iter()
            .map(|b| b.rec)
            .collect();
        assert_eq!(
            got,
            [
                RecMark::None,
                RecMark::Recording,
                RecMark::None,
                RecMark::Scheduled
            ]
        );
    }

    #[test]
    fn r_in_the_grid_records_future_and_airing_programmes_only() {
        use super::{Slot, recordable};
        use crate::store::Programme;
        let now = 10_000;
        let cell = |from: i64, to: i64, programme: bool| Slot {
            from,
            to,
            programme: programme.then(|| Programme {
                start: from,
                stop: to,
                title: "Football Tonight".into(),
                archive: None,
            }),
        };
        // Future and airing: the programme, which the recorder schedules
        // (an airing one starting at once, recorder tests).
        assert_eq!(
            recordable(cell(12_000, 14_000, true), now).unwrap().start,
            12_000
        );
        assert_eq!(
            recordable(cell(9_000, 11_000, true), now).unwrap().stop,
            11_000
        );
        // Ended, even by a second, and a gap: why not.
        assert_eq!(
            recordable(cell(8_000, now, true), now),
            Err("That programme has ended")
        );
        assert!(
            recordable(cell(12_000, 14_000, false), now)
                .unwrap_err()
                .contains("no programme")
        );
    }

    /// Re-reading a row changes only the blocks that differ, so Slint keeps
    /// the rest (ticket 06).
    #[test]
    fn a_reread_row_keeps_the_blocks_it_shares() {
        use super::{GuideBlock, sync_blocks};
        use slint::{Model, VecModel};
        let block = |start: i32, airing: bool| GuideBlock {
            title: format!("{start}").into(),
            start,
            airing,
            ..Default::default()
        };
        let starts = |m: &VecModel<GuideBlock>| -> Vec<i32> { m.iter().map(|b| b.start).collect() };
        let model = VecModel::from(vec![block(0, false), block(10, true), block(20, false)]);
        // A later page: the first goes, two come, one changes.
        sync_blocks(
            &model,
            vec![
                block(10, false),
                block(20, false),
                block(30, false),
                block(40, false),
            ],
        );
        assert_eq!(starts(&model), [10, 20, 30, 40]);
        assert!(!model.row_data(0).unwrap().airing);
        // An earlier page, and one in the middle gone.
        sync_blocks(
            &model,
            vec![block(-10, false), block(10, false), block(30, false)],
        );
        assert_eq!(starts(&model), [-10, 10, 30]);
        sync_blocks(&model, vec![]);
        assert_eq!(model.row_count(), 0);
    }

    /// A programme from `from` to `to`.
    fn prog(from: i64, to: i64) -> crate::store::Programme {
        crate::store::Programme {
            start: from,
            stop: to,
            title: format!("{from}"),
            archive: None,
        }
    }

    #[test]
    fn the_selection_is_placed_from_the_origin() {
        use super::{GUIDE_SPAN, Slot, guide_sel};
        let cell = |from, to, p: Option<i64>| Slot {
            from,
            to,
            programme: p.map(|s| prog(s, to)),
        };
        // A programme: matched by its start, even one begun before the origin.
        let sel = guide_sel(4, &cell(1000, 5000, Some(400)), 1000);
        assert_eq!((sel.row, sel.start, sel.gap), (4, -600, false));
        // A gap: where to draw it.
        let sel = guide_sel(
            2,
            &cell(1000 + GUIDE_SPAN, 1000 + 3 * GUIDE_SPAN, None),
            1000,
        );
        assert!(sel.gap);
        assert_eq!((sel.x, sel.w), (1.0, 2.0));
        assert_eq!(sel.start, i32::MIN);
    }

    #[test]
    fn guide_slots_fill_the_row() {
        use super::{Slot, guide_slots, slot_at};
        let cells = |progs: &[crate::store::Programme], lo, hi| -> Vec<(i64, i64, bool)> {
            guide_slots(progs, lo, hi)
                .iter()
                .map(|s: &Slot| (s.from, s.to, s.programme.is_some()))
                .collect()
        };
        assert_eq!(cells(&[], 0, 100), [(0, 100, false)]);
        assert_eq!(
            cells(
                &[
                    // Began before the window: cut to it.
                    prog(-50, 20),
                    // A gap, then one the next overlaps: cut where it starts.
                    prog(30, 60),
                    // Wholly inside the one before, so a gap follows it.
                    prog(40, 50),
                    prog(70, 90),
                ],
                0,
                100,
            ),
            [
                (0, 20, true),
                (20, 30, false),
                (30, 40, true),
                (40, 50, true),
                (50, 70, false),
                (70, 90, true),
                (90, 100, false),
            ]
        );
        // Past the window's end: cut, and nothing after.
        assert_eq!(cells(&[prog(0, 200)], 0, 100), [(0, 100, true)]);
        // A gap longer than the span: cut at its multiples.
        let span = super::GUIDE_SPAN;
        assert_eq!(
            cells(&[prog(3 * span + 600, 4 * span)], 600, 4 * span),
            [
                (600, span, false),
                (span, 2 * span, false),
                (2 * span, 3 * span, false),
                (3 * span, 3 * span + 600, false),
                (3 * span + 600, 4 * span, true),
            ]
        );
        // Up to the span, one gap, even across a multiple.
        assert_eq!(
            cells(&[prog(span + 600, span + 900)], 1200, span + 900),
            [(1200, span + 600, false), (span + 600, span + 900, true)]
        );
        let slots = guide_slots(&[prog(10, 20), prog(20, 30)], 0, 100);
        assert_eq!(slot_at(&slots, 0), 0);
        assert_eq!(slot_at(&slots, 10), 1);
        assert_eq!(slot_at(&slots, 19), 1);
        assert_eq!(slot_at(&slots, 20), 2);
        assert_eq!(slot_at(&slots, 99), 3);
        // Outside the row: its first or last cell.
        assert_eq!(slot_at(&slots, -5), 0);
        assert_eq!(slot_at(&slots, 500), 3);
    }

    /// `j`/`k` (SPEC 8.3): the cell on the next channel at the selected
    /// time, which is now while the selection airs, else its start.
    #[test]
    fn up_and_down_keep_to_the_selected_time() {
        use super::{guide_anchor, guide_slots, guide_time, slot_at};
        let now = 1_800_000_000;
        let (lo, hi) = super::guide_window(now, 3600);
        let row = |progs: &[crate::store::Programme]| guide_slots(progs, lo, hi);
        let a = row(&[prog(now - 3000, now + 600), prog(now + 600, now + 4200)]);
        let b = row(&[prog(now - 600, now + 1200), prog(now + 1200, now + 9000)]);
        // A gap where `a`'s second starts and over now.
        let c = row(&[prog(now - 1200, now - 300), prog(now + 900, now + 2000)]);

        // On the airing one: now, not its start an hour ago.
        let anchor = guide_anchor(&a[slot_at(&a, now)], now);
        assert_eq!(anchor, None);
        let t = guide_time(anchor, now, 3600);
        assert_eq!(b[slot_at(&b, t)].from, now - 600);
        let gap = &c[slot_at(&c, t)];
        assert_eq!(
            (gap.from, gap.to, gap.programme.is_none()),
            (now - 300, now + 900, true)
        );
        // Now follows the clock.
        let later = now + 1500;
        assert_eq!(
            b[slot_at(&b, guide_time(anchor, later, 3600))].from,
            now + 1200
        );

        // On one that has not started: its start, kept down the column.
        let next = &a[slot_at(&a, now) + 1];
        let anchor = guide_anchor(next, now);
        assert_eq!(anchor, Some(now + 600));
        let t = guide_time(anchor, now, 3600);
        assert_eq!(b[slot_at(&b, t)].from, now - 600);
        assert_eq!(c[slot_at(&c, t)].from, now - 300);
        // Moving through `b`'s earlier start does not drift the time.
        assert_eq!(a[slot_at(&a, t)].from, now + 600);

        // A past one: its start, not now.
        let past = &c[1];
        assert_eq!(guide_anchor(past, now), Some(now - 1200));
        // Older than the window, as time passes: its start.
        assert_eq!(guide_time(Some(lo - 500), now, 3600), lo);
        assert_eq!(guide_time(Some(hi + 500), now, 3600), hi - 1);
    }

    #[test]
    fn previous_and_next_programme_step_through_gaps() {
        use super::{guide_slots, slot_at};
        let slots = guide_slots(&[prog(10, 20), prog(30, 40)], 0, 50);
        let at = slot_at(&slots, 15);
        let step = |i: usize, d: isize| i.saturating_add_signed(d).min(slots.len() - 1);
        let from = |i: usize| (slots[i].from, slots[i].programme.is_some());
        assert_eq!(from(step(at, 1)), (20, false));
        assert_eq!(from(step(step(at, 1), 1)), (30, true));
        assert_eq!(from(step(at, -1)), (0, false));
        // At the window's ends: no further.
        assert_eq!(step(0, -1), 0);
        assert_eq!(step(slots.len() - 1, 1), slots.len() - 1);
    }

    #[test]
    fn a_drag_locks_to_the_direction_it_starts_in() {
        use super::{DragAxis::*, drag_axis};
        // Not yet: a click's jitter, or a slow start.
        assert_eq!(drag_axis(0.0, 0.0), None);
        assert_eq!(drag_axis(3.0, -4.0), None);
        assert_eq!(drag_axis(-7.9, 0.0), None);
        // Then the way it went further.
        assert_eq!(drag_axis(8.0, 0.0), Some(Horizontal));
        assert_eq!(drag_axis(-20.0, 9.0), Some(Horizontal));
        assert_eq!(drag_axis(0.0, -8.0), Some(Vertical));
        assert_eq!(drag_axis(5.0, 12.0), Some(Vertical));
        // A diagonal pans the list, as it always has.
        assert_eq!(drag_axis(6.0, -6.0), Some(Vertical));
    }

    #[test]
    fn rows_hold_a_page_around_the_span() {
        use super::{GUIDE_SPAN, guide_page, guide_start, guide_ticks};
        let half = guide_start(1_800_000_000);
        // Aligned or not, the span is inside the page.
        for view in [half, half + 1, half + 1799] {
            let (from, to) = guide_page(view);
            assert_eq!(from, half);
            assert!(view + GUIDE_SPAN <= to, "{view}");
        }
        // Ticks: the half hours inside the span, where they fall.
        let at = |view: i64| -> Vec<(i64, f32)> {
            guide_ticks(view)
                .into_iter()
                .map(|(t, x)| (t - half, x * 6.0))
                .collect()
        };
        assert_eq!(
            at(half),
            [
                (0, 0.0),
                (1800, 1.0),
                (3600, 2.0),
                (5400, 3.0),
                (7200, 4.0),
                (9000, 5.0)
            ]
        );
        // A quarter hour on: the first tick is the next half hour.
        let ticks = at(half + 900);
        assert_eq!(ticks.len(), 6);
        assert_eq!(ticks[0], (1800, 0.5));
        assert_eq!(ticks[5], (10_800, 5.5));
    }

    #[test]
    fn the_span_follows_the_selection_only_as_needed() {
        use super::{GUIDE_SPAN, guide_follow, guide_start};
        let now = 1_800_000_000;
        let s = guide_start(now);
        // In view: stays.
        assert_eq!(guide_follow(s, s + 600, s + 2400, now, 3600), s);
        // Starts before the span: back to its half hour.
        assert_eq!(
            guide_follow(s + 3600, s + 2000, s + 4000, now, 3600),
            s + 1800
        );
        // Ends after it: on until its end shows.
        assert_eq!(guide_follow(s, s + 9000, s + 11_000, now, 3600), s + 1800);
        // Longer than the span: its start shows.
        assert_eq!(
            guide_follow(s, s + 7200, s + 7200 + 4 * 3600, now, 3600),
            s + 7200
        );
        // Not past the kept window's ends.
        assert_eq!(
            guide_follow(s, now - 7200, now - 3600, now, 3600),
            guide_start(now - 3600)
        );
        let end = guide_start(now + 3 * 86_400 + 1799) - GUIDE_SPAN;
        assert_eq!(
            guide_follow(end, now + 3 * 86_400, now + 4 * 86_400, now, 3600),
            end
        );
    }

    #[test]
    fn jumps_stop_at_the_kept_window() {
        use super::{GUIDE_SPAN, Jump, guide_jump, guide_start, guide_window};
        // A minute past a half hour.
        let now = guide_start(1_800_000_000) + 60;
        let (lo, hi) = guide_window(now, 3600);
        let s = guide_start(now);
        let shows = |(t, s): (i64, i64)| assert!((s..s + GUIDE_SPAN).contains(&t), "{t} {s}");
        // Now: back to the current half hour.
        assert_eq!(
            guide_jump(Jump::Now, now + 86_400, s + 86_400, now, 3600),
            (now, s)
        );
        // A day ahead and back, the span moved by the day.
        assert_eq!(
            guide_jump(Jump::Day(1), now + 600, s, now, 3600),
            (now + 86_400 + 600, s + 86_400)
        );
        assert_eq!(
            guide_jump(Jump::Day(-1), now + 86_400 + 600, s + 86_400, now, 3600),
            (now + 600, s)
        );
        // Back from today: the window's start, in view.
        let j = guide_jump(Jump::Day(-1), now, s, now, 3600);
        assert_eq!(j, (lo, guide_start(lo)));
        shows(j);
        // Near the end: a day ahead stops at it.
        let j = guide_jump(
            Jump::Day(1),
            now + 2 * 86_400 + 600,
            s + 2 * 86_400,
            now,
            3600,
        );
        assert_eq!(j.0, now + 3 * 86_400 - 1);
        assert_eq!(j.1, guide_start(hi + 1799) - GUIDE_SPAN);
        shows(j);
        // Just inside the end: a day ahead still stops at it.
        let j = guide_jump(Jump::Day(1), hi - 86_400 + 60, s + 2 * 86_400, now, 3600);
        assert_eq!(j.0, hi - 1);
        shows(j);
        // Just inside the start: a day back stops at it.
        let j = guide_jump(Jump::Day(-1), lo + 86_400 - 60, s, now, 3600);
        assert_eq!(j.0, lo);
        shows(j);
        // Prime time: in view, starting the span.
        let (t, start) = guide_jump(Jump::Prime, now, s, now, 3600);
        assert_eq!(start, guide_start(t));
        shows((t, start));
    }

    /// Catch-up (ticket 03): the grid reaches back as far as the longest
    /// archive in the category, so a day back from today stays a day back.
    #[test]
    fn the_grid_reaches_back_as_far_as_the_archive() {
        use super::{Jump, guide_back, guide_jump, guide_start, guide_window};
        assert_eq!(guide_back(0), 3600);
        assert_eq!(guide_back(3), 3 * 86_400);
        let now = guide_start(1_800_000_000) + 60;
        let s = guide_start(now);
        let back = guide_back(3);
        assert_eq!(guide_window(now, back).0, now - 3 * 86_400);
        assert_eq!(
            guide_jump(Jump::Day(-1), now + 600, s, now, back),
            (now - 86_400 + 600, s - 86_400)
        );
        // Three days back stops at the window's start.
        let j = guide_jump(
            Jump::Day(-1),
            now - 2 * 86_400 - 60,
            s - 2 * 86_400,
            now,
            back,
        );
        assert_eq!(j, (now - 3 * 86_400, guide_start(now - 3 * 86_400)));
    }

    #[test]
    fn past_programmes_play_from_the_archive_or_say_why_not() {
        use super::replayable;
        let now = 1_800_000_000;
        let three = 3 * 86_400;
        let r = |start: i64, stop: i64, days: i64| replayable(&prog(start, stop), stop, days, now);
        // Not ended: Enter plays live.
        assert_eq!(r(now - 600, now + 600, 3), None);
        // Ended, in the window.
        assert_eq!(r(now - 3600, now - 600, 3), Some(Ok(())));
        assert_eq!(r(now - 3600, now, 3), Some(Ok(())));
        // The window's edge: from exactly 3 days back, not a second more.
        assert_eq!(r(now - three, now - three + 3600, 3), Some(Ok(())));
        let older = Some(Err("Older than this channel's 3-day archive".into()));
        assert_eq!(r(now - three - 1, now - three + 3600, 3), older);
        // No archive.
        let none = Some(Err("No archive on this channel".into()));
        assert_eq!(r(now - 3600, now - 600, 0), none);

        // The channel's table decides over the days (catch-up ticket 06).
        let flagged = |start: i64, archive: bool, days: i64| {
            let p = crate::store::Programme {
                archive: Some(archive),
                ..prog(start, start + 3600)
            };
            replayable(&p, start + 3600, days, now)
        };
        assert_eq!(flagged(now - three - 1800, true, 3), Some(Ok(())));
        assert_eq!(
            flagged(now - 7200, false, 3),
            Some(Err("Not in this channel's archive".into()))
        );
        // A guide id shared with a channel that has an archive: this one
        // still has none.
        assert_eq!(flagged(now - 7200, true, 0), none);
        // Not ended is live whatever the table says.
        assert_eq!(flagged(now - 1800, false, 3), None);
    }

    #[test]
    fn rows_want_their_table_before_the_stored_past() {
        use super::needs_table;
        let now = 1_800_000_000;
        let day = 86_400;
        let page = |from: i64| (from, from + 3 * 3600 + 1800);
        // The guide starts a day back.
        let progs = [prog(now - 2 * 3600 - day, now - day + 3600)];
        let at = |from: i64, days: i64| needs_table(&progs[..0], page(from), days, now);
        // The page starts before what is stored.
        assert!(needs_table(
            &[prog(now - day, now)],
            page(now - day - 600),
            3,
            now
        ));
        // Inside it.
        assert!(!needs_table(&progs, page(now - day), 3, now));
        // Nothing stored on the page at all, as days back.
        assert!(at(now - 2 * day, 3));
        // No archive, past it, or not in the past.
        assert!(!at(now - 2 * day, 0));
        assert!(!at(now - 4 * day, 3));
        assert!(!at(now + 600, 3));
    }

    #[test]
    fn start_over_is_offered_or_says_why_not() {
        use super::{guide_slots, slot_at, start_over};
        let now = 1_800_000_000;
        let airing = prog(now - 1200, now + 600);
        // On a channel with an archive: the programme airing, from its start.
        assert_eq!(start_over(Some(airing.clone()), 3), Ok(airing.clone()));
        // No archive, guide or not.
        let none = Err("No archive on this channel");
        assert_eq!(start_over(Some(airing.clone()), 0), none);
        assert_eq!(start_over(None, 0), none);
        // No guide for the channel.
        let no_guide = Err("Nothing to start over: the guide has no programme airing");
        assert_eq!(start_over(None, 3), no_guide);
        // A gap in the guide airing now, as the grid selects it.
        let slots = guide_slots(
            &[prog(now - 3600, now - 600), prog(now + 600, now + 1800)],
            now - 3600,
            now + 3600,
        );
        let gap = &slots[slot_at(&slots, now)];
        assert_eq!((gap.from, gap.to), (now - 600, now + 600));
        assert_eq!(start_over(gap.programme.clone(), 3), no_guide);
    }

    #[test]
    fn past_blocks_are_marked_or_dimmed() {
        use super::{Programme, guide_blocks};
        let now = 1_800_000_000;
        let p = |start: i64, stop: i64| Programme {
            start,
            stop,
            title: String::new(),
            archive: None,
        };
        let progs = [
            p(now - 4 * 86_400, now - 4 * 86_400 + 3600),
            p(now - 7200, now - 3600),
            p(now - 3600, now + 600),
        ];
        let marks = |days| -> Vec<(bool, bool)> {
            guide_blocks(&progs, 0, now, &[], days)
                .iter()
                .map(|b| (b.archive, b.gone))
                .collect()
        };
        // Too old, kept, airing.
        assert_eq!(marks(3), [(false, true), (true, false), (false, false)]);
        // No archive: every past one dimmed.
        assert_eq!(marks(0), [(false, true), (false, true), (false, false)]);
    }

    /// mpv's title says what is replayed, and never holds the URL (SPEC
    /// 9.1).
    #[test]
    fn the_mpv_title_names_the_programme_replayed() {
        use super::media_title;
        assert_eq!(
            media_title(Some(101), "Harbour One", None),
            "101 Harbour One"
        );
        assert_eq!(
            media_title(Some(101), "Harbour One", Some("Evening News")),
            "101 Harbour One · Evening News"
        );
        assert_eq!(media_title(None, "Harbour One", None), "Harbour One");
        let c = crate::xtream::Client::new(crate::config::Provider {
            id: "p".into(),
            name: "P".into(),
            url: "http://h".into(),
            username: "user".into(),
            password: "pass".into(),
            epg: Vec::new(),
        });
        let url = c.timeshift_url(7, "2026-10-01:20-00", 50, "ts").unwrap();
        let title = media_title(Some(101), "Harbour One", Some("Evening News"));
        assert!(!title.contains(url.as_str()) && !title.contains("pass"));
    }

    /// A movie plays from `/movie/` under its own extension, titled with
    /// its name, never the URL (SPEC 9.1, VOD ticket 04).
    #[tokio::test]
    async fn a_movie_plays_from_its_url_under_its_name() {
        use crate::fake_xtream::{self, FakeXtream};
        use crate::store::{self, Kind, Order, TitleView};
        use crate::xtream::Client;
        let fake = FakeXtream::start(fake_xtream::active).await;
        let dir = tempfile::tempdir().unwrap();
        let writer = std::sync::Arc::new(std::sync::Mutex::new(
            store::open(&dir.path().join("v.db")).unwrap(),
        ));
        let client = Client::new(fake.provider());
        crate::import::movies(&client, &writer, &fake.provider())
            .await
            .unwrap();
        let row = {
            let conn = writer.lock().unwrap();
            let keys = store::title_keys(&conn, "p", Kind::Movie, TitleView::All(Order::Provider))
                .unwrap();
            store::title(&conn, Kind::Movie, keys[0]).unwrap().unwrap()
        };
        let (url, title) = super::movie_request(&client, &row).unwrap();
        assert_eq!(title, "The Quiet Year");
        assert_eq!(
            url.as_str(),
            format!("{}/movie/user/pass/20.mkv", fake.url())
        );
        let body = crate::xtream::http().get(url.clone()).send().await.unwrap();
        assert_eq!(body.text().await.unwrap(), "movie");
        assert_eq!(fake.targets().last().unwrap(), "/movie/user/pass/20.mkv");
        for secret in [url.as_str(), "user", "pass", fake.url()] {
            assert!(!title.contains(secret), "{secret}");
        }

        let none = super::TitleRow {
            extension: None,
            ..row
        };
        assert!(super::movie_request(&client, &none).is_err());
    }

    /// An episode plays from `/series/` under its own id and extension,
    /// titled with the series, `S1 E2` and its name, never the URL (SPEC
    /// 9.1, VOD ticket 05).
    #[tokio::test]
    async fn an_episode_plays_from_its_url_under_the_series_and_its_name() {
        use crate::fake_xtream::{self, FakeXtream};
        use crate::xtream::Client;
        let fake = FakeXtream::start(fake_xtream::active).await;
        let client = Client::new(fake.provider());
        let body = client.series_info(10).await.unwrap();
        let info = crate::info::parse_series(&body).unwrap();
        let episode = info.episodes.iter().find(|e| e.num == Some(2)).unwrap();
        let (url, title, line) = super::episode_request(&client, "Harbour Watch", episode).unwrap();
        assert_eq!(title, "Harbour Watch · S1 E2 · Low Water");
        assert_eq!(line, "S1 E2  ·  Low Water");
        assert_eq!(
            url.as_str(),
            format!("{}/series/user/pass/31.mp4", fake.url())
        );
        let body = crate::xtream::http().get(url.clone()).send().await.unwrap();
        assert_eq!(body.text().await.unwrap(), "episode");
        assert_eq!(fake.targets().last().unwrap(), "/series/user/pass/31.mp4");
        for secret in [url.as_str(), "user", "pass", fake.url()] {
            assert!(!title.contains(secret), "{secret}");
        }

        let none = super::Episode {
            extension: None,
            ..episode.clone()
        };
        assert!(super::episode_request(&client, "Harbour Watch", &none).is_err());
    }

    /// An episode's name is what follows the provider's `S01E02`, or its
    /// whole title without one (SPEC 6.1).
    #[test]
    fn episode_names_drop_the_series_and_its_mark() {
        use super::{Episode, episode_item, episode_name, store};
        let names = [
            "Harbour Watch - S01E02 - Low Water",
            "Harbour Watch S1E12: Flares",
            "s02e04 The Long Tide",
            "Harbour Watch - S01E02",
            "The Night Ferry",
            "Glass Houses2E1 - Sense",
            "Northwind S01E02x",
        ]
        .map(episode_name);
        assert_eq!(
            names,
            [
                "Low Water",
                "Flares",
                "The Long Tide",
                "",
                "The Night Ferry",
                "Glass Houses2E1 - Sense",
                "Northwind S01E02x",
            ]
        );
        let bare = Episode {
            season: 1,
            num: Some(2),
            title: "Harbour Watch - S01E02".into(),
            length: Some(2712),
            ..Default::default()
        };
        let item = episode_item(&bare, None, None);
        assert_eq!(
            (item.num.as_str(), item.title.as_str(), item.length.as_str()),
            ("2", "Episode 2", "46 min")
        );
        assert!(!item.watched && !item.has_progress);
        // Part way, then watched (VOD ticket 07).
        let mut p = store::Progress {
            pos: 678,
            length: Some(2712),
            watched: false,
        };
        let item = episode_item(&bare, Some(&p), None);
        assert!(!item.watched && item.has_progress && item.progress == 0.25);
        p.pos = 0;
        p.watched = true;
        let item = episode_item(&bare, Some(&p), None);
        assert!(item.watched && !item.has_progress);
    }

    /// The card shows mpv's position, and its length once known.
    #[test]
    fn movie_card_is_mpvs_position_and_length() {
        use super::{MoviePlaying, MpvState, movie_playing};
        let mut m = MoviePlaying {
            name: "The Quiet Year".into(),
            meta: String::new(),
            pos: 754.6,
            length: None,
            provider_id: "p".into(),
            item: super::Watchable::Movie(1),
            play: 1,
            heard: true,
            saved: 0.0,
            done: false,
        };
        let mpv = MpvState::default();
        let card = movie_playing(&m, &mpv);
        assert!(card.visible && card.movie && !card.behind && !card.has_guide);
        assert_eq!(
            (
                card.heading.as_str(),
                card.title.as_str(),
                card.span.as_str()
            ),
            ("NOW PLAYING", "Movie", "0:12:34")
        );
        assert_eq!(card.left, "");
        m.length = Some(5594.2);
        m.meta = "2019  ·  ★ 7.1".into();
        let paused = MpvState {
            paused: true,
            ..Default::default()
        };
        let card = movie_playing(&m, &paused);
        assert_eq!(
            (
                card.heading.as_str(),
                card.span.as_str(),
                card.left.as_str()
            ),
            ("PAUSED", "0:12:34 / 1:33:14", "1 h 21 min left")
        );
        assert_eq!(card.title, "2019  ·  ★ 7.1");
        assert!(card.has_guide && (card.progress - 754.0 / 5594.0).abs() < 1e-6);
    }

    /// The card's position comes from the guide's times and `time-pos`.
    #[test]
    fn catch_up_position_is_in_the_programme() {
        use super::{CatchUp, Programme, catch_up_heading};
        let mut c = CatchUp {
            programme: Programme {
                start: 1030,
                stop: 1030 + 3600,
                title: String::new(),
                archive: None,
            },
            // The archive starts at the minute below.
            from: 1020,
            pos: 0.0,
        };
        assert_eq!(c.elapsed(), 0);
        c.pos = 610.4;
        assert_eq!(c.elapsed(), 600);
        // Into the padding past its stop: the end.
        c.pos = 4000.0;
        assert_eq!(c.elapsed(), 3600);
        assert_eq!(catch_up_heading(false), "CATCH-UP");
        assert_eq!(catch_up_heading(true), "PAUSED · CATCH-UP");
    }

    #[test]
    fn prime_time_is_tonight_or_tomorrow() {
        use super::{local_tm, prime_time};
        // 19:59 and 20:00 local, some day.
        let mut tm = local_tm(1_800_000_000);
        (tm.tm_hour, tm.tm_min, tm.tm_sec, tm.tm_isdst) = (19, 59, 0, -1);
        let before = unsafe { libc::mktime(&mut tm) as i64 };
        let at_eight = before + 60;
        assert_eq!(prime_time(before), at_eight);
        // Once 20:00 has come, tomorrow's.
        let next = prime_time(at_eight);
        let tm = local_tm(next);
        assert_eq!((tm.tm_hour, tm.tm_min, tm.tm_sec), (20, 0, 0));
        assert!((23 * 3600..=25 * 3600).contains(&(next - at_eight)));
        assert_eq!(prime_time(at_eight + 3 * 3600), next);
    }

    #[test]
    fn detail_strip_times() {
        use super::{duration, guide_times};
        assert_eq!(duration(60), "1 min");
        assert_eq!(duration(61), "2 min");
        assert_eq!(duration(3600), "1 h");
        assert_eq!(duration(3900), "1 h 5 min");
        assert_eq!(duration(86_400), "1 day");
        assert_eq!(duration(86_400 + 7200 + 60), "1 day 2 h");
        assert_eq!(duration(2 * 86_400), "2 days");
        // Local noon, so an hour or two either way is the same day.
        let mut tm = super::local_tm(1_800_000_000);
        (tm.tm_hour, tm.tm_min, tm.tm_sec, tm.tm_isdst) = (12, 0, 0, -1);
        let now = unsafe { libc::mktime(&mut tm) as i64 };
        let at = |from: i64, to: i64| guide_times(&prog(now + from, now + to), now);
        let times = |from: i64, to: i64| {
            format!("{} – {}", super::clock(now + from), super::clock(now + to))
        };
        assert_eq!(
            at(-600, 1800),
            format!("{} · 30 min left", times(-600, 1800))
        );
        assert_eq!(
            at(3600, 5400),
            format!("{} · starts in 1 h", times(3600, 5400))
        );
        assert_eq!(
            at(-3600, -1200),
            format!("{} · ended 20 min ago", times(-3600, -1200))
        );
        assert!(at(86_400, 90_000).starts_with("Tomorrow · "));
        let far = at(2 * 86_400, 2 * 86_400 + 600);
        assert!(far.ends_with("starts in 2 days"), "{far}");
        assert!(!far.starts_with("Tomorrow"), "{far}");
    }

    /// Opens for `r`, not for a background refresh, and for a background
    /// failure; dismissed, it stays closed while the refresh carries on.
    #[test]
    fn the_refresh_panel_opens_when_asked_or_on_a_failure() {
        use super::{PanelEvent, Update, refresh_panel};
        use crate::refresh::{Stage, StageState};
        let running = Update::Stages(vec![
            (
                Stage::Channels,
                StageState::Running {
                    mb: 3,
                    percent: None,
                },
            ),
            (Stage::Epg, StageState::Waiting),
        ]);
        let ok = Update::Idle {
            updated: Some(1),
            error: None,
            retry_at: None,
        };
        let failed = Update::Idle {
            updated: Some(1),
            error: Some("provider returned HTTP 404".into()),
            retry_at: None,
        };
        let on = |open, u: &Update| refresh_panel(open, PanelEvent::Update(u), true);

        // Background: the sweep line only.
        assert_eq!(on(false, &running), Some((false, false)));
        assert_eq!(on(false, &ok), Some((false, false)));
        let busy = Update::Busy("Updating guide…".into());
        assert_eq!(on(false, &busy), None);
        // `r`: opens, and closes by itself after a success.
        assert_eq!(
            refresh_panel(false, PanelEvent::Asked, true),
            Some((true, false))
        );
        assert_eq!(on(true, &running), Some((true, false)));
        assert_eq!(on(true, &ok), Some((true, true)));
        // A background failure opens it and it stays; one with no stages
        // (the launch's account check) has nothing to show.
        assert_eq!(on(false, &failed), Some((true, false)));
        assert_eq!(
            refresh_panel(false, PanelEvent::Update(&failed), false),
            Some((false, false))
        );
        // Dismissed mid-refresh: later stages and the success leave it
        // closed.
        assert_eq!(
            refresh_panel(true, PanelEvent::Dismissed, true),
            Some((false, false))
        );
        assert_eq!(on(false, &running), Some((false, false)));
        assert_eq!(on(false, &ok), Some((false, false)));
        // Another refresh while one is closing: it stays for it.
        assert_eq!(on(true, &running), Some((true, false)));
    }

    /// An empty database opens on the first-run screen; the channels
    /// landing hand over to the main screen and the refresh panel; a
    /// failure before then leaves it showing.
    #[test]
    fn the_first_run_screen_shows_until_channels_land() {
        use super::{Update, first_run, first_run_after};
        use crate::store;
        let dir = tempfile::tempdir().unwrap();
        let mut conn = store::open(&dir.path().join("v.db")).unwrap();
        assert!(first_run(&conn, "p").unwrap());

        let failed = Update::Idle {
            updated: None,
            error: Some("provider returned HTTP 404".into()),
            retry_at: None,
        };
        let busy = Update::Busy("Updating channels…".into());
        assert_eq!(first_run_after(true, &busy), (true, false));
        assert_eq!(first_run_after(true, &failed), (true, false));
        assert_eq!(first_run_after(true, &Update::Channels(3)), (false, true));
        // Later imports do not hand over again.
        assert_eq!(first_run_after(false, &Update::Channels(3)), (false, false));
        assert_eq!(first_run_after(false, &failed), (false, false));

        let tx = conn.transaction().unwrap();
        store::begin_channel_import(&tx, "p", "P", [("a", "News")]).unwrap();
        store::finish_channel_import(&tx, "p", 0).unwrap();
        tx.commit().unwrap();
        assert!(!first_run(&conn, "p").unwrap());
    }

    #[test]
    fn stage_rows_say_where_each_stage_is() {
        use super::{count_short, stage_items};
        use crate::refresh::{Stage, StageState};
        let stages = vec![
            (Stage::Channels, StageState::Done(58_312)),
            (
                Stage::Epg,
                StageState::Running {
                    mb: 30,
                    percent: Some(41),
                },
            ),
            (Stage::Movies, StageState::Backoff { retry_at: 1042 }),
            (
                Stage::Series,
                StageState::Running {
                    mb: 12,
                    percent: None,
                },
            ),
            (
                Stage::Series,
                StageState::Failed("Demo TV: provider returned HTTP 404".into()),
            ),
            (Stage::Series, StageState::Waiting),
        ];
        let rows: Vec<(String, String, String)> = stage_items(&stages, "Demo TV", 1000)
            .into_iter()
            .map(|r| (r.name.into(), r.state.into(), r.reason.into()))
            .collect();
        let row = |n: &str, s: &str, r: &str| (n.to_owned(), s.to_owned(), r.to_owned());
        assert_eq!(
            rows,
            [
                row("Channels", "✓ 58k", ""),
                row("EPG", "▸ 41 %", ""),
                row("Movies", "provider busy, retry in 0:42", ""),
                row("Series", "▸ 12 MB", ""),
                row("Series", "failed", "provider returned HTTP 404"),
                row("Series", "waiting", ""),
            ]
        );
        assert_eq!(count_short(640), "640");
        assert_eq!(count_short(1234), "1.2k");
        assert_eq!(count_short(9999), "10k");
    }

    #[test]
    fn unbound_keys_are_not_handled() {
        assert_eq!(action("x", false), None);
        assert_eq!(action("j", true), None);
        assert_eq!(action("", false), None);
        assert_eq!(action("jj", false), None);
    }

    /// The setup screen's rows (onboarding ticket 03): `Fix permissions`
    /// only for a file it may fix; its text names the file and the chmod.
    #[test]
    fn setup_offers_a_fix_only_when_it_may() {
        use super::{config::Setup, setup_items, setup_text};
        let labels = |setup: &Setup| -> Vec<String> {
            setup_items(setup)
                .iter()
                .map(|i| i.label.to_string())
                .collect()
        };
        let why = "/home/u/.config/vidicon/credentials.toml has mode 0644; it must not be readable by others.\nFix: chmod 600 /home/u/.config/vidicon/credentials.toml".to_string();
        let fixable = Setup::Loose {
            why: why.clone(),
            fix: true,
        };
        assert_eq!(labels(&fixable), ["Fix permissions", "Check again"]);
        let (_, text) = setup_text(&fixable, std::path::Path::new("/home/u"));
        assert!(
            text.starts_with(
                "~/.config/vidicon/credentials.toml has mode 0644; it must not be readable by others.\nFix: chmod 600 ~/.config/vidicon/credentials.toml\n\n"
            ),
            "{text}"
        );
        let not = Setup::Loose { why, fix: false };
        assert_eq!(labels(&not), ["Check again"]);
        assert_eq!(labels(&Setup::Invalid("bad".into())), ["Check again"]);
        let (title, text) = setup_text(&Setup::Empty, std::path::Path::new("/home/u"));
        assert_eq!((title, text.as_str()), ("Add a provider", ""));
    }

    /// The Providers section's paths (onboarding ticket 01): the list and
    /// its add row, then the form typed, masked, cut, checking and warned.
    #[test]
    fn provider_items_follow_the_form() {
        use super::{FormCheck, ProviderForm, RemoveConfirm, SettingKind, provider_items, store};
        let providers = [("Demo TV", "demo · Expires 9 Mar 2027".to_string())];
        let list = provider_items(&providers, None);
        let labels: Vec<String> = list.iter().map(|i| i.label.to_string()).collect();
        assert_eq!(labels, ["Demo TV", "Add provider…"]);
        assert!(list.iter().all(|i| i.kind == SettingKind::Row));

        let mut form = ProviderForm {
            values: [
                "Spare TV".into(),
                "http://127.0.0.1:47811".into(),
                "demo".into(),
                "hunter2".into(),
            ],
            cut: true,
            ..Default::default()
        };
        let items = provider_items(&providers, Some(&form));
        assert_eq!(items.len(), 5);
        assert_eq!(items[3].value, "•••••••");
        assert_eq!(items[3].reveal, "Show");
        assert!(items[1].note.contains("dropped"), "{}", items[1].note);
        assert!(
            items
                .iter()
                .all(|i| !i.label.contains("hunter2") && !i.note.contains("hunter2"))
        );
        assert_eq!(items[4].label, "Check and save");
        assert_eq!(items[4].button, "");

        form.revealed = true;
        let items = provider_items(&providers, Some(&form));
        assert_eq!(
            (items[3].value.as_str(), items[3].reveal.as_str()),
            ("hunter2", "Hide")
        );

        form.check = FormCheck::Checking(1);
        let items = provider_items(&providers, Some(&form));
        assert_eq!(items[4].note, "Checking the account…");

        // Onboarding ticket 02: editing names the id, saves changes and
        // offers removal, which asks with the counts.
        form.check = FormCheck::None;
        form.id = Some("spare".into());
        let items = provider_items(&providers, Some(&form));
        let labels: Vec<String> = items.iter().map(|i| i.label.to_string()).collect();
        assert_eq!(
            labels,
            [
                "Name",
                "Server URL",
                "Username",
                "Password",
                "Save changes",
                "Remove provider…"
            ]
        );
        assert!(
            items[0].note.contains("spare, never changes"),
            "{}",
            items[0].note
        );

        form.remove = Some(RemoveConfirm {
            name: "Spare TV".into(),
            data: store::ProviderData {
                channels: 2,
                movies: 1,
                series: 3,
                scheduled: 1,
                queued: 0,
                favourites: 1,
                progress: 4,
                recorded: 5,
            },
            personal: false,
        });
        let items = provider_items(&providers, Some(&form));
        assert_eq!(items.len(), 2);
        assert_eq!((items[0].kind, items[0].chosen), (SettingKind::Toggle, 0));
        assert_eq!(
            items[0].note,
            "1 favourite, 4 with progress, 5 recorded or downloaded · files on disk stay"
        );
        assert_eq!(items[1].label, "Remove Spare TV");
        assert_eq!(
            items[1].note,
            "Deletes 2 channels, 1 movie and 3 series and their guide · cancels 1 scheduled recording · forgets 0 queued downloads"
        );
    }

    #[test]
    fn account_view_shows_what_the_provider_sent() {
        use super::{AccountInfo, account_view, end_of_day};
        use crate::xtream::{Blocked, ServerInfo, UserInfo};
        let now = end_of_day(1_790_000_000) - 3600;
        let full = AccountInfo {
            user: UserInfo {
                auth: Some(1),
                status: Some("Active".into()),
                exp_date: Some(end_of_day(now) + 159 * 86_400 + 3600),
                max_connections: Some(2),
                active_cons: Some(1),
                is_trial: Some(false),
                created_at: Some(now - 200 * 86_400),
                allowed_output_formats: vec!["m3u8".into(), "ts".into()],
                message: Some(" Welcome ".into()),
            },
            server: ServerInfo {
                port: Some("80".into()),
                https_port: Some("443".into()),
                rtmp_port: None,
                server_protocol: Some("http".into()),
                timezone: Some("UTC".into()),
                time_now: Some("2026-09-30 14:05:12".into()),
            },
            blocked: None,
            checked: now,
        };
        let v = account_view("Demo TV", Some(&full), false, now);
        assert_eq!(v.title, "Demo TV");
        assert!(v.note.starts_with("Checked "), "{}", v.note);
        assert_eq!((v.status.as_str(), v.ok), ("Active", true));
        assert_eq!(v.message, "Welcome");
        assert!(v.expires.ends_with(" · in 160 days"), "{}", v.expires);
        assert_eq!(v.trial, "No");
        assert_eq!(v.connections, "1 of 2 in use");
        assert_eq!(v.formats, "m3u8, ts (using ts)");
        assert_eq!(v.ports, "HTTP 80 · HTTPS 443");
        assert_eq!(v.protocol, "http");
        assert_eq!(v.server_time, "2026-09-30 14:05:12");
        assert!(
            account_view("Demo TV", Some(&full), true, now)
                .note
                .starts_with("Checking…")
        );

        // Only `auth`: nothing invented.
        let bare = AccountInfo {
            user: UserInfo {
                auth: Some(1),
                ..Default::default()
            },
            server: ServerInfo::default(),
            blocked: None,
            checked: now,
        };
        let v = account_view("Demo TV", Some(&bare), false, now);
        assert_eq!(v.status, "");
        assert_eq!(v.expires, "Never");
        for field in [
            &v.created,
            &v.trial,
            &v.connections,
            &v.formats,
            &v.ports,
            &v.timezone,
            &v.message,
        ] {
            assert_eq!(field, "");
        }

        // Past its date while still `Active`.
        let expired = AccountInfo {
            blocked: Some(Blocked::Expired),
            user: UserInfo {
                exp_date: Some(now - 3 * 86_400),
                ..full.user.clone()
            },
            ..full.clone()
        };
        let v = account_view("Demo TV", Some(&expired), false, now);
        assert_eq!((v.status.as_str(), v.ok), ("Expired", false));
        assert!(v.expires.ends_with(" · 3 days ago"), "{}", v.expires);

        // Passed since the check, not yet blocked by one.
        let passed = AccountInfo {
            blocked: None,
            ..expired.clone()
        };
        let v = account_view("Demo TV", Some(&passed), false, now);
        assert_eq!((v.status.as_str(), v.ok), ("Expired", false));

        let v = account_view("Demo TV", None, false, now);
        assert!(v.note.starts_with("Not checked yet"));
    }

    #[test]
    fn expiry_warns_within_a_week() {
        use super::{AccountInfo, Urgency, account_text, end_of_day};
        use crate::xtream::{Blocked, ServerInfo, UserInfo};
        // Local noon, so whole days later stay on the clock.
        let now = end_of_day(1_790_000_000) - 12 * 3600;
        let at = |exp: i64, blocked: Option<Blocked>| {
            let a = AccountInfo {
                user: UserInfo {
                    exp_date: Some(exp),
                    ..Default::default()
                },
                server: ServerInfo::default(),
                blocked,
                checked: now,
            };
            account_text(&a, now)
        };
        let days = |d: i64| at(now + d * 86_400, None);
        let (text, u) = days(8);
        assert!(
            text.starts_with("Expires ") && !text.contains("in "),
            "{text}"
        );
        assert_eq!(u, Urgency::None);
        assert_eq!(days(7), ("Expires in 7 days".into(), Urgency::Warn));
        assert_eq!(days(4), ("Expires in 4 days".into(), Urgency::Warn));
        assert_eq!(days(3), ("Expires in 3 days".into(), Urgency::Danger));
        assert_eq!(days(1), ("Expires tomorrow 12:00".into(), Urgency::Danger));
        assert_eq!(
            at(now + 6 * 3600, None),
            ("Expires today 18:00".into(), Urgency::Danger)
        );
        // Passed since the last check, and blocked by it.
        let (text, u) = at(now - 60, None);
        assert!(text.starts_with("Expired "), "{text}");
        assert_eq!(u, Urgency::Danger);
        let (text, u) = at(now - 86_400, Some(Blocked::Expired));
        assert!(text.starts_with("Expired "), "{text}");
        assert_eq!(u, Urgency::Danger);
        assert_eq!(
            at(now + 30 * 86_400, Some(Blocked::Disabled("Banned".into()))),
            ("Account banned".into(), Urgency::Danger)
        );
    }

    #[test]
    fn the_grid_opens_on_the_channel_or_the_next_with_a_guide() {
        use super::guide_row_for;
        let base = [10, 11, 12, 13, 14, 15];
        let guide = [11, 13, 14];
        // Its own row.
        assert_eq!(guide_row_for(&base, &guide, Some(13)), 1);
        assert_eq!(guide_row_for(&base, &guide, Some(11)), 0);
        // No guide: the next one with a guide.
        assert_eq!(guide_row_for(&base, &guide, Some(10)), 0);
        assert_eq!(guide_row_for(&base, &guide, Some(12)), 1);
        // None after it: the last before it.
        assert_eq!(guide_row_for(&base, &guide, Some(15)), 2);
        // Not in the category, or nothing selected: the top.
        assert_eq!(guide_row_for(&base, &guide, Some(99)), 0);
        assert_eq!(guide_row_for(&base, &guide, None), 0);
        // No guide channels at all.
        assert_eq!(guide_row_for(&base, &[], Some(12)), 0);
    }

    /// `[` and `]` in the grid step through the library as the sidebar
    /// lists it: the views across kinds, then the kind's, only the shown
    /// categories. The kind switch picks whose (VOD ticket 03).
    #[test]
    fn the_library_lists_the_kinds_shown_categories() {
        use super::library;
        use crate::store::{self, Kind, Order};
        let dir = tempfile::tempdir().unwrap();
        let mut conn = store::open(&dir.path().join("v.db")).unwrap();
        let tx = conn.transaction().unwrap();
        let cats = [("a", "UK| News"), ("b", "US| News"), ("c", "AA| Sport")];
        store::begin_channel_import(&tx, "p", "P", cats).unwrap();
        store::finish_channel_import(&tx, "p", 0).unwrap();
        // Ids repeat across kinds.
        let movie_cats = [("a", "EN - Drama"), ("b", "EN - Comedy")];
        store::begin_import(&tx, "p", "P", Kind::Movie, movie_cats).unwrap();
        store::finish_vod_import(&tx, "p", Kind::Movie, 0).unwrap();
        store::begin_import(&tx, "p", "P", Kind::Series, [("a", "Crime Series")]).unwrap();
        store::finish_vod_import(&tx, "p", Kind::Series, 0).unwrap();
        tx.commit().unwrap();
        let names = |conn: &rusqlite::Connection, kind, order| -> Vec<String> {
            library(kind, store::categories(conn, "p", kind, order).unwrap())
                .iter()
                .map(|c| c.name().to_string())
                .collect()
        };
        let with = |kind: &[&str], cats: &[&str]| -> Vec<String> {
            ["Favourites", "Recently played", "Continue watching"]
                .iter()
                .chain(kind)
                .chain(cats)
                .map(|s| s.to_string())
                .collect()
        };
        let live = |cats| with(&["All channels"], cats);
        let movies = |cats| with(&["All movies", "Recently added"], cats);
        assert_eq!(
            names(&conn, Kind::Live, Order::Provider),
            live(&["UK| News", "US| News", "AA| Sport"])
        );
        assert_eq!(
            names(&conn, Kind::Live, Order::Az),
            live(&["AA| Sport", "UK| News", "US| News"])
        );
        assert_eq!(
            names(&conn, Kind::Movie, Order::Provider),
            movies(&["EN - Drama", "EN - Comedy"])
        );
        assert_eq!(
            names(&conn, Kind::Movie, Order::Az),
            movies(&["EN - Comedy", "EN - Drama"])
        );
        assert_eq!(
            names(&conn, Kind::Series, Order::Provider),
            with(&["All series", "Recently added"], &["Crime Series"])
        );
        // Show only UK|: the others are left out, the fixed views stay, and
        // the movie categories have rules of their own.
        store::toggle_category_rule(&mut conn, "p", Kind::Live, "UK|").unwrap();
        assert_eq!(
            names(&conn, Kind::Live, Order::Provider),
            live(&["UK| News"])
        );
        assert_eq!(
            names(&conn, Kind::Movie, Order::Provider),
            movies(&["EN - Drama", "EN - Comedy"])
        );
        store::toggle_category(&mut conn, "p", Kind::Movie, "b").unwrap();
        assert_eq!(
            names(&conn, Kind::Movie, Order::Provider),
            movies(&["EN - Drama"])
        );
    }

    /// Every action has a row on the keys panel, and single-character keys
    /// on it do what it says.
    #[test]
    fn keys_panel_lists_every_action() {
        use super::{KEY_PANEL, key_panel};
        use slint::Model;
        // Exhaustive, so a new action does not compile until it has a key.
        fn panel_key(a: &super::Action) -> &'static str {
            match a {
                Move(_) => "j",
                HalfPage(_) => "Ctrl d",
                Page(_) => "PgDn",
                Top => "g",
                Bottom => "G",
                Category(_) => "h",
                Play => "Enter",
                Favourite => "f",
                MoveFavourite(_) => "J",
                Search => "/",
                ClearFilter => "Esc",
                Refresh => "r",
                Providers => "p",
                Control(Pause) => "Space",
                Control(Stop) => "s",
                Control(Mute) => "m",
                Control(Fullscreen) => "F",
                Control(Live) => "L",
                Control(Restart) => "S",
                Record => "R",
                StartOver => "S",
                LastChannel => "-",
                Alternates => "o",
                ClearHistory => "c",
                Watched => "w",
                Download => "d",
                DownloadSeason => "D",
                ToggleOrder => "a",
                Manage => "e",
                GuideSources => "E",
                Account => "i",
                Keys => "?",
                Recordings => "v",
                Settings => ",",
                Guide => "t",
                Jump(super::Jump::Now) => "n",
                Jump(super::Jump::Day(-1)) => "<",
                Jump(super::Jump::Day(_)) => ">",
                Jump(super::Jump::Prime) => "P",
                GuideCategory(-1) => "[",
                GuideCategory(_) => "]",
                Cycle(-1) => "⇧Tab",
                Cycle(_) => "Tab",
                Quit => "q",
                #[cfg(feature = "devtools")]
                Snapshot => "F12",
            }
        }
        let mut all = vec![
            Move(1),
            HalfPage(1),
            Page(1),
            Top,
            Bottom,
            Category(-1),
            Play,
            Favourite,
            MoveFavourite(1),
            Search,
            ClearFilter,
            Refresh,
            Providers,
            Control(Pause),
            Control(Stop),
            Control(Mute),
            Control(Fullscreen),
            Control(Live),
            Record,
            StartOver,
            LastChannel,
            Alternates,
            ClearHistory,
            Watched,
            ToggleOrder,
            Manage,
            GuideSources,
            Account,
            Keys,
            Recordings,
            Settings,
            Guide,
            Jump(super::Jump::Now),
            Jump(super::Jump::Day(-1)),
            Jump(super::Jump::Day(1)),
            Jump(super::Jump::Prime),
            GuideCategory(-1),
            GuideCategory(1),
            Cycle(-1),
            Cycle(1),
            Quit,
        ];
        #[cfg(feature = "devtools")]
        all.push(Snapshot);
        let listed: Vec<&str> = KEY_PANEL
            .iter()
            .flat_map(|groups| groups.iter())
            .flat_map(|(_, rows)| rows.iter())
            .flat_map(|(keys, _)| keys.iter().copied())
            .collect();
        for a in &all {
            let key = panel_key(a);
            assert!(listed.contains(&key), "{a:?}: {key} is not on the panel");
            if key.chars().count() == 1 {
                assert_eq!(action(key, false).as_ref(), Some(a), "{key}");
            }
        }
        // `p` with one provider too: the picker adds one (onboarding
        // ticket 01).
        let (columns, notes) = key_panel();
        let keys = |columns: &[super::KeyColumn]| -> Vec<String> {
            columns
                .iter()
                .flat_map(|c| c.groups.iter())
                .flat_map(|g| g.rows.iter().collect::<Vec<_>>())
                .flat_map(|r| r.keys.iter().map(|k| k.to_string()).collect::<Vec<_>>())
                .collect()
        };
        assert!(keys(&columns).contains(&"p".to_string()));
        assert!(notes.iter().any(|n| n.view == "Provider picker"));
    }
}
