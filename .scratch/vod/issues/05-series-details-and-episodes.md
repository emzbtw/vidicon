# 05: Series details and episodes

Status: done. Blocked by: 03.

## Goal

The right pane shows a selected series' seasons and episodes, and an
episode plays.

## Context

`docs/milestones/vod.md` (the chosen mock: season pills, episode list).
SPEC 6.1 (`get_series_info`, `/series/` URL, from 01), 8.2, 8.3, 8.4.

## Scope

- On selection, `get_series_info` as 04 fetches a movie's; seasons and
  episodes stored for that series. Season pills from `episodes`' keys,
  not `seasons`, which lists seasons the provider may not have (01).
  An episode's `title` is the provider's long one; its `id` is the
  stream id, not `info.id`.
- Right pane: poster tile, title, years, seasons, genre, rating, plot;
  season pills; the season's episodes with number, title and length.
- Keys and mouse to move into the episode list, change season, and play
  an episode (settle the keys; Enter on a series could play its next
  episode once 07 knows it).
- NOW PLAYING shows the series, `S2 E4` and the episode title; so does
  mpv's title.
- SPEC 8.2, 8.3, 8.4, 9.1 updated; keys panel.

## Acceptance

- [x] Tests: info parsed from 01's shapes (seasons without episodes,
  episode numbers as strings); episode URLs against `fake_xtream`.
- [x] Checked by hand against the demo provider and the real one.

## Notes

Done 2026-10-03, checked by the user on the real provider 2026-10-03.
`src/info.rs` now fetches a series' info as it does a movie's (same queue, once per import
of the series list); episodes go in a new `episode` table and the
season count and latest year on the series' row (migration 20), all
cleared by the next series import. A failed fetch says so in the panel.

Keys settled: Enter (or a double click) on a series moves keys into its
episodes; there `j`/`k` and the page keys move, `h`/`l` change season,
Enter plays, Esc goes back. The Player's keys and `-`, `r`, `i`, `v`,
`?`, `q` still work there; any other key goes back to the list and acts.
A click on an episode enters, a click in the list leaves. When 07 knows
the next episode, Enter on a series could play it instead, with another
key to go in (not settled here).

The episode's name is taken as what follows the title's `S01E02` mark;
the real titles' shape was only described in 01 ("longer than the
episode's name"). On the real provider the names read right.

Checked with the demo provider and `fake-mpv` via `--keys`: loading,
no episodes, 3 and 4 seasons (`S1` pills past 3), `G`, play (mpv titled
`Silver Signal · S2 E2 · The Night Ferry`). Screenshots `series` and
`keys` retaken. The NOW PLAYING card's position is now asked every
second, not every 5 s, for movies, episodes and catch-up alike.

## Out of scope

Watched marks and resume (07). Next episode (08). Downloads (10).
