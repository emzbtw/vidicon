# Milestone: catch-up

After recording (`recording.md`, done 2026-10-01). Builds on the guide grid
(`guide-grid.md`), which is where a past programme is picked. SPEC 1 and 2
list catch-up as not v1. Planned 2026-10-02; tickets in
`.scratch/catch-up/`. Done 2026-10-02 (tickets 01 to 06).

## Why now

Measured 2026-10-02 against the user's provider: about 1k of about 58k live
streams have an archive (`tv_archive` 1), nearly all kept 3 days, a few 7.
Thin overall, but concentrated where the user watches: about 300 of the
about 18k channels shown after the allowlist, and 12 of the 27 in Recently
played. 10 of those 12 have past programmes in the stored guide; of the
about 300, about half have any guide at all.

## Requirements

- **Play a past programme** from the guide grid, on a channel with an
  archive, from the provider's archive, from its start.
- **Start over:** play the programme airing now from its start.
- **Know which channels have one:** the import keeps `tv_archive` and
  `tv_archive_duration` (dropped today), so the grid can tell a playable
  past programme from one that is gone.
- **A guide that reaches back:** today the import keeps programmes that
  ended up to 1 h ago (7.1). Channels with an archive need theirs back to
  the archive's start, or there is nothing to pick.
- **Same politeness (6.2)** and the same single Player (ADR 0002): an
  archive stream replaces what mpv plays, on its one connection.

## Proposed (settle in the tickets)

- **Schema:** `channel.archive_days INTEGER NOT NULL DEFAULT 0`, from
  `tv_archive_duration` when `tv_archive` is 1, else 0.
- **Guide window:** keep past programmes only for channels with an archive,
  back `archive_days`; every other channel keeps the 1 h it has now. About
  1k channels at most, so the database grows a little, not by days of the
  whole guide. `xmltv.php` reaches only about a day back (SPEC 6.1, ticket
  01), so that is what the import gives (ticket 02); older days come from
  each channel's `get_simple_data_table`, fetched on demand for the rows
  on screen and kept by later imports until they leave the archive
  (ticket 06).
- **Grid:** time goes back as far as the furthest archive in the
  category, not 1 h. A past programme that is playable has a mark (an
  archive glyph at its top right, like the record ring) and the detail
  strip says `Catch-up · Enter plays from the start`; one past the archive,
  or on a channel without one, is dimmed and Enter says why in the status
  bar.
- **Playing:** Enter on a playable past programme plays it from the
  archive. The card's heading reads `CATCH-UP` (as it reads `BEHIND LIVE`
  today, SPEC 8.2) with the programme and its times, and the status bar
  says so; `L` (go live) returns to the live stream, as when behind live.
- **Start over key:** proposed `S` (SPEC 8.3 has it free): in the grid on
  the programme airing now, and in the list on the channel playing.
- **Archive URL** (SPEC 6.1): the `/timeshift/` form with the live
  stream's `ext`, the start in the server's local time (its offset from
  the account check's `time_now`), `minutes` covering the programme. mpv
  seeks in it natively. Its `duration` is meaningless for `.ts`, so
  positions come from the programme's guide times.
- **mpv title:** `<num> <channel name>` as now, plus ` · <programme>` for
  catch-up, so the window says what is replayed. Still never a URL (9.1).

## Open

- Settled in ticket 04: a `.ts` start over ends at now, as `.m3u8` does,
  and the app goes live by itself (SPEC 6.1).
- Settled in ticket 03: a programme on the far side of a daylight saving
  change from the account check starts an hour off, accepted (SPEC 6.1).
- Archive channels with no guide (62 of the about 300 shown) stay out of
  the grid: their guide tables are empty too (SPEC 6.1, checked
  2026-10-02). Catch-up by time, without a programme, would be a feature
  of its own.
- Recording from the archive (a download of a past programme) is not
  planned here; it can come later, on the recorder.
