# 04: Hide what an M3U provider lacks

Status: done. Blocked by: 02.

## Goal

With an M3U provider active, nothing on screen leads to the account,
catch-up, or movies and series.

## Context

`docs/milestones/m3u.md`. SPEC 6.3, 8.2, 8.3, 8.5. `src/ui.rs`,
`ui/app.slint`, `src/info.rs`, `src/tables.rs`.

## Scope

- Capabilities per provider (`archive`, `vod`, `account`), from its
  `Source`.
- Hidden for M3U: the account dashboard and expiry colours, the kind
  switch and movie and series results in search, catch-up marks and keys
  in the guide strip, grid and detail panel, the guide tables fetch.
  Their keys do nothing rather than error.
- Switching providers updates the screen at once.
- SPEC 8.2, 8.3, 8.5 updated.

## Acceptance

- [x] Tests: each capability off hides its view and disables its keys.
  Pure functions only (`lacking`, `key_panel`, `step_category`,
  `guide_blocks`, `status_account`); the window's views have no test
  harness, so the screenshots below check them.
- [x] Screenshots with an Xtream and an M3U demo provider, compared:
  main view, keys panel, search and manage mode. The M3U side has no
  Continue watching, kind switches, `i`, `S`, Tab or movie keys. Its
  grid has no programmes until 05, so the archive marks rest on the
  test.
- [x] `code-review` of the ticket's diff; findings fixed: the manage
  hint built once instead of three copies, `Caps` named `Capabilities`.
  Kept: `key_panel` matches its own display strings, as it did for F12.

## Out of scope

The settings and providers views (06).
