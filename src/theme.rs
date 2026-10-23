//! The colour palette (themes milestone): the base tokens of `Palette` in
//! `ui/app.slint`. The translucent tokens there are derived from these.

use crate::ui::AppWindow;
use slint::{Color, ComponentHandle};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

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

/// The palette from a theme file that is read again whenever its mtime
/// changes (themes ticket 02). A missing or unreadable file gives the
/// default palette, a bad one keeps the last good; either way `note` says
/// why.
pub struct Watched {
    path: PathBuf,
    mtime: Option<SystemTime>,
    pub palette: Palette,
    pub note: Option<String>,
}

impl Watched {
    pub fn new(path: PathBuf) -> Self {
        let mut watched = Self {
            path,
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
                    format!("No Noctalia palette at {}", self.path.display())
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

    /// Sets the mtime apart from the last write's, as a later rewrite
    /// would, however fast the test runs.
    fn rewrite(path: &std::path::Path, text: &str, secs: u64) {
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
        let mut watched = Watched::new(path.clone());
        assert_eq!(watched.palette, Palette::default());
        let note = watched.note.clone().unwrap();
        assert!(note.starts_with("No Noctalia palette at /"), "{note}");
        assert!(note.ends_with("noctalia-palette.toml"), "{note}");
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

        std::fs::write(&path, b"bg = \"#\xff\"\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2_500))
            .unwrap();
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
