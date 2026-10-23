//! The colour palette (themes milestone): the base tokens of `Palette` in
//! `ui/app.slint`. The translucent tokens there are derived from these.

use crate::ui::AppWindow;
use slint::{Color, ComponentHandle};

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
}
