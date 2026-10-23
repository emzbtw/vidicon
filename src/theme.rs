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
type BuiltIn = (&'static str, &'static str, &'static str);

/// A [`BuiltIn`] from `assets/themes/<name>.toml`.
macro_rules! built_in {
    ($name:literal, $label:literal) => {
        (
            $name,
            $label,
            include_str!(concat!("../assets/themes/", $name, ".toml")),
        )
    };
}

/// The built-in themes (built-in themes ticket 02), in picker order.
const BUILT_IN: &[BuiltIn] = &[
    built_in!("catppuccin-mocha", "Catppuccin Mocha"),
    built_in!("dracula", "Dracula"),
    built_in!("everforest-dark", "Everforest Dark"),
    built_in!("gruvbox-dark", "Gruvbox Dark"),
    built_in!("kanagawa-wave", "Kanagawa Wave"),
    built_in!("nord", "Nord"),
    built_in!("rose-pine", "Rosé Pine"),
    built_in!("tokyo-night", "Tokyo Night"),
];

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
    /// Noctalia's palette file or a user theme's, watched: the palette for
    /// when it is missing or bad, and the note for when it is missing.
    File(PathBuf, Palette, Option<String>),
    /// The note.
    Unknown(String),
}

/// A user file `<name>.toml` in `dir`, watched whether or not it exists
/// yet, over the built-in of that name or else the default palette.
fn resolve(theme: &Theme, dir: &Path, built_in: &[BuiltIn]) -> Source {
    let name = match theme {
        Theme::Default => return Source::Default,
        Theme::Noctalia => {
            let path = config::noctalia_palette_path();
            let missing = format!("No Noctalia palette at {}", path.display());
            return Source::File(path, Palette::default(), Some(missing));
        }
        Theme::Named(name) => name,
    };
    let missing = format!("No theme \"{name}\"");
    if name.contains('/') {
        return Source::Unknown(missing);
    }
    let path = dir.join(format!("{name}.toml"));
    match built_in.iter().find(|(n, ..)| n == name) {
        // Ticket 02's test parses every built-in.
        Some((.., text)) => Source::File(path, Palette::parse(text).unwrap_or_default(), None),
        None => Source::File(path, Palette::default(), Some(missing)),
    }
}

/// An option in the Theme picker: the name `config.toml` holds and the
/// label shown.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeChoice {
    pub name: String,
    pub label: String,
}

impl ThemeChoice {
    pub fn new(name: &str, label: &str) -> Self {
        Self {
            name: name.to_owned(),
            label: label.to_owned(),
        }
    }
}

/// The Theme picker's options: Default, Noctalia, the built-ins, then the
/// user themes in `dir` by file stem in name order. A user file with a
/// built-in's name is that built-in's entry.
fn choices_in(dir: &Path, built_in: &[BuiltIn]) -> Vec<ThemeChoice> {
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
    Theme::FIXED
        .iter()
        .map(|(_, name, label)| ThemeChoice::new(name, label))
        .chain(
            built_in
                .iter()
                .map(|(name, label, _)| ThemeChoice::new(name, label)),
        )
        .chain(user.iter().map(|stem| ThemeChoice::new(stem, stem)))
        .collect()
}

/// [`choices_in`] the user's themes directory.
pub fn choices() -> Vec<ThemeChoice> {
    choices_in(&config::themes_dir(), BUILT_IN)
}

/// The palette `theme` names, and why it is not the one asked for, if it
/// is not (built-in themes ticket 01). A file is watched; the rest are
/// fixed.
pub enum Chosen {
    Fixed(Palette, Option<String>),
    Watched(Watched),
}

impl Chosen {
    pub fn new(theme: &Theme) -> Self {
        Self::from_source(resolve(theme, &config::themes_dir(), BUILT_IN))
    }

