# 10: Downloads

Status: done. Blocked by: 04, 05.

## Goal

A movie, an episode or a whole season can be saved to disk.

## Context

`docs/milestones/vod.md` (Open: where downloads show and live). 01's
findings on ranges, speed and connections. SPEC 6.3 (`max_connections`),
9.4 and 9.5 (the recorder, its files, failures, quit), 8.2 (Recordings
view).

## Scope

- Settle with the user: downloads in the Recordings view and
  `recordings_dir`, or a view and folder of their own.
- `d` downloads the selected movie or episode, `D` the selected season,
  one at a time per provider within `max_connections`, queued behind
  each other. A plain HTTP GET (01: about 18 MB/s, ranges honoured, so a
  cut download resumes with `Range`). 01 could not tell whether a
  download counts: `active_cons` stayed 0 during one. The account has
  `max_connections` 1, so settle with the user whether to try a
  download beside playback once, or treat it as a connection.
- Progress, failures and cancel where they are shown; a finished file
  named from the title (and `S02E04` for an episode).
- The right pane says when a title is downloaded or downloading; playing
  a downloaded one plays the file.
- SPEC 8.2, 8.3, 9 updated; keys panel.

## Acceptance

- [x] Tests: queueing within the connection limit; names; a failure's
  note; quitting mid-download.
- [x] Checked by hand against the demo provider and the real one.
  Demo, 2026-10-04: `d` on a movie, `D` on a season, the chip and the
  episodes' marks, Enter playing the file, the Downloads tab with a
  running and queued season, a pause when a channel played on a
  1-connection account (`DEMO_CONNECTIONS=1 DEMO_RATE=8`), the resume
  from a part at launch, `x x`, `d d`; screenshots `downloads.png`,
  `recordings.png` (its header gained the tabs), and a trashed episode
  restored while the app ran getting its mark back. Real provider, by
  the user 2026-10-04: a download paused while a live channel played and
  resumed once it closed.

## Notes

Settled with the user 2026-10-04: downloads get their own tab in the
Recordings view (Tab switches, `v` opens the one last shown) and their
own folder, `downloads_dir` (`Videos/Vidicon/Downloads`), with
`recordings_dir`'s default moved to `Videos/Vidicon/Recordings` to
match. A download is treated as a connection rather than tried beside
playback on the real account; playback or a recording that needs it
pauses the download, which resumes with `Range` when one is free.

## Out of scope

Downloading from the archive (catch-up's Open).
