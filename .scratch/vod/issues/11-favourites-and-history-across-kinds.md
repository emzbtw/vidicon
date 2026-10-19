# 11: Favourites and history across kinds

Status: done. Blocked by: 03.

## Goal

A movie or series can be a favourite, and Recently played includes what
was watched.

## Context

`docs/milestones/vod.md`. SPEC 5 (`favourite`, `history`, the name
fallback when ids change), 8.2, 8.3. 01's findings on whether ids collide
across kinds.

## Scope

- `favourite` and `history` gain a kind; existing rows are live. 01
  found series ids colliding with about 1k live and 13k movie ids, so
  the kind is part of the key.
- `f` on a movie or series toggles it; Favourites and Recently played
  list all kinds, each row drawn for its kind; `J`/`K` reorder across
  them.
- The name fallback (5) applies per kind.
- `-` (last channel) stays live only.
- SPEC 5, 8.2, 8.3 updated.

## Acceptance

- [x] Tests: the migration keeps existing favourites and history; ids
  that collide across kinds stay apart; the name fallback per kind.
- [x] Checked by hand against the demo provider.

## Notes

From 03: the detail panel's RECENTLY PLAYED (the last 4, click plays)
is hidden while a movie or series is selected, as it lists channels
only. Settle whether it comes back there once history spans the kinds.

Settled: it stays channels only, the last 4 for zapping back as `-`
does, so it stays hidden for a movie or series (SPEC 8.2).

Done: migration 22 adds `kind` to both keys, old rows live, rowids kept
for history's tie-break. Favourites and Recently played hold
`store::list_key`s, as Continue watching now does: a channel's key is
its row key, movies and series from 2^40 and 2^41, so channel code
(surf, the grid, `channel_position`) reads them unchanged. A series is
played when any of its episodes is sent to mpv. The header counts
`titles` once a row is not a channel. Search there filters channels
only until 09. Checked against the demo: a channel, two movies and a
series in Favourites, `K` moving a movie between channels, a movie
play topping Recently played.

## Out of scope

Named favourite groups (parked, SPEC 2).
