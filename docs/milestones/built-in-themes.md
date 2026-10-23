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
