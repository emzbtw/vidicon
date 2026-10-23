# 01: Theme names and user themes

Status: done. Blocked by: none.

## Goal

`theme` names a palette, and a TOML file in
`~/.config/vidicon/themes/` is a theme the Settings picker lists and a
running Vidicon follows as the file changes.

## Context

`docs/milestones/built-in-themes.md`. `src/theme.rs` (`Palette::parse`,
`Watched`), `src/config.rs` (`Theme`, `noctalia_palette_path`),
`src/ui.rs` (the Theme field in General, the code that applies the
theme and runs the watch timer). SPEC 3 (Paths), 4.2, 8.1. The `Choice`
kind is the picker `Start with` already uses.

## Scope

- `theme` becomes a name: `"default"`, `"noctalia"`, or a theme found
  by name. Resolution: a user file
  `$XDG_CONFIG_HOME/vidicon/themes/<name>.toml` first, then the
  built-ins (an empty list here; ticket 02 fills it). An unknown name
  loads the default palette with the note `No theme "<name>"`.
- A chosen user theme is read and watched with `theme::Watched`, as
  the Noctalia file is: bad value or TOML error named by line, last
  good palette kept on a parse failure, default on a missing file.
- Theme field: `SettingKind::Choice`, options Default, Noctalia, the
  built-ins, then user themes by file stem in name order. The list is
  read when Settings opens. A saved name that is no longer there still
  shows, with its note, so the field never silently changes it.
- `config.toml` keeps today's values meaning what they mean:
  `"default"` and `"noctalia"` still load as before (a test).
- SPEC 3 (the themes directory in Paths), 4.2 (`theme` values), 8.1.

## Acceptance

- [x] Unit tests: name resolution (user file, built-in, unknown), a
  user file shadowing a built-in name (with a test-only built-in),
  and the old `config.toml` values.
- [x] Against the demo provider: a user theme in a throwaway
  `XDG_CONFIG_HOME` is listed, chosen, applied, and re-applied within
  about a second after an edit; removing it gives the default palette
  and the note. Checked with `--screenshot` and `--keys`.
- [x] Choosing Default or Noctalia still behaves as in v0.3.0.

## Out of scope

The built-in palettes themselves (02). Live preview in the picker.
