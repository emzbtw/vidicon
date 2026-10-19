# 07: Resume and Continue watching

Status: done. Blocked by: 04, 05.

## Goal

A movie or episode stopped part way resumes where it stopped, and
Continue watching lists what is part way through.

## Context

`docs/milestones/vod.md`. 01's findings on mpv `time-pos`, `duration`
and starting at a position. SPEC 5, 8.2, 8.3, 9.1, 9.2 (keys inside mpv).

## Scope

- Store the position per movie and episode, from `time-pos` every few
  seconds and on stop or quit (01: it starts at 0; `loadfile ...
  start=<secs>` resumes); watched past about 90 % or the last 5
  minutes (settle the rule).
- Enter resumes; a key plays from the start (settle which). The right
  pane's resume card says where, and how much is left.
- List rows show progress or Watched; episodes show a watched mark.
- Continue watching (library, above the kind switch): part-watched movies
  and each series' next episode, latest first.
- SPEC 5, 8.2, 8.3 updated; keys panel.

## Acceptance

- [x] Tests: position saved and read back; the watched rule at its edges;
  a series' next episode (end of season, last episode).
- [x] Checked by hand against the demo provider and the real one: stop
  part way, quit, relaunch, resume.

## Notes

Settled with the user 2026-10-03: watched past 90 % or in the last 5
minutes, whichever comes first (capped at a quarter of the length, so the
demo's 5-minute file is not watched as it starts); `S` plays from the
start (on the selection, and seeks what plays back to 0, also inside
mpv); Enter on a series plays its next episode in Continue watching only,
elsewhere it goes into the episodes, which now open on the next one.

Migration 21 adds `progress`, by stream id so it outlives imports. Saved
every 5 s while playing, on stop, on a switch, at mpv's end and on quit
(written directly after the event loop, as the writes task is not waited
for). Positions under 60 s are 0 and keep the row's time, so a glance at
an old episode is not the series' latest. mpv's answers are tied to the
play they belong to (`Event::Handed`), so a late `time-pos` of the
previous title is never saved against the next. Continue watching keys
mix movies and series (`store::continue_key`, a series' row key negated).

Checked with the demo provider and `fake-mpv` (now honouring `start=` and
`seek`): a movie saved every 5 s and on stop (mpv killed), resumed from
it; an episode saved with its series, season and number; one resumed near
its end saved as watched, and Continue watching then read `Next: S1 E2`,
Enter there playing it; `S` on the movie played it from the start. Quit
(`q`) saving was not exercised: `--keys` cannot wait for a position.
Screenshots `continue-watching` (new) and `keys` retaken.

Checked by the user on the real provider 2026-10-03. Asked then: the
cards to match live's, so the RESUME card and a movie's NOW PLAYING card
have `START OVER` at the heading's right in place of the key hints, and
playing no longer says `Resuming. S plays it from the start`.
Screenshots `continue-watching` and `player-movie` retaken.

Also asked then: a Clear button on Continue watching, as Recently played
has (`c` twice, or the button). Settled with the user: it empties the list
only, keeping positions and watched marks; a title played again comes
back. Stored as a time per provider in `app_state`. Checked with the demo
provider: armed, cleared, still empty after a relaunch. `keys` retaken.

And: `w` to mark watched or not. Settled with the user: the selected
movie; on a series in the list, the season its panel shows; in the
episodes, the selected one. A partly watched season becomes all watched,
only an all-watched one goes back. Unwatching deletes the row (resume
point too); of several marked at once the furthest on is the series'
latest. Checked with the demo provider: season partly watched to all
watched, back to none, one episode; the panel then opened on the next.
`keys` retaken.

And: the kind switch (a click or Tab) now always opens on the kind's All,
from Favourites, Recently played or Continue watching too, and picking
the kind already chosen there goes to its All; it used to keep those
views selected. Fixed on the way: a channel's panel after a series kept
the series' meta line and plot (`detail-series` was never reset).

## Out of scope

Next episode playing on (08).
