# 05: Measure blackouts

Status: done. Blocked by: none.

## Goal

Know whether the app can tell a channel showing a blackout or holding
picture from a programme, before any code depends on it.

## Context

`docs/milestones/stream-recovery.md` (Open, Blackouts). SPEC 9.3: in
ticket 02's run a blacked-out channel played as normal, the provider
closed it after about 600 s, and the retry got the same picture. Earlier
verify tickets (01, catch-up 01, VOD 01) for how probes and write-ups
are done; no names, hosts or URLs in output, docs or commits.

## Scope

- Whether mpv can report a black picture: ffmpeg's `blackdetect` as an
  mpv `--vf=lavfi=...` filter, read over IPC (`vf-metadata` or the log),
  and what it costs in CPU.
- Whether a holding picture (a still slate, not black) can be told:
  frozen frames (`freezedetect`), the stream's bitrate, or nothing.
- Against the user's provider when a blackout happens: the picture's
  shape (black, slate, with or without sound), whether it is every 600
  s, and whether an alternate (03) played at the time.
- False alarms: dark scenes and fades on a normal channel over an
  evening.
- Results in SPEC 9.3 as `Verified against the user's provider
  (2026-10-xx, stream recovery ticket 05)`, rounded, and the milestone
  and ticket 04 updated with what can be detected.

## Acceptance

- [x] Each question above answered in SPEC, or marked unanswerable with
  why. `--probe-blackout` built and checked against the demo provider
  2026-10-18 (`DEMO_BLACKOUT`, SPEC 9.3): the filters work, and need
  `hwdec=auto-copy` with the user's `hwdec=auto`. The user's blackout
  run, 2026-10-18: black, still and silent, closed after 601 s, no
  alternate, 5 % to 10 % of a core. A normal channel for 44 min: no
  span still for 5 s or silent for 2 s, 46 single black frames at
  cuts, 6 % to 13 % of a core. The blackout's bitrate was not recorded
  and is not needed (SPEC 9.3).
- [x] The milestone and ticket 04 updated.
- [x] No names, hosts or credentials in logs, docs or commits.

## Out of scope

Acting on a blackout (04).
