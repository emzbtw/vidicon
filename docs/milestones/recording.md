# Milestone: recording

After the guide grid (`guide-grid.md`, done 2026-10-01), which is where
scheduling a programme belongs. SPEC 1 lists recording as a v1 non-goal.
Planned 2026-10-01; tickets in `.scratch/recording/`. Done 2026-10-01
(tickets 01 to 05).

## Requirements

- **Record what is playing:** mpv's `stream-record` property, set over the
  existing IPC (ADR 0002). No extra provider connection.
- **Record a programme from the guide grid,** including while watching
  something else: an `ffmpeg` child process with its own connection, which
  counts against the account's max connections (SPEC 6.3). Refuse, with a
  clear message, when it would go over.
- Recordings saved under a configurable directory, named by channel,
  programme and start time.

## Settled 2026-10-01

- **The app must be running.** Schedules live in SQLite and the app starts
  `ffmpeg` when a programme's padded start comes. One that came while the
  app was closed is marked missed, and the next launch says so. A systemd
  user timer, so recordings run with the app closed, can come later.
- **Padding:** 2 minutes before and 5 after the guide's times, by default.
  `record_before` and `record_after` in `config.toml` change them.
- **Container:** the provider's `.ts` as is. `ffmpeg -c copy` and mpv's
  `stream-record` both write `.ts`; nothing is transcoded or remuxed, and a
  recording cut off part way still plays.
- **Managing them:** a record mark on scheduled and recording programmes
  in the grid and its detail strip, a status bar indicator while anything
  records, and a Recordings view listing scheduled, recording and finished
  ones, where a recording is cancelled or its folder opened. Deleting
  one from there, to the trash, was added after ticket 04 (ticket 05).
- **Keys** (proposed, settle in the tickets): `R` records what is playing
  in the list, or toggles the selected programme in the grid; `v` opens the
  Recordings view.
- **Config:** `ffmpeg_path` (default `ffmpeg`), like `mpv_path`;
  `recordings_dir` (default `$XDG_VIDEOS_DIR/Vidicon`, else
  `~/Videos/Vidicon`).

## Open

- Nothing yet. Questions that come up go in the ticket that meets them.
