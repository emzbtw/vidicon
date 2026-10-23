# Milestone: built-in themes

Planned 2026-10-23, after the themes milestone (`themes.md`), which
promised these palettes "in the same file format". Tickets in
`.scratch/built-in-themes/`. Amends SPEC 3 (Paths), 4.2 and 8.1.

## Why now

`theme` has two values: Vidicon's default palette and Noctalia's. A
user without Noctalia has one dark palette. The palette is already set
at run time from a small TOML file (`theme::Palette::parse`), so a
built-in theme is that file compiled in, and a user theme is that file
in a directory.

## Requirements

- **Eight built-in palettes, one dark variant per family:**
  Catppuccin Mocha, Dracula, Everforest Dark (medium), Gruvbox Dark
  (medium), Kanagawa Wave, Nord, Rosé Pine (main), Tokyo Night (night).
  Other variants can follow later; light ones wait for light themes.
- **Each palette sets every base token** (`warn`, `danger`, `star` and
  `ok` included: these families have their own reds, yellows and
  greens, unlike Noctalia's Material roles). The values come from each
  project's published palette, not from memory, and the README credits
  each project with a link.
- **A contrast floor** on `bg`, `surface` and `surface-2`: `text` 7:1,
  `dim` 4.5:1, `accent`, `warn`, `danger`, `star` and `ok` 3:1. A token
  under it takes a different colour from the same family's palette.
  Ticket 03's Noctalia worst cases (`themes.md`) were all above this.
- **User themes:** `$XDG_CONFIG_HOME/vidicon/themes/<name>.toml`, in the
  same format, a missing token filled from the default palette. A user
  file with a built-in's name replaces it, so a built-in can be
  tweaked by copying it.
- **`theme` takes a name:** `"default"`, `"noctalia"`, a built-in name
  (`"catppuccin-mocha"`) or a user theme's file stem. An unknown name
  gives the default palette and the Theme field says
  `No theme "<name>"`, as a missing Noctalia file does now.
- **A chosen user theme is watched** like Noctalia's file
  (`theme::Watched`, an mtime check once a second, only while it is the
  chosen theme), so editing it re-themes a running Vidicon. Built-ins
  are not watched.
- **Settings:** the Theme field becomes a `Choice` picker: Default,
  Noctalia, the built-ins by name, then user themes by file stem, read
  from the directory when Settings opens. A palette applies when an
  option is chosen; no live preview while moving through the list.
- The built-in palettes ship as files in the repo (`assets/themes/`),
  compiled in with `include_str!`, so they double as examples of the
  format.
- SPEC 3, 4.2 and 8.1 updated; README and site Themes section lists
  the built-ins and the user theme format.

## Out of scope

Light themes and light variants. A theme editor in the app. Picker
live preview. Changing the Noctalia template.

## Palettes (settled in ticket 02)

Each file in `assets/themes/` names its source and licence in its
header, and the source palette's name for each colour beside it.
Sources, fetched 2026-10-23: `catppuccin/palette` (`palette.json`),
`dracula/spec` and `dracula/visual-studio-code` (its UI shades),
`sainnhe/everforest` (`palette.md`), `morhetz/gruvbox`
(`colors/gruvbox.vim`), `rebelot/kanagawa.nvim`, `nordtheme/nord`
(`src/nord.css`), `rose-pine/neovim` (`palette.lua`) and
`folke/tokyonight.nvim` (`colors/night.lua` over `storm.lua`). All MIT
but Tokyo Night (Apache-2.0).

| Token | Catppuccin Mocha | Dracula | Everforest Dark | Gruvbox Dark |
|---|---|---|---|---|
| `bg` | crust | BGDark | bg_dim (hard) | dark0 |
| `surface` | mantle | Background | bg_dim | dark0_soft |
| `surface-2` | base | BGLight | bg0 | dark1 |
| `line` | surface0 | BGLighter | bg2 | dark2 |
| `logo-backing` | surface1 | Selection | bg4 | dark3 |
| `text` | text | AnsiBrightWhite | fg | light1 |
| `dim` | overlay2 | Foreground | grey2 | light3 |
| `icon` | subtext1 | Foreground | grey2 | light2 |
| `accent` | mauve | Purple | blue | bright_blue |
| `warn` | peach | Orange | orange | bright_orange |
| `danger` | red | Red | red | bright_red |
| `star` | yellow | Yellow | yellow | bright_yellow |
| `ok` | green | Green | green | bright_green |
| `highlight` | text | Foreground | fg | light1 |
| `shade` | crust | BGDarker | bg_dim (hard) | dark0_hard |

| Token | Kanagawa Wave | Nord | Rosé Pine | Tokyo Night |
|---|---|---|---|---|
| `bg` | sumiInk1 | nord0 | base | bg_dark |
| `surface` | sumiInk3 | nord1 | surface | bg |
| `surface-2` | sumiInk4 | nord2 | overlay | bg_highlight |
| `line` | sumiInk5 | nord3 | highlight_med | fg_gutter |
| `logo-backing` | sumiInk6 | nord3 | highlight_high | terminal_black |
| `text` | fujiWhite | nord6 | text | fg |
| `dim` | springViolet2 | nord4 | subtle | fg_dark |
| `icon` | oldWhite | nord5 | subtle | fg_dark |
| `accent` | crystalBlue | nord8 | iris | blue |
| `warn` | roninYellow | nord13 | gold | orange |
| `danger` | samuraiRed | nord12 | love | red |
| `star` | carpYellow | nord13 | gold | yellow |
| `ok` | springGreen | nord14 | foam | green |
| `highlight` | fujiWhite | nord6 | text | fg |
| `shade` | sumiInk0 | nord0 | base | bg_dark1 |

- **Surfaces** follow the default palette: `bg` darkest, then
  `surface` and `surface-2` a step lighter each, `line` and
  `logo-backing` above them. Where a family names a main background
  (Dracula's Background, Kanagawa's and Tokyo Night's `bg`), it is
  `surface`, with the darker sidebar shade as `bg`.
- **`warn`** is the family's orange and `star` its yellow, as in the
  default palette. Nord and Rosé Pine have no orange; both are the
  family's warning yellow (nord13, gold). Kanagawa's is roninYellow,
  its warning colour, which is orange (surimiOrange is for constants).
- **`ok`** is Rosé Pine's foam, its colour for added lines; it has no
  green.
- **`highlight`** is the family's text colour, as in the Noctalia
  template; **`shade`** its darkest background. Nord's darkest is
  nord0, its `bg`, so a scrim darkens the side panes and cards but not
  the centre; dialogs still stand apart by their surface and line.

Contrast floor (`theme::tests::every_built_in_meets_the_contrast_floor`):
the worst cases are Nord's `danger` 3.0:1, Kanagawa's 3.2, Gruvbox's
3.4, Everforest's `text` 7.4 and `danger` 4.5, Rosé Pine's `dim` 4.7.

Changes from each family's own choice, and why:

- **Everforest `bg`** is bg_dim of the hard contrast. On medium bg1,
  `text` (fg) is 6.4:1; the surfaces moved one step darker, which
  needs a shade below medium's bg_dim.
- **Gruvbox `dim`** is light3, not gray or light4: light4 is 4.2:1 on
  dark1.
- **Kanagawa `dim`** is springViolet2: fujiGray (comments) is 2.9:1
  and springViolet1 4.3 on sumiInk4.
- **Tokyo Night `dim`** is fg_dark: comment and dark5 are 2.2 and
  3.2:1 on bg_highlight.
- **Nord `danger`** is nord12 (orange): nord11, Nord's red, is 2.5:1 on
  nord1 and 2.1 on nord2.
- **Dracula `dim`** is Foreground and `text` its bright white: Comment
  is 3.4:1 on BGDark and 2.5 on BGLight, and the palette has no other
  grey. Purple as `dim`, tried first, read as accent on every row.
  Secondary text is as bright as primary under Dracula; weight and
  size still separate them.

Looked at under each palette against the demo provider: main with
playback, search, guide grid, Settings with the Theme picker open,
the refresh panel and the Keys modal.
