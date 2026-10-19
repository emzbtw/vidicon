# 01: Verify movies and series against the provider

Status: done. Blocked by: none.

## Goal

Know, from real requests, how the provider serves movies and series,
before any code depends on it.

## Context

`docs/milestones/vod.md`. SPEC 6.1 (endpoints, and how earlier
verifications were written up), 6.2 (politeness), 6.3 (`max_connections`),
9.1 (never a URL in a title), ADR 0003 (never the whole list in memory),
ADR 0004 (credentials). Probes run through the app as a `devtools` flag
that prints shapes, counts, sizes and timings only, never URLs, hosts,
credentials or titles (`--probe-archive` and `--probe-tables` in
`src/probe.rs` are the pattern).

## Scope

- **Lists:** `get_vod_categories`, `get_vod_streams`,
  `get_series_categories`, `get_series`: counts, body sizes, time, fields
  and their types (numbers as strings?), how category ids relate to the
  live ones (shared id space or not), and how many categories and titles
  the user's current category rules would show.
- **Info:** `get_vod_info` and `get_series_info` for a few titles: shape,
  size, which fields are filled (plot, cast, duration, rating, release
  date, backdrop), how seasons and episodes are laid out, episode ids.
- **Ids:** whether movie `stream_id`s collide with live ones, and series
  ids with either (decides favourites' and history's keys, ticket 11).
- **Streams:** the `/movie/` and `/series/` URL forms, `container_extension`
  values seen, redirects (as live, 6.1).
- **mpv:** `duration` and `time-pos` known and right; seeking; where
  `time-pos` starts; whether `start=` resumes at a position.
- **Posters:** `stream_icon` / `cover` hosts (counts by host, provider's
  own or third party), sizes and formats, failures.
- **Downloads:** whether the provider honours HTTP ranges, how fast a
  movie downloads, whether a download counts against `max_connections`.
- **Search size:** total title count, to decide whether names fit in
  memory as live search keeps them (Open in the milestone).
- Results in SPEC 6.1 as `Verified against the user's provider
  (2026-10-xx, VOD ticket 01)`, rounded and naming no hosts or titles.

## Acceptance

- [x] Each question above answered in SPEC 6.1, or marked as unanswerable
  with why.
- [x] The milestone's Proposed section updated where an answer changes
  it, and later tickets' scope with it.
- [x] Every request rate-limited as 6.2; no credentials, hosts or titles
  in logs, docs or commits.

## Notes

Done 2026-10-02 with `vidicon --probe-vod`, two runs. Unanswered: whether
a download counts against `max_connections` (`active_cons` stayed 0 during
one; trying one beside playback on a 1-connection account was left to
ticket 10 and the user), and whether info's `duration` matches mpv (04).
The live category rules match no VOD category; per-kind rules proposed
for 02.

## Out of scope

Any user-facing change. A devtools probe added for this may stay if small.
