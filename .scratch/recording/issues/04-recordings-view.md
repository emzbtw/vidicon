# 04: Recordings view

Status: done. Blocked by: 01, 02.

## Goal

One place to see what is scheduled, recording and recorded, and to cancel
or find them.

## Context

`docs/milestones/recording.md`, SPEC 8.2 (views, keys panel, account
dashboard as a modal precedent), tickets 01 and 02.

## Scope

- `v` opens a Recordings view; `v` or Esc closes it. Three groups: now
  recording, scheduled (soonest first), finished (latest first, with
  failed, missed and cancelled ones marked). Each row: channel, programme,
  times, and its state or size.
- Keys in it: `j`/`k` move, `x` cancels a scheduled or running recording
  (a second press confirms, as `c` does for recently played), Enter on a
  finished one plays the file in mpv, `o` opens its folder (`xdg-open`).
- Recordings from 01 (what is playing) are listed with the rest.
- Empty: "Nothing recorded yet. Press R on a programme in the guide."
- The view updates as recordings start and end, without reopening.
- Keys panel note, SPEC 8.2 and 8.3.

## Acceptance

- [x] Tests: the grouping and order, and cancel on each state.
- [x] Screenshot with one of each state, demo data.

## Notes

- Done 2026-10-01. Screenshot: `docs/screenshots/recordings.png`
  (`keys.png` replaced for the new `v` row and Recordings note). SPEC 8.2
  (Recordings view), 8.3, 9.4 and 5.
- Recordings of what plays were not stored before; the recorder now
  stores them (`Command::Playing`, `PlayingEnded`) when mpv reports
  them, and sends `Event::Listed` so the view re-reads without a second
  status message. Their `stop` equals `start` until they end, which is
  how the view tells them apart (`Today from 20:04`).
- `x` on a running recording of what plays stops it as `R` does and
  keeps it as cancelled, like a running scheduled one. Cancel on a
  finished one does nothing (status bar says what `x` is for).
- Decisions the ticket left open: the view lists every provider's
  recordings; rows show the guide's times, not the padded ones; finished
  ones sort by padded start, latest first; `q` closes it too, as it does
  the other cards; Enter leaves the view open; the selection follows its
  recording when it moves group (e.g. cancelled into HISTORY).
- The ticket's finished group is headed HISTORY: it also holds
  cancelled and missed ones, which never finished (changed after review).
- The empty text names both ways to record (changed after review): the
  scoped one mentioned only the guide, though `R` also records what plays.
- Added after review: Clear, a header button and `c` (twice,
  as `c` clears recently played), forgetting done, failed, missed and
  cancelled rows. Files are kept, as deleting files stays out of scope.
  The recorder does the delete (`Command::ClearFinished`), as it owns
  the table's writes.
- Playing a recording sets no NOW PLAYING card (it is no channel), so
  `R`, `-` and mpv's PageUp/PageDown do nothing useful while it plays.
- Checked in the app against the demo provider: `x x` on a scheduled
  one moved it to HISTORY as cancelled while open; `R` with the fake
  mpv (which cannot report `time-pos`) stored a failed row with mpv's
  reason and removed the empty file. Not checked with the real mpv, and
  `o` (xdg-open) and Enter (real playback) were not run by hand.

## Out of scope

Deleting files from the app. Editing a schedule's padding per recording.