    fn from_source(source: Source) -> Self {
        match source {
            Source::Default => Self::Fixed(Palette::default(), None),
            Source::File(path, fallback, missing) => {
                Self::Watched(Watched::new(path, fallback, missing))
            }
            Source::Unknown(note) => Self::Fixed(Palette::default(), Some(note)),
        }
    }

    pub fn palette(&self) -> Palette {
        match self {
            Self::Fixed(palette, _) => *palette,
            Self::Watched(watched) => watched.palette,
        }
    }

    pub fn note(&self) -> Option<&str> {
        match self {
            Self::Fixed(_, note) => note.as_deref(),
            Self::Watched(watched) => watched.note.as_deref(),
        }
    }

    pub fn is_watched(&self) -> bool {
        matches!(self, Self::Watched(_))
    }

    /// [`Watched::poll`]; a fixed palette never changes.
    pub fn poll(&mut self) -> bool {
        match self {
            Self::Watched(watched) => watched.poll(),
            Self::Fixed(..) => false,
        }
    }
}

/// The palette from a theme file, Noctalia's or a user theme, read again
/// whenever its mtime changes (themes ticket 02). A missing or unreadable
/// file gives `fallback`, a bad one keeps the last good (`fallback` at
/// first); either way `note` says why, `missing` when there is no file.
pub struct Watched {
    path: PathBuf,
    fallback: Palette,
    missing: Option<String>,
    mtime: Option<SystemTime>,
    pub palette: Palette,
    pub note: Option<String>,
}

