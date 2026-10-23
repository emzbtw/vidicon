# 02: The eight built-in palettes

Status: done. Blocked by: 01.

## Goal

The Theme picker offers eight dark palettes that ship with Vidicon,
each readable on every view.

## Context

`docs/milestones/built-in-themes.md` (the list, the contrast floor).
`docs/milestones/themes.md` (the base tokens, what each is for, and how
ticket 03 checked contrast). `src/theme.rs` (the default palette, whose
token steps are the model for surfaces).

## Scope

- `assets/themes/<name>.toml` for `catppuccin-mocha`, `dracula`,
  `everforest-dark`, `gruvbox-dark`, `kanagawa-wave`, `nord`,
  `rose-pine` and `tokyo-night`, every base token set, compiled in
  with `include_str!` and added to ticket 01's built-in list. Picker
  labels: `Catppuccin Mocha`, `Dracula` and so on.
- The colours come from each project's published palette (its repo or
  site), fetched in this ticket, never from memory. Each file's header
  names the source and its licence. Settle, per family, which of its
  colours is `surface`, `surface-2`, `line`, `logo-backing`, `dim`,
  `icon` and `accent`; record the mapping in the milestone doc.
- Contrast: a test computes the WCAG ratios for every built-in against
  the floor in the milestone doc, so a later edit cannot break one.
- Look at every view under each palette against the demo provider
  (`--screenshot` with `--keys`): main, search, guide grid, Settings,
  the refresh panel and a modal. A token that reads wrong, though above
  the floor, takes another colour from the same palette.

## Acceptance

- [x] Eight files, each naming its source and licence; all eight parse
  with no missing token (a test).
- [x] The contrast test passes for all eight.
- [x] Every view looked at under each palette; changes and reasons in
  the milestone doc.

## Out of scope

Other variants (Latte, Storm, Moon, Dawn and the rest). Docs and
screenshots (03).
