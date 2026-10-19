# 03: Play a past programme from the grid

Status: done. Blocked by: 01, 02.

## Goal

In the guide grid, Enter on a programme that has aired, on a channel with
an archive, plays it from its start.

## Context

`docs/milestones/catch-up.md`. SPEC 6.1 (archive URL, from 01), 8.2 (guide
grid, detail strip, NOW PLAYING, status bar), 8.3 (keys), 9.1 (title, never
a URL), CONTEXT.md (Catch-up, Behind live). `src/player.rs`, `src/ui.rs`,
`ui/`.

## Scope

- Grid time goes back to the furthest `archive_days` among the category's
  channels (1 h if none), with `<` and `n` as now. Until ticket 06, days
  the stored guide does not reach show as gaps.
- The archive URL (SPEC 6.1): `/timeshift/`, the live `ext`, the start in
  the server's local time. The offset comes from the account check's
  `server_info.time_now` against the clock, rounded to 15 min, kept per
  provider. `minutes` covers the programme, plus `record_after` for a
  guide running late.
- A playable past programme (ended, within its channel's archive) has an
  archive mark at its top right; other past programmes are dimmed. The
  detail strip adds `Catch-up · Enter plays from the start`.
- Enter on a playable one plays it from the archive. On an unplayable past
  one, nothing plays and the status bar says why (`No archive on this
  channel`, `Older than this channel's 3-day archive`). Enter on now or
  later plays live, as today.
- While catch-up plays: the NOW PLAYING card's heading reads `CATCH-UP`
  (as `BEHIND LIVE · 2 min` does, 8.2), with the programme and its times,
  and the status bar names it compactly; mpv's
  title adds ` · <programme>`. Position and progress come from the
  programme's guide times and mpv's `time-pos`, never mpv's `duration`
  (meaningless for `.ts`). `L` goes live on the same channel. History
  records the channel as for a live play.
- `R` (9.4) while catch-up plays: says recording a replay is not supported
  yet, rather than recording it.
- SPEC 8.2, 8.3, 9.1 updated; keys panel.

## Acceptance

- [x] Tests: playable or not (ended, in window, edge of window, no
  archive); the archive URL from 01's form, against `fake_xtream`; the
  title never holds the URL.
- [x] Checked by hand against the demo provider: mark, dimming, playing,
  `L` back to live. Screenshot `docs/screenshots/guide-catch-up.png`.
- [x] Checked by hand against the real provider: a programme from
  yesterday plays from its start.

## Notes

- Done 2026-10-02; checked against the real provider by the user. The URL
  helpers (`wall`, `url_time`, `server_offset`, `archive_request`) moved
  into `src/xtream.rs`, the probe using them from there. The offset comes
  from the provider's latest account check (`Source::info`), not stored:
  each launch makes one.
- The grid's reach is `Screen::guide_back`, read per category from
  migration 14's partial covering index over channels with an archive
  (0.5 ms against 19 ms reading every channel, on a copy of the real
  database with about 1k archives simulated).
- Not in the scope but needed for it: with a 7-day reach in All, a
  channel without an archive has a gap of days before its guide. As one
  cell, `h` onto it jumped the span 7 days back and the selected gap drew
  as a black bar (wider than the renderer takes). Gaps longer than the
  span are now cut at its multiples (`push_gap`). The same held for the
  gap after a guide that ends early, 3 days ahead.
- Daylight saving: settled as accepted (SPEC 6.1, the milestone's Open).
- The demo provider serves `/timeshift/` as the test pattern labelled
  `ARCHIVE` and the start; its server clock is UTC, so the offset path is
  tested against `fake_xtream`, whose clock runs at UTC+2.
- `R` in the grid still schedules the selected programme (that one has
  ended, so it says so); the refusal is `R` on what plays.
- A double click in the grid does what Enter does (SPEC 8.4).

## Out of scope

Starting the programme airing now from its start (04). Seeking (05).
Catch-up from the list's detail panel.
