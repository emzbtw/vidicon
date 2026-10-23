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
milestone, in the same file format).

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

## Proposed (settle in the tickets)

- Token mapping from Noctalia's Material 3 names: `bg` from `surface`,
  `surface` and `surface-2` from `surface_container_low` and
  `surface_container`, `line` from `outline_variant`, `text` and `dim`
  from `on_surface` and `on_surface_variant`, `accent` from `primary`,
  `danger` from `error`. Noctalia's `hover` is a solid colour, not an
  overlay, and its `scrim` is opaque black, so neither maps directly:
  hover and pressed are `text` at the current alphas, the scrim is
  `scrim` at the current alpha.
- `star`, `ok` and `warn` stay fixed: Material 3 has no yellow, green or
  orange role. Check them against a few wallpapers.
- `logo-backing` derived from `surface` (lighter by a fixed step), so
  white and dark logos both stay visible.
- Watching by an mtime check once a second on the UI timer, no new
  dependency. Noctalia writes the file by rename, so a read never sees
  half a file; a parse failure keeps the last good palette.
- Output path `$XDG_CONFIG_HOME/vidicon/noctalia-palette.toml`: beside
  `config.toml`, not in a cache a clean-up would empty.
- `theme` in the General section, a two-way choice like
  `refresh_hours`.
- Later: offer the template to Noctalia's community catalog so setup is
  one toggle in Noctalia's Templates settings.
