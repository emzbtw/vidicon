# 04: Start over

Status: done. Blocked by: 03.

## Goal

One key plays the programme airing now from its start, on a channel with an
archive.

## Context

`docs/milestones/catch-up.md`, ticket 03. SPEC 8.2, 8.3.

## Scope

- `S` (proposed; 8.3 has it free) in the guide grid on a programme airing
  now, and in the list on the channel playing live: plays it from the
  archive from its start, as 03 plays a past one.
- Not on a channel without an archive, or with no guide: the status bar
  says why.
- The detail strip and NOW PLAYING card hint `S start over` where it
  applies. While it plays it is catch-up (03): `L` goes live.
- Reaching now: ticket 01 found an `.m3u8` ends there and did not see what
  a `.ts` does (SPEC 6.1). Check by hand first; whichever it is, the
  result is sensible: keep playing on into live, or go live by itself.
- SPEC 8.2, 8.3; keys panel.

## Acceptance

- [x] Tests: start over offered or refused (archive, none, no guide, a gap).
- [x] Checked by hand against the demo provider and the real one.

## Notes

- 2026-10-02: implemented. Checked against the demo provider with
  `--keys` and fake mpv: the card's and detail strip's start over hint,
  `S` in the grid asking for the archive from the programme's start
  (`65` minutes: 60 plus `record_after`), `CATCH-UP` while it plays, and
  the refusals in the status bar. Not yet checked: `S` in the list (it
  needs mpv running, which `--keys` replays before), `S` pressed for
  real (Shift, see the `--keys` caveat), and reaching now.
- Reaching now: the stream asks for the programme to its guide stop plus
  `record_after`. When it ends (mpv `end-file` `eof`, for the latest
  `loadfile` only, so a replaced stream ending late does not count), the
  app goes live by itself and says `CAUGHT UP: LIVE`. That covers both
  outcomes: `.m3u8` ends at now; a `.ts` that runs on plays on until its
  minutes are up.
- Checked 2026-10-02 by the user against the real provider (`.ts`): `S`
  pressed for real, then seeking forward in mpv past now, the stream
  ended at now and the app went live with `CAUGHT UP: LIVE`.
  `S` works in both the grid and the list.
- After closing: `S` inside mpv starts over too, as in the list (SPEC
  9.2), replacing mpv's `S` screenshot without subtitles (asked for by
  the user). Checked with `real_mpv_smoke`.
- `L` from catch-up or from behind live says `LIVE` in the status bar
  (asked for by the user); at live it still does nothing and says nothing.
  From catch-up, `LIVE` and `CAUGHT UP: LIVE` show only once the live
  stream has started, so a failed play keeps its error.
- The hint reads `START OVER`, not `S start over`, and the catch-up hint
  (ticket 03) now reads `CATCH-UP`, not `Catch-up · Enter plays from the
  start` (the user's choice).
- In the list `S` acts on the channel playing, not the selected one, as
  `R` does. A past programme's end is left as mpv leaves it (03).

## Out of scope

Seeking (05).
