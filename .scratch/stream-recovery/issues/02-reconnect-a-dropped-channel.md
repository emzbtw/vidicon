# 02: Reconnect a dropped channel

Status: done. Blocked by: 01.

## Goal

A live channel that drops comes back by itself.

## Context

`docs/milestones/stream-recovery.md`, ticket 01's findings. Catch-up
ticket 04 for acting on `end-file` for the latest `loadfile` only. v1
ticket 14 (`L`, behind live). SPEC 6.2, 8.2, 9.1.

## Scope

- Spawn mpv with `--stream-lavf-o-append=reconnect=1`,
  `--stream-lavf-o-append=reconnect_streamed=1` and
  `--stream-lavf-o-append=reconnect_delay_max=10` (one pair each, so a
  user's own `stream-lavf-o` is kept). 01 (SPEC 9.3): this
  hides a closed connection from the app; mpv reconnects at the live
  edge and reports only a `playback-restart`. Check what that does to
  the time behind live and to a recording of what plays, and fix either
  if wrong.
- When mpv gives up (about 10 s of `paused-for-cache`, then `eof`), a
  drop (milestone Requirements) on a live channel retries the same
  channel, at the live edge, on the milestone's schedule.
- Status bar: `RECONNECTING` while it tries; the error, as today, when it
  gives up.
- A key or play by the user during retries cancels them. Stop (`s`) and
  quitting mpv are not drops.
- Not for catch-up's own end, movies or episodes, unless 01 says
  otherwise.
- SPEC 8.2, 9.1 updated.

## Acceptance

- [x] Tests: drop then recover; drop and give up; user action cancels;
  stop and catch-up's end are not drops; an older `loadfile` ending does
  not count.
- [x] Checked with the real mpv against the demo provider (`DEMO_DROP`
  for a drop, stopping the server for one mpv cannot recover), and by the
  user against the real one. Demo part done 2026-10-12 (SPEC 9.3). The
  user's run, 2026-10-12 to 18 with the debug log: mpv reconnected by
  itself 12 times, unseen; the app retried 2 drops, each back after 1
  retry in about 4 s, showing `RECONNECTING` and returning at the live
  edge. Both were a clean `eof` 600 s after a `loadfile`, the server
  closing the stream every 10 min, which `reconnect_streamed` does not
  cover. No recording spanned a reconnect; the demo check stands for it.

## Out of scope

Alternates (03, 04).
