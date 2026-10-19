# 05: Delete recordings

Status: done. Blocked by: 04.

## Goal

Free disk space from the Recordings view: move a finished recording's
file to the trash and drop its row, in one step.

## Context

`docs/milestones/recording.md`, SPEC 8.2 (Recordings view, Clear), ticket
04. Deleting in the file manager after `o` leaves the row reading `File
missing`, so cleanup takes two places; the view shows sizes, which is
what prompts it.

## Scope

- `d` on a recording in HISTORY arms its delete: the row reads `Press d
  again to delete` in amber, the status bar `Press d again to move <file
  name> (1.2 GB) to the trash`. A second `d` within 4 s confirms; any
  other key, or the 4 s passing, disarms (as `x` and `c` do).
- Confirmed: the file goes to the user's trash, per the freedesktop trash
  spec (`$XDG_DATA_HOME/Trash`, with its `.trashinfo`), so it can be
  restored; then the row is forgotten. Status bar: `Moved <file name> to
  the trash`. Nothing is ever unlinked outright.
- Only HISTORY: nothing for scheduled or running recordings (`x` cancels
  them). A row with no file, or whose file is gone, is just forgotten,
  saying so.
- Only files under `recordings_dir`: a row whose file is elsewhere (the
  setting changed since) is refused with a status bar message, not
  trashed.
- A failure (trash on another filesystem, permissions) leaves the file
  and the row as they were, with the error in amber.
- The recorder does the row delete, as it owns the table's writes; the
  view re-reads as it does for Clear.
- Keys panel note, SPEC 8.2 and 8.3.

## Acceptance

- [x] Tests: the trash move (file and `.trashinfo` land, original path
  recorded, a taken name in the trash gets a new one), refusal outside
  `recordings_dir`, refusal for scheduled and running ones, a missing
  file only dropping the row, a failed move keeping both.
- [x] Checked by hand: a deleted recording shows in the desktop's trash
  and restores to its folder.

## Notes

- Done 2026-10-01; checked by hand against the desktop's trash, a
  restored file showing in the view again. SPEC 8.2 and 8.3. Screenshots `recordings.png` and
  `keys.png` replaced for the new hint and Recordings note.
- No crate: `src/trash.rs` implements the spec's home-trash case (the
  `.trashinfo` made with `create_new` first claims the name, then a
  rename; a taken name becomes `name (2).ts`, as `reserve_recording`
  names files). A cross-device rename fails with `it is on another
  filesystem than the trash`.
- `recorder::deletable` holds the checks (finished, has a file, under
  `recordings_dir` with the folders resolved), so the first `d` refuses
  at once and the recorder applies the same rules on `Command::Delete`,
  answering `Event::Deleted(row, outcome)`.
- Decisions the ticket left open: a row with no file, or a gone file,
  is armed too (`Press d again to remove … from the list; its file is
  gone`), so nothing leaves the list on one key; a refusal is a plain
  status message, only a failed move is amber. The keys panel's
  Recordings note was reworded to fit with `d` (`twice: x cancels, d
  moves its file to the trash, c clears the finished ones`).
- Changed after review: a restore from the trash did not bring the row
  back, as it was forgotten. Now `d` marks it `trashed` (migration 12)
  and the view hides it only while its file is gone, so a restored file
  shows again on the next re-read. A new recording taking a trashed
  row's file name forgets that row, so it never shows another file.
- Checked in the app against the demo provider: `d` on a scheduled row
  and on a file outside `recordings_dir` refused; `d d` on a done one
  moved it to `$XDG_DATA_HOME/Trash/files` with its `.trashinfo` and
  hid the row, the selection moving to the next; moving the file back
  and reopening the view showed it again.
- `gio trash` is not on the system; use the `trash` crate (check it
  follows the spec and adds little) or a small implementation of the
  spec's home-trash case.

## Out of scope

Deleting several at once. Emptying the trash. Trash on other volumes
(`$topdir/.Trash`): a file on another filesystem fails with a message.
