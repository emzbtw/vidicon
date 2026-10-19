# 04: Movie details and play

Status: done. Blocked by: 03.

## Goal

The right pane shows a selected movie's details, and Enter plays it.

## Context

`docs/milestones/vod.md`. SPEC 6.1 (`get_vod_info`, `/movie/` URL, from
01), 6.2, 8.2 (right pane, NOW PLAYING), 9.1 (title, never a URL).
Catch-up ticket 06 for fetching on demand behind refreshes.

## Scope

- On selection, `get_vod_info` through the client's gate behind API
  requests, the selected one first; kept until its list next refreshes.
- Right pane: poster tile (06 adds the image), title, year, length,
  genre, rating, plot, director and cast where given. Length from mpv
  while playing; `duration_secs` is unreliable (01), so check `duration`
  (`HH:MM:SS`) against mpv before showing it.
- Enter plays the movie in the Player; mpv's title is the movie's title;
  NOW PLAYING shows it with its position and length from mpv.
- Live-only controls (go live, start over, record) are hidden or say why
  while a movie plays.
- SPEC 6.1, 8.2, 9.1 updated.

## Acceptance

- [x] Tests: info parsed from 01's shapes (fields missing, numbers as
  strings); the URL against `fake_xtream`; the title never holds the URL.
- [x] Checked by hand against the demo provider and the real one.

## Notes

Done 2026-10-02, checked by the user on the real provider 2026-10-03. Info is
stored on the movie's row (migration 19, `info_at`), so the next import of
the list clears it; `src/info.rs` fetches it for the selected movie, the
latest selection first, at most 3 waiting, behind any API request, once
per import. A movie plays with its name as mpv's title, takes a
connection, and is not added to Recently played (11). While one plays,
the card shows mpv's `time-pos` (every 5 s) and `duration`, has no go
live or record button, and `L`, `S`, `R` say they are for channels.
Checked with the demo provider: `--keys` with `fake-mpv`, and the real
mpv headless (`--vo=null --ao=null`) on the demo's 5-minute `/movie/`
file, where the card read `0:00:21 / 0:05:00`. Screenshots `movies` and
`player-movie`.

On the real provider the user found the panel's length (the info's
`duration`) matching mpv's on the card and in mpv itself, so it stays.

## Out of scope

Resume (07). Downloads (10).
