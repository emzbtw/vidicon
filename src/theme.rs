//! The colour palette (themes milestone): the base tokens of `Palette` in
//! `ui/app.slint`. The translucent tokens there are derived from these.

use crate::config::{self, Theme};
use crate::ui::AppWindow;
use slint::{Color, ComponentHandle};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A theme that ships with Vidicon: its name in `config.toml`, its label
/// in the Theme picker and its file.
pub type BuiltIn = (&'static str, &'static str, &'static str);

/// The built-in themes (built-in themes ticket 02).
pub const BUILT_IN: &[BuiltIn] = &[];

/// One colour per base token; the default is the built-in dark palette, the
/// same values `ui/app.slint` starts with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub bg: Color,
    pub surface: Color,
    pub surface_2: Color,
    pub line: Color,
    pub text: Color,
    pub dim: Color,
    pub icon: Color,
    pub accent: Color,
    pub warn: Color,
    pub danger: Color,
    pub star: Color,
    pub ok: Color,
    pub logo_backing: Color,
    pub highlight: Color,
    pub shade: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::from_argb_encoded(0xff00_0000 | hex)
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            bg: rgb(0x0e1014),
            surface: rgb(0x151920),
            surface_2: rgb(0x1c212a),
            line: rgb(0x262c37),
            text: rgb(0xe7e9ee),
            dim: rgb(0x8b93a4),
            icon: rgb(0xc9cedb),
            accent: rgb(0x7c9cff),
            warn: rgb(0xe8a86b),
            danger: rgb(0xef6b6b),
            star: rgb(0xf2c14e),
            ok: rgb(0x6fcf97),
            logo_backing: rgb(0x3a404c),
            highlight: rgb(0xffffff),
            shade: rgb(0x000000),
        }
    }
}

impl Palette {
    /// A theme file (themes ticket 02): `token = "#rrggbb"` for any of the
    /// base tokens, named as in `Palette`. A token left out keeps its
    /// default; an unknown key is ignored, as a newer template may add one.
    /// An error names the line.
    pub fn parse(text: &str) -> Result<Self, String> {
        let at = |span: Option<std::ops::Range<usize>>, what: &str| match span {
            Some(s) => format!("line {}: {what}", text[..s.start].matches('\n').count() + 1),
            None => what.to_owned(),
        };
        let table: BTreeMap<String, toml::Spanned<toml::Value>> =
            toml::from_str(text).map_err(|e| at(e.span(), e.message().trim()))?;
        let mut palette = Self::default();
        for (key, value) in table {
            let Some(token) = palette.token(&key) else {
                continue;
            };
            *token = value
                .get_ref()
                .as_str()
                .and_then(hex)
                .ok_or_else(|| at(Some(value.span()), "not a colour"))?;
        }
        Ok(palette)
    }

    fn token(&mut self, name: &str) -> Option<&mut Color> {
        Some(match name {
            "bg" => &mut self.bg,
            "surface" => &mut self.surface,
            "surface-2" => &mut self.surface_2,
            "line" => &mut self.line,
            "text" => &mut self.text,
            "dim" => &mut self.dim,
            "icon" => &mut self.icon,
            "accent" => &mut self.accent,
            "warn" => &mut self.warn,
            "danger" => &mut self.danger,
            "star" => &mut self.star,
            "ok" => &mut self.ok,
            "logo-backing" => &mut self.logo_backing,
            "highlight" => &mut self.highlight,
            "shade" => &mut self.shade,
            _ => return None,
        })
    }

    /// Sets every base token; the window repaints with it at once.
    pub fn apply(&self, window: &AppWindow) {
        let tokens = window.global::<crate::ui::Palette>();
        tokens.set_bg(self.bg);
        tokens.set_surface(self.surface);
        tokens.set_surface_2(self.surface_2);
        tokens.set_line(self.line);
        tokens.set_text(self.text);
        tokens.set_dim(self.dim);
        tokens.set_icon(self.icon);
        tokens.set_accent(self.accent);
        tokens.set_warn(self.warn);
        tokens.set_danger(self.danger);
        tokens.set_star(self.star);
        tokens.set_ok(self.ok);
        tokens.set_logo_backing(self.logo_backing);
        tokens.set_highlight(self.highlight);
        tokens.set_shade(self.shade);
    }
}

