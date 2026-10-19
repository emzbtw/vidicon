# 08: Next episode

Status: done. Blocked by: 05, 07.

## Goal

When an episode ends, the next one follows.

## Context

`docs/milestones/vod.md` (Open: play on or offer). Catch-up ticket 04 for
acting on mpv's `end-file` `eof` for the latest `loadfile` only. SPEC 8.2,
9.1, 9.2.

## Scope

- Settle with the user: play on by itself with a countdown that a key
  cancels, or only offer it.
- The next episode is the next in the season, then the first of the next
  season; none after the last.
- A key inside mpv for the next episode (9.2), if free.
- SPEC 8.2, 9.2 updated.

## Acceptance

- [x] Tests: the next episode across a season's end and at the last one;
  an `eof` from a replaced file does not count.
- [x] Checked by hand against the demo provider.

## Notes

Settled with the user 2026-10-03: play on by itself after a 10 s
countdown, shown in the status bar and on mpv's OSD; Esc in the app or
Backspace in mpv cancels it, as do playing anything else and closing
mpv. PageDown and PageUp in mpv, free while a movie or episode plays,
step to the next and previous episode (also during the countdown, so
PgDn plays the next one at once). An episode stopped part way resumes.

`neighbour` steps through the series' episodes in stored order, so a
season's end leads into the next season and there is none past either
end (`next_episode_crosses_seasons_and_stops_at_the_ends`). An `eof` from
a replaced file is already dropped in the player
(`only_the_latest_file_running_out_is_reported`, catch-up ticket 04),
and the UI only acts on one for the play mpv was handed (07).

Checked 2026-10-03 against the demo provider with the real mpv, driven
over its IPC socket: an episode seeked to its end, the next played about
10 s later; `PGDWN` and `PGUP` keypresses played the next and previous
episode; Backspace during a countdown cancelled it. A window screenshot
showed the OSD countdown over mpv's idle window.

Found by the user: the series panel kept the episode selected before a
PgUp/PgDn, and the season too across a season's end. Any episode played
now selects its season and itself in the panel when it shows that series
(`follow_episode`). Checked with the real mpv and window snapshots: PgDn
and PgUp within season 1, and S1 E9 to S2 E1 selecting the Season 2 pill.

Checked by the user 2026-10-03: the last episode's message at a series'
end, and Esc in the app and Backspace in mpv cancelling a countdown.

## Out of scope

Skipping intros or credits.
