# 03: Conditional fetches and fetch limits

Status: done. Blocked by: 02.

## Goal

No source is fetched more than it allows, and an unchanged feed costs
one small request.

## Context

`docs/milestones/epg-sources.md`, ticket 02. SPEC 6.2, 7.2.
`src/refresh.rs`, `src/xtream.rs` (backoff), `src/store.rs`.

## Scope

- `If-Modified-Since` / `ETag` per source; a local path by mtime.
- `every_hours` (default 12, at least 1) and `max_per_day` (default
  none). Every request counted over a rolling 24 h in SQLite; scheduled
  fetches stop 2 short of the cap; `r` skips a source at the cap.
- `429` / `503` with `Retry-After` backs that source off.
- Refresh state carries per source: unchanged, fetched, skipped at cap
  with the next time, backing off, failed.
- SPEC 6.2, 7.2 updated.

## Acceptance

- [x] Tests: 304 imports nothing; interval and cap with the reserve;
  count survives a restart; `Retry-After` honoured; `r` at the cap.
- [x] Checked against the demo provider with a low cap. Demo checked
  2026-10-13 with `max_per_day = 3` on `match.xml`: the first run
  fetched both feeds, a restart fetched nothing, two `r` got 304s, the
  third `r` skipped `match.xml` at 3/3 with the next slot 24 h after the
  first request. With `DEMO_FAIL=epg:429` an `r` backed `other.xml.gz`
  off for the `Retry-After: 60`, and an `r` inside that minute skipped
  it without a request.

## Out of scope

Showing it (04).

## Settled

- `max_per_day` is at least 3, so scheduled fetches get at least one of
  the day's requests after the 2 kept for `r`. `every_hours` and
  `max_per_day` are refused on a path and on the provider's guide.
- A source backing off is due again once its `Retry-After` passes, not
  `every_hours` later; `r` does not override a backoff.
- A 429 or 503 without `Retry-After` is a plain failure, waiting
  `every_hours`.
- A new `shift_minutes` drops the source's validators and mtime, so the
  next fetch imports with the new shift rather than getting a 304.
- A path's modification time is stored after each read, failed or not,
  so a missing file is tried once, not at every check.
- Outcomes go out as `Update::Sources`, by label in order, for the
  sources the stage took a turn on; the UI ignores them until 04.
