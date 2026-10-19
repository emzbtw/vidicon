# 09: Scoped search

Status: done. Blocked by: 03.

## Goal

Search finds titles of the kind on screen, and all kinds when asked.

## Context

`docs/milestones/vod.md` (Requirements: search). SPEC 8.5 (search),
`src/search.rs` (in-memory index of names), v1 ticket 23. 01's title
count decides whether names stay in memory.

## Scope

- `/` in a Live, Movies or Series category searches that kind only; the
  search bar names it (`Series`). In Favourites, Recently played and
  Continue watching it searches what is in them.
- Tab in the search bar widens to all kinds, results grouped by kind,
  each group capped, live first.
- An index per kind, built as the live one is (01: about 183k movie and
  49k series names, 7 MB, fit). Measure Movies' All, four times the
  live list; if over budget, `channel_fts`-style prefiltering (8.5).
- SPEC 8.5 updated; keys panel.

## Acceptance

- [x] Tests: scope per view; widening; grouping and caps; matching as
  live search does (folding, superscripts).
- [x] Each keystroke within SPEC 10's search budget at 01's sizes.
  Measured 2026-10-03 (SPEC 8.5, `docs/perf/v1.md`): Movies' All 5 to
  16 ms with rare outliers to 18 ms on common-word queries matching 20k
  to 120k titles; widened 6 to 15 ms. The outliers are accepted (SPEC
  10).

## Notes

From 11: Favourites and Recently played hold movies and series as
`store::list_key`s (2^40 and up), beside channel row keys. The live
index has channel names only, so a search there drops the titles until
this ticket; `Action::Search` lets it through by `Category::rows`.

## Out of scope

Searching plots or cast.
