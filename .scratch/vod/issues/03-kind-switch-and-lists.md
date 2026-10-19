# 03: Kind switch and the lists

Status: done. Blocked by: 02.

## Goal

The library pane switches between Live, Movies and Series, and the middle
pane lists the chosen kind's titles.

## Context

`docs/milestones/vod.md` (the chosen mock). SPEC 8.2 (layout, library,
list rows), 8.3 (keys), 8.4 (mouse), ADR 0005 (`ListView` directly).
`src/ui.rs`, `ui/app.slint`.

## Scope

- Favourites, Recently played and Continue watching (empty until 07) at
  the top of the library, then a Live · Movies · Series switch, then the
  chosen kind's categories (All, Recently added, then the provider's).
  A key cycles it (settle which; 8.3 has few left); a click picks.
- Movie rows: small poster tile (initials until 06), title, rating, and
  year and length once its info is known (01: the list has neither).
  Series rows: title, year, seasons if known.
- Order (`a`) and the category keys work in every kind; the guide grid
  (`t`) stays live only.
- The right pane shows what the list rows know until 04 and 05 add the
  rest.
- Screenshots in `docs/screenshots/` against the demo provider.
- SPEC 8.2, 8.3, 8.4 updated; keys panel.

## Acceptance

- [x] Tests: the switch picks each kind's categories; rules hide a movie
  category; paged reads (no whole list in memory).
- [x] Checked by hand against the demo provider, with screenshots.
- [x] Scrolling a large list (01's count) stays within SPEC 10's budgets.

## Notes

From 02: manage mode (`e`) already has a kind switch and opens on Live;
once the library has one, open manage mode on the library's kind.
`store::categories` reads live only and needs a kind; the movie and
series tables have `*_by_category` indexes for provider order, but no
A–Z index yet.

Settled with the user 2026-10-02: Tab and Shift-Tab cycle the kind (in
the guide grid they still step categories); Recently added is the latest
200 by `added` or `last_modified`. Done here: the library is Favourites,
Recently played, Continue watching, the switch, then the kind's All
(`All channels`, `All movies`, `All series`), Recently added for movies
and series, and its categories; manage mode opens on the library's kind;
`store::categories` takes a kind; migration 18 adds the movie and series
order indexes (their import cost is in `docs/perf/v1.md`). Until 04, 05,
09 and 11, Enter, `/` and `f` on a movie or series say `… is not ready
yet` in the status bar, and `t` says the grid is for live channels.
Checked with `--keys` and screenshots against the demo provider, and by
the user on the real provider (Tab, Shift-Tab, clicks on the switch):
All movies, about 183k, opens in 35 to 63 ms, All series, about 49k, in
9 to 18 ms. The user's report: the detail panel's RECENTLY PLAYED
(channels) showed beside a movie; now hidden for movies and series, and
noted on ticket 11. Screenshots not retaken:
`player-recording`, `recordings` (they need the real `ffmpeg` and
database edits) still show the old library.

## Out of scope

Playing (04, 05). Search (09). Favourites of movies or series (11).
