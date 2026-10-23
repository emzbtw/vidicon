# Milestone: themes

Planned 2026-10-23, after the M3U milestone (`m3u.md`). Tickets in
`.scratch/themes/`. Amends SPEC 8.1 ("Dark theme").

## Why now

The colours are fixed at build time. The user's desktop shell, Noctalia
(v5), already makes palettes: its built-in schemes, community palettes,
and colours taken from the wallpaper. It hands them to apps through
templates it re-renders on every theme change. Taking Noctalia's palette
gives Vidicon all three sources at once, so it comes before any palettes
of Vidicon's own (Catppuccin, Gruvbox, Nord, Tokyo Night: a later
milestone, in the same file format, `built-in-themes.md`).

## Requirements

- **One palette, set at run time.** Every colour in `ui/app.slint` comes
  from the `Palette` global. The about 24 literals that bypass it today
  (hover and pressed overlays, modal scrims, the drop shadow, the icon
  grey, tinted fills) become tokens or are derived from one. Rust sets
  the tokens; a change applies live, with no restart.
- **The default palette does not change.** With no theme set, every
  view looks as it does now, pixel for pixel.
- **A theme file** in a small TOML format of Vidicon's own, one hex
  colour per base token. Translucent tokens (`accent-dim`, hover,
  pressed, scrim) are derived from base ones with fixed alphas, so a
  theme file never has to get an alpha right.
- **`theme` in `config.toml`**: `"default"` or `"noctalia"`, edited in
  the Settings view (SPEC 4.2 rule: the key names its section).
- **Noctalia's file is watched.** Noctalia rewrites it on a wallpaper or
  scheme change and Vidicon applies the new palette within about a
  second. This is the one file the app watches (4.2 says the app never
  watches its own files; that rule stays for `config.toml` and
  `credentials.toml`).
- **A missing or broken file never breaks the UI.** Vidicon uses the
  default palette and the Settings field says why (`No Noctalia palette
  at <path>`, `noctalia-palette.toml: line 3: not a colour`). A file
  missing some tokens fills them from the default.
- **A Noctalia template ships in the repo**, with setup steps in the
  README and on the site. Noctalia 5.3 lists a user template from its
  `config.toml`:

  ```toml
  [theme.templates.user.vidicon]
  input_path = "/path/to/vidicon.toml"
  output_path = "~/.config/vidicon/noctalia-palette.toml"
  ```

  (checked 2026-10-23 with `noctalia theme --list-templates` against a
  throwaway `XDG_CONFIG_HOME`; `[[theme.templates.user]]` passes
  `config validate` but is not listed).
- **Dark only.** The template reads Noctalia's `.dark` variants
  (`{{colors.surface.dark.hex}}`), so Noctalia's light or auto mode
  still gives Vidicon a dark palette. Light themes are a later
  milestone.
- SPEC 4.2 and 8.1 updated; README and site gain a Themes section.

## Tokens (settled in ticket 01)

Base tokens, which Rust sets (`src/theme.rs`) and a theme file will
name: `bg`, `surface`, `surface-2`, `line`, `text`, `dim`, `icon` (icons
at rest, season labels), `accent`, `warn`, `danger`, `star`, `ok`,
`logo-backing`, `highlight` (white: what hover and press lighten with)
and `shade` (black: what scrims and shadows darken with).

Derived in `ui/app.slint` by `.with-alpha()`, each the exact literal it
replaced:

| Token | From | Alpha |
|---|---|---|
| `accent-dim`, `danger-dim` | `accent`, `danger` | 0x2e |
| `star-tint`, `ok-tint`, `danger-tint` | `star`, `ok`, `danger` | 0x22 |
| `warn-tint` | `warn` | 0x1f |
| `hover`, `pressed` | `highlight` | 0x0f, 0x1a |
| `initials` (on a logo tile) | `highlight` | 0xe6 |
| `scrim`, `shadow` | `shade` | 0x99, 0x88 |

`highlight` and `shade` are base tokens, not derived from `text` and
black. `text` is not white, so deriving hover from it would change the
default palette. A Noctalia
template can still set `highlight` from `on_surface`.

## Theme file (settled in ticket 02)

- Path `$XDG_CONFIG_HOME/vidicon/noctalia-palette.toml`: beside
  `config.toml`, not in a cache a clean-up would empty.
- `token = "#rrggbb"` per base token, named as in `Palette`; unknown
  keys ignored, missing ones from the default palette, a bad value or
  TOML error named by line.
- Watched by an mtime check once a second on a UI timer that runs only
  while `theme` is `"noctalia"`, no new dependency
  (`theme::Watched`). Noctalia writes the file by rename, so a read
  never sees half a file. A parse failure keeps the last good palette;
  a missing or unreadable file gives the default one. The reason shows
  in amber under the field and clears on the next good read.
- `theme` is the third field in General, a `Default` / `Noctalia`
  switch like `refresh_hours`.

## Template (settled in ticket 03)

`contrib/noctalia/vidicon.toml`, reading `.dark` variants. Checked by
contrast against 50 renders: Noctalia's 10 schemes on 5 invented
images of different hue and brightness. Worst cases: `text` 11.3:1,
`dim` 6.1, `accent` 4.6, `danger` 4.2 on `bg`, `surface` and
`surface-2`.

- `bg` is `surface_dim`; `surface`, `surface-2`, `line` and
  `logo-backing` are `surface_dim | lighten 4`, `7`, `11` and `19`,
  the steps of the default palette in HSL lightness points. The
  container roles do not work: the non-Material schemes (`vibrant`,
  `faithful`, `soft`, `muted`, `dysfunctional`) put
  `surface_container_low` below `surface` and `surface_container`
  above `_high`, and their `surface` is lighter than the Material ones,
  which `surface_dim` is not.
- `text` and `highlight` are `on_surface`; `dim` and `icon` are
  `on_surface_variant`. `outline` is near 2:1 in the non-Material
  schemes.
- `accent` is `primary`; `shade` is `scrim`.
- `warn`, `danger`, `star` and `ok` are left out and keep the default
  palette's. Material has no orange, yellow or green role, and `error`
  in the non-Material schemes takes the wallpaper's hue (2.1:1, and
  magenta, on a magenta one).
- Noctalia's filters take lightness points (`lighten 4`), not
  fractions. Noctalia reads every `.toml` in `~/.config/noctalia/` as
  config, so the README puts the template in `~/.config/vidicon/`.

Later: offer the template to Noctalia's community catalog so setup is
one toggle in Noctalia's Templates settings.
