# 06: Grid performance pass

Status: done. Blocked by: 02, 05.

## Goal

The grid holds 144 fps at 144 Hz while scrolling, vertically and through
time, the acceptance criterion ADR 0005 set for this milestone.

## Context

ADR 0005 (prototype at 134 to 140 fps; first things to try), SPEC 10,
`docs/perf/v1.md` (method, and the main view's accepted stress-test
result). Ticket 01's first measurement.

## Scope

- Measure first: vertical stress test (one row per frame), and a new
  horizontal one (time moved every frame), at 144 Hz and 60 Hz, warm and
  cold logos, against the 50k demo provider.
- Then, in this order, stopping when the target is met: reuse per-row
  block models instead of rebuilding them; keep only the visible span's
  blocks; the Skia renderer (ADR 0005); anything the measurements point to.
- Launch, idle and memory still within SPEC 10 with the grid open.
- Record before and after in `docs/perf/`.

## Acceptance

- [x] 144 fps at 144 Hz in both stress tests, or a measured reason it
  cannot be and the user's decision on it.
- [x] Launch, idle CPU and memory with the grid open within SPEC 10.

## Out of scope

The main view's scrolling (accepted in v1 ticket 16).