/// `#rrggbb`, opaque.
fn hex(text: &str) -> Option<Color> {
    let digits = text.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok().map(rgb)
}

/// Where a theme's palette comes from (built-in themes ticket 01).
#[derive(Debug, PartialEq)]
enum Source {
    Default,
    /// Noctalia's palette file or a user theme, watched; the note for
    /// when it is missing.
    File(PathBuf, String),
    BuiltIn(&'static str),
    /// The note.
    Unknown(String),
}

/// A user file `<name>.toml` in `dir` first, then a built-in.
fn resolve(theme: &Theme, dir: &Path, built_in: &[BuiltIn]) -> Source {
    let name = match theme {
        Theme::Default => return Source::Default,
        Theme::Noctalia => {
            let path = config::noctalia_palette_path();
            let missing = format!("No Noctalia palette at {}", path.display());
            return Source::File(path, missing);
        }
        Theme::Named(name) => name,
    };
    let missing = format!("No theme \"{name}\"");
    let path = dir.join(format!("{name}.toml"));
    if !name.contains('/') && path.is_file() {
        return Source::File(path, missing);
    }
    match built_in.iter().find(|(n, ..)| n == name) {
        Some((.., text)) => Source::BuiltIn(text),
        None => Source::Unknown(missing),
    }
}

/// The Theme picker's options, name and label each: Default, Noctalia, the
/// built-ins, then the user themes in `dir` by file stem in name order. A
/// user file with a built-in's name is that built-in's entry.
fn choices_in(dir: &Path, built_in: &[BuiltIn]) -> Vec<(String, String)> {
    let mut user: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "toml" || !path.is_file() {
                return None;
            }
            path.file_stem()?.to_str().map(str::to_owned)
        })
        .filter(|stem| {
            matches!(Theme::from(stem.clone()), Theme::Named(_))
                && !built_in.iter().any(|(n, ..)| n == stem)
        })
        .collect();
    user.sort();
    [("default", "Default"), ("noctalia", "Noctalia")]
        .into_iter()
        .chain(built_in.iter().map(|(name, label, _)| (*name, *label)))
        .map(|(name, label)| (name.to_owned(), label.to_owned()))
        .chain(user.into_iter().map(|stem| (stem.clone(), stem)))
        .collect()
}

/// [`choices_in`] the user's themes directory.
pub fn choices() -> Vec<(String, String)> {
    choices_in(&config::themes_dir(), BUILT_IN)
}

/// The palette `theme` names, and why it is not the one asked for, if it
/// is not (built-in themes ticket 01). A file is watched, with the theme
/// it is for; the rest are fixed.
pub enum Chosen {
    Fixed(Palette, Option<String>),
    Watched(Watched, Theme),
}

impl Chosen {
    pub fn new(theme: &Theme) -> Self {
        Self::from_source(resolve(theme, &config::themes_dir(), BUILT_IN), theme)
    }

    fn from_source(source: Source, theme: &Theme) -> Self {
        match source {
            Source::Default => Self::Fixed(Palette::default(), None),
            Source::File(path, missing) => {
                Self::Watched(Watched::new(path, missing), theme.clone())
            }
            // Ticket 02's test parses every built-in.
            Source::BuiltIn(text) => Self::Fixed(Palette::parse(text).unwrap_or_default(), None),
            Source::Unknown(note) => Self::Fixed(Palette::default(), Some(note)),
        }
    }

    pub fn palette(&self) -> Palette {
        match self {
            Self::Fixed(palette, _) => *palette,
            Self::Watched(watched, _) => watched.palette,
        }
    }

