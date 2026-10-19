# 02: Import movies and series

Status: done. Blocked by: 01.

## Goal

The store holds the provider's movie and series lists and their
categories, refreshed like the channels, under the same category rules.

## Context

`docs/milestones/vod.md`, ticket 01's findings. SPEC 5 (data model,
category rules, overrides, `shown`), 6.1, 7.1 (streaming import), 7.2
(refresh), ADR 0003. `src/import.rs`, `src/store.rs`, `src/refresh.rs`,
`src/fake_xtream.rs`, `tools/demo/demo_xtream.py`.

## Scope

- Categories gain a kind (live, movie, series); overrides apply to all
  three, and `shown` is derived for each. Rules per kind (01: the live
  rules match no movie or series category), a kind with none showing
  everything; settle with the user.
- `movie` and `series` tables from `get_vod_streams` and `get_series`,
  streamed into one transaction each as channels are (7.1), with the
  fields 01 found in the lists (SPEC 6.1). Movies: name, rating, poster
  URL, extension, added (no year or length in the list). Series: name,
  year from `release_date`, rating, cover, genre, plot, cast, director,
  `episode_run_time`, `last_modified`.
- Refresh: on its own schedule (01: about 70 MB each, against 21 MB for
  the channels; once a day proposed) and by the manual key; failures
  roll back as now.
- Manage mode (`e`) shows each kind's categories with the kind switch
  (03) or a kind column; settle which in this ticket.
- The demo provider and `fake_xtream` serve invented movies, series and
  their categories.
- SPEC 5, 7.1, 7.2 updated.

## Acceptance

- [x] Tests: import of both lists (fields as numbers or strings, missing
  ones), rules hiding a movie category, rollback on a cut body.
- [x] An import holds memory flat (7.1).
- [x] Measured against the real provider: import time, database size,
  before and after, in `docs/perf/v1.md` (rounded).

## Notes

Done 2026-10-02. Settled with the user: each kind has its own category
rules and overrides, a kind with none showing everything (channel rules
and kept tags stay live only); movies and series refresh when older than
24 h and with `r`, after channels and EPG; manage mode has a Live ·
Movies · Series switch (Tab, Shift-Tab, a click) and opens on Live.
Migration 17 puts the kind in the key of `category`, `category_override`
and `category_rule`, and existing rows become `live`. Measured on the
real provider: about 183k movies in about 6 s, about 49k series in about
3 s, the database from about 60 to about 137 MB (`docs/perf/v1.md`).
`store::categories` still reads live only; 03 gives it a kind.

## Out of scope

Anything a user sees beyond manage mode (03). Seasons and episodes (05).
