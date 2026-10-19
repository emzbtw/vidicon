# 05: Seek in catch-up

Status: done. Blocked by: 03.

## Goal

Skip back and forth within a catch-up programme.

## Context

`docs/milestones/catch-up.md`, ticket 01's findings on seeking, ticket 03.
SPEC 9.2 (keys inside mpv).

## Scope

mpv seeks in the archive natively (SPEC 6.1, ticket 01), so this ticket
only checks that mpv's own seek keys work in a catch-up programme and
that the NOW PLAYING position follows (from guide times and `time-pos`,
ticket 03), then closes with a note.

## Acceptance

- [x] Seeking back and forward 30 s and 5 min works in a catch-up programme,
  checked by hand against the real provider.

## Notes

- Checked 2026-10-02 by the user against the real provider: mpv's own
  seek keys work in a catch-up programme and the NOW PLAYING position
  follows (`time-pos`, asked every 5 s, ticket 03). No code changed.

## Out of scope

Seeking in live streams.
