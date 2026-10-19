# 04: Search in the grid

Status: done. Blocked by: 03.

## Goal

`/` in the grid filters its channels by name, as it does in the main view.

## Context

SPEC 8.5 (fuzzy channel search, nucleo, best match first), `src/search.rs`.
Ticket 03's categories.

## Scope

- `/` opens a search field above the grid; typing filters the category's
  guide channels live; Enter keeps the filter, Esc clears it, as in the
  main view (SPEC 8.5). Results best match first.
- While typing, printable keys type (so `t`, `n`, `[` and the jumps do not
  act); arrows, page keys and Enter work.
- The kept filter survives time moves and is dropped by a category switch
  or leaving the grid.
- Per keystroke within the search budget (SPEC 10: under 16 ms) at 50k.
- Keys panel note and SPEC 8.2, 8.5.

## Acceptance

- [x] Tests: the filter over guide channels only.
- [x] Per keystroke timing at 50k in `docs/perf/`.
- [x] Screenshot of a kept filter, demo data.

## Out of scope

Searching programme titles.
