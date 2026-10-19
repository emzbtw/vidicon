# 10: Provider switcher

Status: done. Blocked by: 03.

## Goal

Several providers in the credentials file; one active at a time; `p` switches.

## Context

SPEC 2 (v1 scope), 4.1 (`[[provider]]` entries), 4.2 (`active_provider`).
ADR 0001.

## Scope

- Load every `[[provider]]`; `id` must be unique (error otherwise).
- `active_provider` in `config.toml` picks the start provider; default first.
- `p` opens a small picker (keyboard and mouse); switching swaps the whole view
  to that provider's data and remembers the choice.
- Each provider refreshes independently (ticket 08 logic per provider).

## Acceptance

- [x] Two providers configured: switching is instant from cached data.
- [x] Favourites and last refresh are per provider.
- [x] Duplicate `id` gives a clear error.

## Out of scope

Adding providers from the UI.
