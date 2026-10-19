# 16: Detail panel cost at 144 Hz

Status: done. Blocked by: 11.

## Goal

The scrolling stress test (`VIDICON_FPS=1`) holds 144 fps on the 144 Hz
output. Today it holds 141 to 142 (median), 60 of 60 at 60 Hz.

## Context

`docs/perf/v1.md` (Scrolling). The test selects the next row every frame,
so the detail panel is refilled every frame. Most frames render in 2.4 ms,
but the ones where the panel has more text to shape take 4 to 6 ms and miss
the compositor's deadline. Without refilling the panel the same run holds
143 to 144 fps.

Real input never moves the selection 144 times a second, so this is headroom
rather than something the user sees today.

Tried in ticket 11 and not kept: updating the LATER rows in place instead of
replacing the model (about 1 fps better, and the list's scroll reset needs
another trigger).

## Scope

One of:

- Fill the detail panel once the selection has rested for a moment, so fast
  movement skips it. Changes what the panel shows while a key is held.
- Make the fill cheaper: less text to shape per change.
- Accept it and say so in SPEC 10.

## Acceptance

- [x] `VIDICON_FPS=1` on DP-1 holds 144 fps with warm and cold logos, or
      SPEC 10 records why not. Numbers in `docs/perf/v1.md`.

## Outcome

Accepted on 2026-09-30, recorded in SPEC 10. No code change.
