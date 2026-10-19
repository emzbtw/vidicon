# 01: Refresh panel

Status: done. Blocked by: none.

## Goal

A refresh started with `r`, or one that fails, shows each stage's
progress in a panel that does not block the main screen.

## Context

`docs/milestones/refresh-panel.md`. SPEC 6.2 (backoff), 7.2, 8.2
(status bar, sweep line), 8.3, 8.4. `src/refresh.rs`, `src/ui.rs`.

## Scope

- `refresh` reports per stage: waiting, running with progress, done
  with a count, backoff with time left, failed with the reason. Settle
  how progress is measured (milestone Proposed).
- The panel: bottom right, above the status bar, one row per stage in
  run order.
- Opens for `r` and for a failed refresh; background ones stay on the
  sweep line. A click on the sweep line, or `r` while one runs, opens it.
- Closes by itself after a success (settle the delay); a failure stays.
  Esc or its × dismisses it; the refresh carries on.
- Keys panel (`?`) row; the demo provider can slow or fail a stage to
  show it.
- SPEC 7.2, 8.2, 8.3, 8.4 updated.

## Acceptance

- [x] Tests: stage states from a refresh with a backoff and a failure;
  opens for `r`, not for a background refresh, and for a background
  failure; dismissing does not stop the refresh.
- [x] Checked by screenshot against the demo provider, a success and a
  failure.

## Out of scope

The first run with an empty database (02). External EPG rows
(EPG sources milestone).
