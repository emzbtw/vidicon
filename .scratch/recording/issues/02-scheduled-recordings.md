# 02: Scheduled recordings

Status: done. Blocked by: none.

## Goal

The recorder behind scheduling: a stored schedule, and an `ffmpeg` process
that records a programme on its own connection when it comes, within the
account's connection limit.

## Context

`docs/milestones/recording.md`. SPEC 5 (schema), 6.2 (politeness), 6.3
(max connections), 7 (refresh), ADR 0003 (SQLite), `src/player.rs` for
how a child process is run.

## Scope

- Schema: a `recording` table (provider, stream id, guide id, programme
  title, start and stop, padded start and stop, state, file, error).
  States: scheduled, recording, done, failed, missed, cancelled.
- Padding from `record_before` and `record_after` in `config.toml`
  (minutes, defaults 2 and 5).
- The recorder: at a padded start, `ffmpeg -i <stream url> -c copy` to a
  `.ts` under `recordings_dir` (the name as in 01), stopped at the padded
  stop. `ffmpeg_path` in `config.toml`, default `ffmpeg`. Checked on the
  minute tick and a timer for the next start, not polled.
- Connections: a recording needs one. Refuse to start, and say why, when
  mpv's stream plus running recordings would exceed `max_connections`
  (6.3); unknown limits count as 1. A refused start is failed, not
  retried silently.
- ffmpeg exiting early, or failing to start, is failed with its last error
  line kept. Nothing is retried in this ticket.
- On launch: schedules whose padded start passed while the app was closed
  are missed, and the status bar says how many. One still inside its time
  starts late.
- Quitting while recording asks nothing: ffmpeg is stopped and the
  recording is kept as done (cut short), noted in its row.
- A guide re-import moving a programme's times moves an unstarted
  schedule with it, matched by channel and title; no match leaves it.
- SPEC 5, 6.3, 7 and a new SPEC section for recording.

## Acceptance

- [x] Tests: state changes for each path (start, finish, early exit,
  refused for connections, missed on launch, late start, cancel), padding,
  and a re-import moving a schedule. ffmpeg faked as `fake-mpv` fakes mpv.
- [x] A real recording from the demo provider plays back.

## Out of scope

Scheduling from the UI (03), listing them (04). Series or repeat rules.
Running with the app closed (a later systemd timer).

## Notes

- Done 2026-10-01. The recorder is `src/recorder.rs`, SPEC 9.5.
- ffmpeg is faked by shell scripts the tests write, which record their
  arguments and the SIGTERM that stops them. The ignored
  `real_ffmpeg_records_the_demo_provider` records 8 s from the demo
  provider with the real ffmpeg; the file probed as MPEG-2 video and MP2
  audio, 8.1 s, decoding without errors, its last frame showing the
  stream time it ended at.
- Checked in the app against the demo provider with schedules written
  into the database: one passed was missed (the status bar said so), one
  airing started late, and quitting kept it as done with its note.
- Decisions the ticket left open: a re-import follows a schedule only to
  a programme starting within 3 hours of the old start (further off is
  more likely another showing); `max_connections` of 0 or less counts as
  unknown (1); the file is named with the guide's start, not the padded
  one; a cancelled running recording keeps its file. The column the
  ticket calls `error` is `note`, as it also says why a done one was cut
  short.
- `Command::Schedule` and `Command::Cancel` are sent only by tests until
  tickets 03 and 04.
