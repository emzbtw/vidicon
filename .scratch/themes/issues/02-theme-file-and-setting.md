# 02: Theme file and the `theme` setting

Status: done. Blocked by: 01.

## Goal

`theme = "noctalia"` makes Vidicon read its palette from a theme file
and re-apply it when the file changes, falling back to the default
palette with a reason.

## Context

`docs/milestones/themes.md`. SPEC 4.2 (keys, Settings rule, managed
files), 8.1. `src/config.rs`, `src/ui.rs`, the palette type from 01.

## Scope

- The theme file format: TOML, one `"#rrggbb"` per base token from 01,
  token names as in `Palette`. Unknown keys are ignored (a newer
  template may add some); missing ones come from the default palette.
- `theme` key in `config.toml`, `"default"` or `"noctalia"`, default
  `"default"`. A field in the Settings view's General section, saved
  and applied live like `refresh_hours`.
- The file path (proposed
  `$XDG_CONFIG_HOME/vidicon/noctalia-palette.toml`; settle here).
- Watching: an mtime check about once a second while `theme` is
  `"noctalia"`; a change re-reads and re-applies. A parse failure keeps
  the last good palette.
- Fallback: a missing or unreadable file gives the default palette and
  a note under the Settings field naming the path or the line.
- SPEC 4.2 (the key, the watched-file exception) and 8.1 updated.

## Acceptance

- [x] Tests: a full file, a partial file (defaults fill the rest), a
  bad colour (line named, last good kept), unknown keys ignored, the
  key round-trips through `toml_edit` saving.
- [x] Checked by screenshot against the demo provider with a hand-made
  theme file, and by rewriting the file while the app runs (the palette
  changes without a restart).

## Out of scope

The Noctalia template and its setup docs (03). Built-in palettes
(a later milestone).
