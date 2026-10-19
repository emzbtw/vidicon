# 02: First-run refresh screen

Status: done. Blocked by: 01.

## Goal

With an empty database the first refresh shows full-window, then hands
over to the main screen once channels land.

## Context

`docs/milestones/refresh-panel.md`, ticket 01. SPEC 7.2 ("loading
channels" state).

## Scope

- The panel's rows, full-window and in large type, in place of the
  "loading channels" state.
- Once the channels stage is done, the main screen shows and the
  remaining stages carry on in the panel.
- A failure before channels land stays full-window with the error and
  `r` to retry.
- SPEC 7.2, 8.2 updated.

## Acceptance

- [x] Tests: empty database shows the screen; channels done hands over;
  a failure before then stays.
- [x] Checked by screenshot against the demo provider with a fresh
  database.

## Out of scope

No credentials file at all (onboarding milestone).
