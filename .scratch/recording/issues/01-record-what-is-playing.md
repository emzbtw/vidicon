# 01: Record what is playing

Status: done. Blocked by: none.

## Goal

One key records the channel playing in mpv, on the connection mpv already
has.

## Context

`docs/milestones/recording.md`. ADR 0002 and SPEC 9 (mpv over JSON IPC),
SPEC 8.2 (status bar, NOW PLAYING card), `src/player.rs`, `src/config.rs`.

## Scope

- `R` while something plays starts recording it: mpv's `stream-record`
  property, set over IPC, to a file under `recordings_dir`. `R` again, a
  stop, a channel switch or mpv closing ends it.
- Config: `recordings_dir` (default `$XDG_VIDEOS_DIR/Vidicon`, else
  `~/Videos/Vidicon`), created when first needed. A directory that cannot
  be written is an error in the status bar, not a crash.
- File name: channel, programme (when the guide has one) and local start
  time, e.g. `Northwind Sport 1 - Football Tonight - 2026-10-01 20.00.ts`.
  Characters a file name cannot hold are replaced; a name already taken
  gets ` (2)`.
- While recording: a red `● REC 12:04` in the status bar (elapsed, by the
  minute tick) and on the NOW PLAYING card.
- What mpv says when `stream-record` fails (e.g. a full disk) ends the
  recording with the error in the status bar.
- Keys panel (PLAYER group), SPEC 8.2, 8.3 and 9.

## Acceptance

- [x] Tests: the file name (unsafe characters, no guide, a taken name),
  and the IPC sequence against the fake socket: start, stop, stop on
  channel switch.
- [x] Checked by hand with real mpv against the demo provider: the file
  plays, and is cut off cleanly by a channel switch.

## Notes

- Done 2026-10-01. mpv 0.41 reports a failed recording only in its log
  (SPEC 9.4); a full disk could not be simulated, so that path is covered
  by the `recorder` error handling, not checked by hand.
- Changed after testing against the real provider: `stream-record`, as
  scoped above, recorded about 20 s ahead of the screen at both ends (mpv
  reads that far ahead). Replaced by `dump-cache` from the time shown, and
  a stop cutting off what was read past the picture (SPEC 9.4). Checked by
  hand against the demo provider with `DEMO_BURST=20` and its clock: both
  ends within a keyframe (0.3 s) of the screen, for `R` and for a switch.
- Changed after review: the REC counter shows seconds and redraws every
  second while recording, not by the minute tick; and `R` inside mpv
  records too (bound like the 9.2 keys, replacing mpv's subtitles-down).
- `R` works in the guide grid too, recording what plays, until 03 gives it
  the selected programme there.
- Quitting the app, or mpv closing, while recording leaves the file uncut,
  running on by what mpv had read ahead; quitting also kills mpv, so up to
  the last 1 MiB (the recorder's buffer) is lost. The file still plays.

## Out of scope

Recording a channel other than the one playing (02, 03). The Recordings
view (04).