    pub fn note(&self) -> Option<&str> {
        match self {
            Self::Fixed(_, note) => note.as_deref(),
            Self::Watched(watched, _) => watched.note.as_deref(),
        }
    }

    pub fn is_watched(&self) -> bool {
        matches!(self, Self::Watched(..))
    }

    /// [`Watched::poll`]; a fixed palette never changes.
    pub fn poll(&mut self) -> bool {
        self.poll_in(&config::themes_dir(), BUILT_IN)
    }

    /// A removed user file with a built-in's name gives the built-in back,
    /// as copying one to tweak it made the file; with no built-in it stays
    /// watched, so the file can come back.
    fn poll_in(&mut self, dir: &Path, built_in: &[BuiltIn]) -> bool {
        let Self::Watched(watched, theme) = self else {
            return false;
        };
        if !watched.poll() {
            return false;
        }
        if watched.mtime.is_none()
            && let source @ Source::BuiltIn(_) = resolve(theme, dir, built_in)
        {
            *self = Self::from_source(source, &theme.clone());
        }
        true
    }
}

/// The palette from a theme file, Noctalia's or a user theme, read again
/// whenever its mtime changes (themes ticket 02). A missing or unreadable
/// file gives the default palette, a bad one keeps the last good; either
/// way `note` says why, `missing` when there is no file.
pub struct Watched {
    path: PathBuf,
    missing: String,
    mtime: Option<SystemTime>,
    pub palette: Palette,
    pub note: Option<String>,
}

impl Watched {
    pub fn new(path: PathBuf, missing: String) -> Self {
        let mut watched = Self {
            path,
            missing,
            mtime: None,
            palette: Palette::default(),
            note: None,
        };
        watched.read();
        watched
    }

    /// Reads the file again if its mtime changed since the last read; true
    /// when it did.
    pub fn poll(&mut self) -> bool {
        if self.modified() == self.mtime {
            return false;
        }
        self.read();
        true
    }

    fn modified(&self) -> Option<SystemTime> {
        std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok()
    }

