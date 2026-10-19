# 03: Record from the grid

Status: done. Blocked by: 02.

## Goal

Schedule or cancel a programme's recording from the guide grid, and see
which ones are set.

## Context

`docs/milestones/recording.md`, SPEC 8.2 (guide grid, detail strip), the
guide grid tickets in `.scratch/guide-grid/`. `docs/perf/guide-grid.md`:
keep the grid's frame budget (no bindings on the offset inside blocks).

## Scope

- `R` on a programme in the grid schedules it, or cancels it if
  scheduled. On the programme airing now it starts recording at once, to
  its padded end. A past programme or a gap: a status bar message, nothing
  scheduled.
- A refused schedule (connections, 02) says so in the status bar.
- Marks: a red dot on scheduled programmes' blocks, a filled one while
  recording, kept off the per-block layout (a block field the row read
  sets, as `airing` is).
- The detail strip says `Recording` or `Scheduled to record` with the
  padded times.
- The keys panel GUIDE group, SPEC 8.2 and 8.3.

## Acceptance

- [x] Tests: what `R` does on a future, airing and past programme and on a
  gap; the marks from the stored states.
- [x] Screenshot of a scheduled and a recording programme, demo data.
- [x] The grid's two stress tests (`docs/perf/guide-grid.md`) no worse.

## Notes

- Done 2026-10-01. Screenshot: `docs/screenshots/guide-recording.png`
  (`keys.png` replaced for the new GUIDE row). Stress tests in
  `docs/perf/guide-grid.md`.
- The recorder now reports `Scheduled` when it stores a schedule, so the
  grid redraws its marks and the status bar says the padded times. A
  programme scheduled or recording already is not scheduled twice (a
  second `R` before the first was stored).
- Marks: a red ring (scheduled) or filled dot (recording) at a block's
  top right; the title is narrowed to leave room. Read once per grid
  refresh for the provider, not per row (see the perf note).
- `R` on the airing programme toggles a scheduled recording on its own
  connection, not mpv's `stream-record` (01). With the grid open, `R`
  no longer records what plays; `R` inside mpv still does. In the grid
  the status bar shows the playing channel, not the `R` hint, so no hint
  changed.
- Past: a programme whose cell (cut where the next starts) has ended, by
  the cell, not the padded stop.

## Out of scope

Recording from the main list's LATER entries. The Recordings view (04).
