# 11: Performance pass

Status: done. Blocked by: 03, 05, 06, 12, 13.

## Goal

Prove v1 meets SPEC 10's budgets on the user's machine, and keep a way to
re-measure.

## Context

SPEC 10. ADR 0005 (prototype frame measurements). The prototype's
`PROTO_FPS=1` mode on `prototype/slint` (`FpsProbe`, auto-scroll one row per
frame, rendering notifier) is the model.

## Scope

- Debug flag (env var) that auto-scrolls and prints fps, frame-interval p50/p99
  and render time per second.
- A synthetic 50k-channel provider (fixture data or the fake Xtream server from
  ticket 08) with a matching XMLTV.
- Measure every SPEC 10 row on the 60 Hz (HDMI-A-1) and 144 Hz (DP-1) outputs;
  `umbriel msg window-move-to-output-right` moves the window between them.
- Include scrolling with logos loading (ticket 13).
- Record results in `docs/perf/v1.md`. Fix anything over budget.

## Acceptance

- [x] Every SPEC 10 budget met, or a written reason and follow-up ticket.
- [x] `docs/perf/v1.md` has the numbers and how to reproduce them.

## Outcome

Launch (median 290 ms, 6 of 20 launches over 300) and the 144 Hz stress
test (141 to 142 fps) miss their budgets; reasons in `docs/perf/v1.md`,
follow-ups in tickets 15 and 16. The rest are met. Fixed here: logos
arriving for channels already scrolled past rebuilt the detail panel.
