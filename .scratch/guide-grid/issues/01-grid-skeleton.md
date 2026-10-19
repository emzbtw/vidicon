# 01: Grid skeleton

Status: done. Blocked by: none.

## Goal

`t` opens a guide grid for the current category: channels with a guide,
each with its programmes laid out on a 3 h timeline starting at now. A
first measurement of scrolling at 144 Hz, since that is the milestone's
main risk.

## Context

`docs/milestones/guide-grid.md`. Layout C on `prototype/slint`
(`prototypes/slint-ui/ui/app.slint`, `VariantC`). SPEC 5 (programme table),
6.1 (guide coverage: about 14% of channels), 7.1 (kept window), 8.2, 10.
ADR 0003 (paged queries, never the whole guide in memory), ADR 0005
(`ListView` directly; the 144 fps target).

## Scope

- `t` toggles the grid screen; it replaces the three panes, the status bar
  stays. Esc does not leave (it clears a filter elsewhere); `t` does.
- Rows: the current category's channels that have programmes in the kept
  window, in the library's order. Find them without reading every channel:
  measure an `EXISTS` against the programme key first, and only if that is
  slow, keep a flag updated by the EPG import.
- Each row: number, logo, name in a fixed column; then the programmes
  overlapping the visible 3 h, read per row as rows are built (paged, ADR
  0003). The airing one highlighted, as in the prototype.
- A time header with half-hour ticks and a line at now, both fixed while
  the rows scroll.
- `j`/`k`, arrows, page keys, `g`/`G` move between channels; Enter plays the
  selected channel; `f` and the player keys work as in the main view.
- The minute tick moves the now line and the airing highlight.
- `VIDICON_FPS=1` works in the grid (one row per frame). Measure at 144 Hz
  against the 50k demo provider and record it in `docs/perf/`. Not yet held
  to 144 fps: that is ticket 06.
- Empty category: a hint that no channel in it has a guide.
- Keys panel and status bar hints: `t`.
- Update SPEC 8.2 and 8.3.

## Acceptance

- [x] Tests: the guide-only channel query (with and without programmes,
  hidden categories, Favourites and Recently played) and the programme
  read for a row and a time span.
- [x] Screenshot `docs/screenshots/guide-grid.png`, demo data.
- [x] Scrolling measured at 144 Hz, numbers in `docs/perf/`.

## Out of scope

Moving through time, programme selection, the detail strip (02). Category
switching inside the grid (03). Mouse beyond what `ListView` gives (05).
