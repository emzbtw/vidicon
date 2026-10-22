# 04: Hide what an M3U provider lacks

Status: open. Blocked by: 02.

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

- [ ] Tests: each capability off hides its view and disables its keys.
- [ ] Screenshots with an Xtream and an M3U demo provider, compared.
- [ ] `code-review` of the ticket's diff; findings fixed.

## Out of scope

The settings and providers views (06).