    fn read(&mut self) {
        self.mtime = self.modified();
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.palette = Palette::default();
                self.note = Some(if e.kind() == std::io::ErrorKind::NotFound {
                    self.missing.clone()
                } else {
                    format!("Cannot read {}: {e}", self.path.display())
                });
                return;
            }
        };
        let parsed = String::from_utf8(bytes)
            .map_err(|_| "not UTF-8".to_owned())
            .and_then(|text| Palette::parse(&text));
        match parsed {
            Ok(palette) => {
                self.palette = palette;
                self.note = None;
            }
            Err(e) => {
                let name = self.path.file_name().unwrap_or_default().to_string_lossy();
                self.note = Some(format!("{name}: {e}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::software_renderer::MinimalSoftwareWindow;
    use slint::platform::{Platform, WindowAdapter};
    use std::rc::Rc;

    /// A window with no display, for reading properties back.
    struct Headless;

    impl Platform for Headless {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(Default::default()))
        }
    }

    fn window() -> AppWindow {
        // Set once per process; a later test finds it set already.
        let _ = slint::platform::set_platform(Box::new(Headless));
        AppWindow::new().unwrap()
    }

    /// The Rust defaults are the ones `ui/app.slint` starts with, so a change
    /// to one file alone fails here.
    #[test]
    fn default_palette_matches_the_slint_defaults() {
        let window = window();
        let tokens = window.global::<crate::ui::Palette>();
        let read = Palette {
            bg: tokens.get_bg(),
            surface: tokens.get_surface(),
            surface_2: tokens.get_surface_2(),
            line: tokens.get_line(),
            text: tokens.get_text(),
            dim: tokens.get_dim(),
            icon: tokens.get_icon(),
            accent: tokens.get_accent(),
            warn: tokens.get_warn(),
            danger: tokens.get_danger(),
            star: tokens.get_star(),
            ok: tokens.get_ok(),
            logo_backing: tokens.get_logo_backing(),
            highlight: tokens.get_highlight(),
            shade: tokens.get_shade(),
        };
        assert_eq!(read, Palette::default());
    }

    #[test]
    fn applying_a_palette_sets_the_window_tokens() {
        let window = window();
        let tokens = window.global::<crate::ui::Palette>();
        let palette = Palette {
            bg: rgb(0x203040),
            ..Palette::default()
        };
        palette.apply(&window);
        assert_eq!(tokens.get_bg(), rgb(0x203040));
        assert_eq!(tokens.get_surface(), Palette::default().surface);
    }

    /// The default derived tokens to the bit (hover and press never show in
    /// a screenshot).
    #[test]
    fn default_derived_tokens() {
        let window = window();
        let tokens = window.global::<crate::ui::Palette>();
        let argb = Color::from_argb_encoded;
        assert_eq!(tokens.get_accent_dim(), argb(0x2e7c9cff));
        assert_eq!(tokens.get_danger_dim(), argb(0x2eef6b6b));
        assert_eq!(tokens.get_star_tint(), argb(0x22f2c14e));
        assert_eq!(tokens.get_ok_tint(), argb(0x226fcf97));
        assert_eq!(tokens.get_danger_tint(), argb(0x22ef6b6b));
        assert_eq!(tokens.get_warn_tint(), argb(0x1fe8a86b));
        assert_eq!(tokens.get_hover(), argb(0x0fffffff));
        assert_eq!(tokens.get_pressed(), argb(0x1affffff));
        assert_eq!(tokens.get_initials(), argb(0xe6ffffff));
        assert_eq!(tokens.get_scrim(), argb(0x99000000));
        assert_eq!(tokens.get_shadow(), argb(0x88000000));
    }

    const FULL: &str = "\
bg = \"#101010\"
surface = \"#111111\"
surface-2 = \"#121212\"
line = \"#131313\"
text = \"#f0f0f0\"
dim = \"#a0a0a0\"
icon = \"#b0b0b0\"
accent = \"#80C0FF\"
warn = \"#ffa040\"
danger = \"#ff4040\"
star = \"#ffff40\"
ok = \"#40ff40\"
logo-backing = \"#404040\"
highlight = \"#fefefe\"
shade = \"#010101\"
";

    #[test]
    fn a_full_file_sets_every_token() {
        assert_eq!(
            Palette::parse(FULL).unwrap(),
            Palette {
                bg: rgb(0x101010),
                surface: rgb(0x111111),
                surface_2: rgb(0x121212),
                line: rgb(0x131313),
                text: rgb(0xf0f0f0),
                dim: rgb(0xa0a0a0),
                icon: rgb(0xb0b0b0),
                accent: rgb(0x80c0ff),
                warn: rgb(0xffa040),
                danger: rgb(0xff4040),
                star: rgb(0xffff40),
                ok: rgb(0x40ff40),
                logo_backing: rgb(0x404040),
                highlight: rgb(0xfefefe),
                shade: rgb(0x010101),
            }
        );
    }

    #[test]
    fn a_partial_file_keeps_the_other_defaults_and_ignores_unknown_keys() {
        let text = "# from a newer template\naccent = \"#80c0ff\"\nsparkle = \"#ffffff\"\n\
                    [extra]\nsurface = 3\n";
        assert_eq!(
            Palette::parse(text).unwrap(),
            Palette {
                accent: rgb(0x80c0ff),
                ..Palette::default()
            }
        );
    }

    #[test]
    fn a_bad_colour_or_line_is_named() {
        for (text, err) in [
            (
                "bg = \"#101010\"\n\ntext = \"#fff\"\n",
                "line 3: not a colour",
            ),
            ("bg = \"#10101g\"\n", "line 1: not a colour"),
            ("bg = \"+10101\"\n", "line 1: not a colour"),
            ("bg = \"#+10101\"\n", "line 1: not a colour"),
            ("dim = 7\n", "line 1: not a colour"),
            ("bg = \"#101010\"\naccent = #80c0ff\n", "line 2: "),
        ] {
            let got = Palette::parse(text).unwrap_err();
            assert!(got.starts_with(err), "{text:?}: {got}");
        }
    }

    /// A built-in for the tests, as ticket 02's list is not here yet.
    const TEST_BUILT_IN: &[BuiltIn] = &[("dusk", "Dusk", "bg = \"#202020\"\n")];

    fn named(name: &str) -> Theme {
        Theme::Named(name.into())
    }

    #[test]
    fn a_name_is_a_user_file_then_a_built_in() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        std::fs::write(dir.join("mine.toml"), "bg = \"#303030\"\n").unwrap();
        let resolve = |theme: &Theme| resolve(theme, dir, TEST_BUILT_IN);
        assert_eq!(
            resolve(&named("mine")),
            Source::File(dir.join("mine.toml"), "No theme \"mine\"".into())
        );
        assert_eq!(
            resolve(&named("dusk")),
            Source::BuiltIn("bg = \"#202020\"\n")
        );
        let unknown = |name: &str| Source::Unknown(format!("No theme \"{name}\""));
        assert_eq!(resolve(&named("nord")), unknown("nord"));
        assert_eq!(resolve(&named("../mine")), unknown("../mine"));

        // A user file with a built-in's name replaces it.
        std::fs::write(dir.join("dusk.toml"), "bg = \"#404040\"\n").unwrap();
        assert_eq!(
            resolve(&named("dusk")),
            Source::File(dir.join("dusk.toml"), "No theme \"dusk\"".into())
        );

        let unknown = Chosen::from_source(resolve(&named("nord")), &named("nord"));
        assert_eq!(unknown.palette(), Palette::default());
        assert_eq!(unknown.note(), Some("No theme \"nord\""));
        let user = Chosen::from_source(resolve(&named("mine")), &named("mine"));
        assert_eq!((user.palette().bg, user.note()), (rgb(0x303030), None));
        assert!(user.is_watched());
    }

    #[test]
    fn removing_a_tweaked_built_in_gives_the_built_in_back() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let poll = |chosen: &mut Chosen| chosen.poll_in(dir, TEST_BUILT_IN);
        for name in ["dusk", "mine"] {
            rewrite(
                &dir.join(format!("{name}.toml")),
                "bg = \"#404040\"\n",
                1_000,
            );
        }

        let mut dusk =
            Chosen::from_source(resolve(&named("dusk"), dir, TEST_BUILT_IN), &named("dusk"));
        assert_eq!(dusk.palette().bg, rgb(0x404040));
        std::fs::remove_file(dir.join("dusk.toml")).unwrap();
        assert!(poll(&mut dusk));
        assert_eq!((dusk.palette().bg, dusk.note()), (rgb(0x202020), None));
        assert!(!dusk.is_watched() && !poll(&mut dusk));

        // With no built-in: the default and the note, still watched.
        let mut mine =
            Chosen::from_source(resolve(&named("mine"), dir, TEST_BUILT_IN), &named("mine"));
        std::fs::remove_file(dir.join("mine.toml")).unwrap();
        assert!(poll(&mut mine));
        assert_eq!(mine.palette(), Palette::default());
        assert_eq!(mine.note(), Some("No theme \"mine\""));
        rewrite(&dir.join("mine.toml"), "bg = \"#505050\"\n", 2_000);
        assert!(poll(&mut mine));
        assert_eq!((mine.palette().bg, mine.note()), (rgb(0x505050), None));
    }

    /// `config.toml`'s values from before names keep their meaning.
    #[test]
    fn default_and_noctalia_load_as_before() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        // Files named like them in the themes directory change nothing.
        std::fs::write(dir.join("default.toml"), "bg = \"#303030\"\n").unwrap();
        std::fs::write(dir.join("noctalia.toml"), "bg = \"#303030\"\n").unwrap();
        for (text, theme) in [("default", Theme::Default), ("noctalia", Theme::Noctalia)] {
            let path = dir.join("config.toml");
            std::fs::write(&path, format!("theme = \"{text}\"\n")).unwrap();
            assert_eq!(config::load_settings(&path).unwrap().theme, theme);
        }
        assert_eq!(
            resolve(&Theme::Default, dir, TEST_BUILT_IN),
            Source::Default
        );
        let Source::File(path, note) = resolve(&Theme::Noctalia, dir, TEST_BUILT_IN) else {
            panic!("noctalia is a file");
        };
        assert!(path.ends_with("vidicon/noctalia-palette.toml"), "{path:?}");
        assert!(note.starts_with("No Noctalia palette at /"), "{note}");
        assert!(note.ends_with("noctalia-palette.toml"), "{note}");
        let fixed = Chosen::from_source(Source::Default, &Theme::Default);
        assert_eq!((fixed.palette(), fixed.note()), (Palette::default(), None));
        assert!(!fixed.is_watched());
    }

    #[test]
    fn the_picker_lists_built_ins_then_user_themes_by_name() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let names = |dir: &Path| -> Vec<String> {
            choices_in(dir, TEST_BUILT_IN)
                .into_iter()
                .map(|(name, label)| format!("{name}:{label}"))
                .collect()
        };
        assert_eq!(
            names(&dir.join("missing")),
            ["default:Default", "noctalia:Noctalia", "dusk:Dusk"]
        );
        for file in [
            "zeta.toml",
            "alpha.toml",
            "dusk.toml",
            "default.toml",
            "notes.txt",
        ] {
            std::fs::write(dir.join(file), "").unwrap();
        }
        std::fs::create_dir(dir.join("folder.toml")).unwrap();
        assert_eq!(
            names(dir),
            [
                "default:Default",
                "noctalia:Noctalia",
                "dusk:Dusk",
                "alpha:alpha",
                "zeta:zeta"
            ]
        );
    }

    /// Sets the mtime apart from the last write's, as a later rewrite
    /// would, however fast the test runs.
    fn rewrite(path: &std::path::Path, text: impl AsRef<[u8]>, secs: u64) {
        std::fs::write(path, text).unwrap();
        let mtime = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    #[test]
    fn a_watched_file_is_read_again_when_it_changes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("noctalia-palette.toml");
        let mut watched = Watched::new(path.clone(), "No palette".into());
        assert_eq!(watched.palette, Palette::default());
        assert_eq!(watched.note.as_deref(), Some("No palette"));
        assert!(!watched.poll());

        rewrite(&path, "bg = \"#203040\"\n", 1_000);
        assert!(watched.poll());
        assert_eq!(watched.palette.bg, rgb(0x203040));
        assert_eq!(watched.note, None);
        assert!(!watched.poll());

        // A bad file keeps the last good palette.
        rewrite(&path, "bg = \"#203040\"\n\nok = \"green\"\n", 2_000);
        assert!(watched.poll());
        assert_eq!(watched.palette.bg, rgb(0x203040));
        assert_eq!(
            watched.note.as_deref(),
            Some("noctalia-palette.toml: line 3: not a colour")
        );

        rewrite(&path, b"bg = \"#\xff\"\n", 2_500);
        assert!(watched.poll());
        assert_eq!(watched.palette.bg, rgb(0x203040));
        assert_eq!(
            watched.note.as_deref(),
            Some("noctalia-palette.toml: not UTF-8")
        );

        rewrite(&path, "bg = \"#405060\"\n", 3_000);
        assert!(watched.poll());
        assert_eq!((watched.palette.bg, &watched.note), (rgb(0x405060), &None));

        // Removed: the default again.
        std::fs::remove_file(&path).unwrap();
        assert!(watched.poll());
        assert_eq!(watched.palette, Palette::default());
        assert!(watched.note.is_some());
    }
}
