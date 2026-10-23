# 01: One palette for every colour

Status: open. Blocked by: none.

## Goal

Every colour in `ui/app.slint` comes from `Palette`, and Rust can set
`Palette`, with no visible change.

## Context

`docs/milestones/themes.md`. SPEC 8.1. `ui/app.slint` (`global Palette`
at the top, the literals throughout), `src/ui.rs`.

## Scope

- `Palette` tokens become `in-out` so Rust can set them; their current
  values stay as the defaults.
- The literals that bypass `Palette` (`grep -n '#[0-9a-fA-F]\{3,8\}'
  ui/app.slint` outside the global: about 24) become tokens or
  expressions on tokens: hover and pressed overlays, modal scrims, the
  drop shadow, the icon and season-label grey, the `#ffffffe6` text,
  the tinted fills (`#f2c14e22`, `#6fcf9722`, `#ef6b6b22`, `#e8a86b1f`).
  Which are base tokens and which are derived (a base token with an
  alpha, `.transparentize()` or `.with-alpha()`) is settled here and
  noted in the milestone doc.
- A Rust type for the palette (base tokens only) with the current
  values as its default, and one function that applies it to the
  window. Nothing calls it with other values yet.

## Acceptance

- [ ] No colour literal left in `ui/app.slint` outside `Palette`.
- [ ] Screenshots of every view (main, search, guide grid, settings,
  dashboard, recordings, VOD, the refresh panel) match the ones taken
  before the change, against the demo provider.
- [ ] Test: applying a palette with a changed `bg` changes
  `Palette.bg` as read back from the window.

## Out of scope

Reading any file (02), Noctalia (03).