impl Watched {
    fn new(path: PathBuf, fallback: Palette, missing: Option<String>) -> Self {
        let mut watched = Self {
            path,
            fallback,
            missing,
            mtime: None,
            palette: fallback,
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
                self.palette = self.fallback;
                self.note = if e.kind() == std::io::ErrorKind::NotFound {
                    self.missing.clone()
                } else {
                    Some(format!("Cannot read {}: {e}", self.path.display()))
                };
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

    /// Every built-in sets every token `FULL` sets, and nothing else.
    #[test]
    fn every_built_in_parses_with_every_token() {
        let keys = |text: &str| -> Vec<String> {
            let table: BTreeMap<String, toml::Value> = toml::from_str(text).unwrap();
            table.into_keys().collect()
        };
        assert_eq!(BUILT_IN.len(), 8);
        for (name, _, text) in BUILT_IN {
            assert_eq!(keys(text), keys(FULL), "{name}");
            assert!(Palette::parse(text).is_ok(), "{name}");
        }
    }

    /// WCAG 2 relative luminance.
    fn luminance(c: Color) -> f64 {
        let linear = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(c.red()) + 0.7152 * linear(c.green()) + 0.0722 * linear(c.blue())
    }

    fn contrast(a: Color, b: Color) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// The contrast floor in `docs/milestones/built-in-themes.md`.
    #[test]
    fn every_built_in_meets_the_contrast_floor() {
        for (name, _, text) in BUILT_IN {
            let p = Palette::parse(text).unwrap();
            for (ink, what, floor) in [
                (p.text, "text", 7.0),
                (p.dim, "dim", 4.5),
                (p.accent, "accent", 3.0),
                (p.warn, "warn", 3.0),
                (p.danger, "danger", 3.0),
                (p.star, "star", 3.0),
                (p.ok, "ok", 3.0),
            ] {
                for (back, on) in [
                    (p.bg, "bg"),
                    (p.surface, "surface"),
                    (p.surface_2, "surface-2"),
                ] {
                    let ratio = contrast(ink, back);
                    assert!(ratio >= floor, "{name}: {what} on {on} is {ratio:.2}:1");
                }
            }
        }
    }

    /// A built-in for the tests, apart from the real list.
    const TEST_BUILT_IN: &[BuiltIn] = &[("dusk", "Dusk", "bg = \"#202020\"\n")];

    fn named(name: &str) -> Theme {
        Theme::Named(name.into())
    }

    #[test]
    fn a_name_watches_its_user_file_over_a_built_in() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        std::fs::write(dir.join("mine.toml"), "bg = \"#303030\"\n").unwrap();
        let resolve = |name: &str| resolve(&named(name), dir, TEST_BUILT_IN);
        let dusk = Palette::parse("bg = \"#202020\"\n").unwrap();
        let note = |name: &str| Some(format!("No theme \"{name}\""));
        assert_eq!(
            resolve("mine"),
            Source::File(dir.join("mine.toml"), Palette::default(), note("mine"))
        );
        // A user file with a built-in's name replaces it, so the built-in
        // is the fallback for a missing or bad one.
        assert_eq!(
            resolve("dusk"),
            Source::File(dir.join("dusk.toml"), dusk, None)
        );
        assert_eq!(
            resolve("../mine"),
            Source::Unknown("No theme \"../mine\"".into())
        );

        let user = Chosen::from_source(resolve("mine"));
        assert_eq!((user.palette().bg, user.note()), (rgb(0x303030), None));
        let built_in = Chosen::from_source(resolve("dusk"));
        assert_eq!((built_in.palette(), built_in.note()), (dusk, None));
        let unknown = Chosen::from_source(resolve("nord"));
        assert_eq!(unknown.palette(), Palette::default());
        assert_eq!(unknown.note(), Some("No theme \"nord\""));
        assert!(user.is_watched() && built_in.is_watched() && unknown.is_watched());
        assert!(!Chosen::from_source(resolve("../mine")).is_watched());
    }

    #[test]
    fn a_user_file_can_come_and_go() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let chosen = |name: &str| Chosen::from_source(resolve(&named(name), dir, TEST_BUILT_IN));

        // Named before its file exists: the default, then the file.
        let mut mine = chosen("mine");
        assert_eq!(mine.note(), Some("No theme \"mine\""));
        rewrite(&dir.join("mine.toml"), "bg = \"#505050\"\n", 1_000);
        assert!(mine.poll());
        assert_eq!((mine.palette().bg, mine.note()), (rgb(0x505050), None));
        std::fs::remove_file(dir.join("mine.toml")).unwrap();
        assert!(mine.poll());
        assert_eq!(mine.palette(), Palette::default());
        assert_eq!(mine.note(), Some("No theme \"mine\""));

        // A tweaked built-in: a bad file keeps the built-in, a removed one
        // gives it back, and a new one is read again.
        let path = dir.join("dusk.toml");
        rewrite(&path, "bg = \"red\"\n", 1_000);
        let mut dusk = chosen("dusk");
        assert_eq!(dusk.palette().bg, rgb(0x202020));
        assert_eq!(dusk.note(), Some("dusk.toml: line 1: not a colour"));
        rewrite(&path, "bg = \"#404040\"\n", 2_000);
        assert!(dusk.poll());
        assert_eq!((dusk.palette().bg, dusk.note()), (rgb(0x404040), None));
        std::fs::remove_file(&path).unwrap();
        assert!(dusk.poll());
        assert_eq!((dusk.palette().bg, dusk.note()), (rgb(0x202020), None));
        rewrite(&path, "bg = \"#606060\"\n", 3_000);
        assert!(dusk.poll());
        assert_eq!((dusk.palette().bg, dusk.note()), (rgb(0x606060), None));
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
        let Source::File(path, fallback, Some(note)) =
            resolve(&Theme::Noctalia, dir, TEST_BUILT_IN)
        else {
            panic!("noctalia is a file with a note");
        };
        assert_eq!(fallback, Palette::default());
        assert!(path.ends_with("vidicon/noctalia-palette.toml"), "{path:?}");
        assert!(note.starts_with("No Noctalia palette at /"), "{note}");
        assert!(note.ends_with("noctalia-palette.toml"), "{note}");
        let fixed = Chosen::from_source(Source::Default);
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
                .map(|choice| format!("{}:{}", choice.name, choice.label))
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
        let mut watched = Watched::new(path.clone(), Palette::default(), Some("No palette".into()));
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
