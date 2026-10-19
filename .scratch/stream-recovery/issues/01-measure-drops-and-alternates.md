# 01: Measure drops and alternates

Status: done. Blocked by: none.

## Goal

Know what a real drop looks like and how many channels have an alternate,
before any code depends on it.

## Context

`docs/milestones/stream-recovery.md`. SPEC 3 (`name_norm`, `channel.shown`),
6.2, 9.1. `src/player.rs` (`end-file` handling). Earlier verify tickets
(catch-up 01, VOD 01) for how probes and write-ups are done; no names,
hosts or URLs in output, docs or commits.

## Scope

- **Alternates:** from the local database only, no provider requests. For
  shown channels, Favourites and Recently played: how many have at least
  one other shown channel with the same `name_norm`, and the spread
  (1, 2, 3+). A devtools flag (e.g. `--probe-alternates`) printing counts
  only.
- **Drops:** log, at `debug`, each `end-file` for a live channel with its
  reason, how long it played, and whether and how long it was paused
  before. The user runs with it for a few days of normal use.
- **Pause:** by hand, pause a channel for 1, 5 and 15 min, resume, and
  note what mpv does (plays on, stalls, `eof`, `error`).
- **mpv's reconnect:** with `stream-lavf-o=reconnect=1,
  reconnect_streamed=1,reconnect_delay_max=...`, does a drop recover
  without the app? Does it hide the drop from the app?
- **Movies, episodes, catch-up:** whether they drop too (milestone Open).
- Results in SPEC 6.1 (or 9.1) as `Verified against the user's provider
  (2026-10-xx, stream recovery ticket 01)`, rounded.

## Acceptance

- [x] Each question above answered in SPEC, or marked unanswerable with why.
- [x] The milestone's Proposed section and tickets 02 to 04 updated where
  an answer changes them (e.g. 02 shrinks to mpv options, or 03 and 04 to
  little if few channels have alternates).
- [x] No names, hosts or credentials in logs, docs or commits.

## Out of scope

Any user-facing change. The probe and the debug logging may stay if small.
