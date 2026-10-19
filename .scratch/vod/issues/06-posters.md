# 06: Posters

Status: done. Blocked by: 04.

## Goal

Movies and series show their posters, in the list rows and the right
pane.

## Context

`docs/milestones/vod.md`. SPEC 6.2 (logo limits per host), v1 ticket 13
and `docs/milestones/channel-logos.md`, `src/logos.rs` (cache, prune,
per-host gates). 01's findings on poster hosts and sizes.

## Scope

- Posters through the logo loader: the same cache, limits per host,
  failures falling back to the tile, at the row's and the pane's sizes.
- The cache's cap and pruning account for posters being larger and more
  numerous than logos: 01 found about 160k distinct URLs, all on third
  party hosts (one holding most), nearly all 600×900 JPEG of about
  100 KB, and a few `data:` URIs. Keep them scaled to the sizes drawn.
- Only rows on screen ask, as logos do.

## Acceptance

- [x] Tests: a poster downloads once and comes from disk after; a broken
  one falls back and is not retried.
- [x] Checked by hand against the demo provider (screenshot) and the
  real one, scrolling fast without 429s.

## Notes

Done 2026-10-03. Checked with the demo provider (`movies`, `series`,
`player-movie` and `search-all-kinds` screenshots retaken), and by the
user on the real provider 2026-10-03: a large movie list scrolled fast,
no `backing off` in the log.

Posters go through the logo loader with a `poster` flag on the request:
the same host queues and gates (so a host serving logos and posters
still gets 2 in flight), but their own cache, `posters/` beside `logos/`,
256 MB, as JPEG no taller than 240 px (160×240, about 12 KB), so about
20k fit. Logos stay PNG at 176 px. A poster is not trimmed; it is fitted
into a square `px` box, which for 2:3 gives the tile's shape. Rows,
the detail panel and NOW PLAYING (an episode shows its series' cover)
ask at their drawn heights (42, 54, 120, 44 px). `data:` URIs show the
tile: 3 of about 160k were not worth a decoder. The demo provider now
serves drawn posters from `localhost`.

## Out of scope

Backdrops; a poster grid view (milestone's C, later).
