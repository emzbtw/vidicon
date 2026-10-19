# 01: Verify the archive against the provider

Status: done. Blocked by: none.

## Goal

Know, from real requests, how the provider serves its archive, before any
code depends on it.

## Context

`docs/milestones/catch-up.md`. SPEC 6.1 (endpoints, and how earlier
verifications were written up), 6.2 (politeness), 9.1 (mpv title, never a
URL), ADR 0004 (credentials). Agents cannot read the credentials file:
probes run through the app (e.g. a `devtools` flag that makes one request
and logs only shapes and counts, redacted as `xtream::redact` does) or by
the user with a command that prints aggregates only.

## Scope

- **Past guide in `xmltv.php`:** how far back its programmes reach, for
  channels with an archive and without. Decides ticket 02's approach.
- **`get_simple_data_table&stream_id=`** for one archive channel: its
  shape, how far back, whether it marks programmes `has_archive`, size.
- **Archive URL:** which form works: `/timeshift/{user}/{pass}/{minutes}/
  {YYYY-MM-DD:HH-MM}/{stream_id}.ts`, `/streaming/timeshift.php?...`, or
  an `.m3u8` variant. Follow redirects as for live (6.1).
- **Start time zone:** UTC, local, or `server_info.timezone` from the
  account check. Check by playing a programme whose start is easy to see.
- **Duration:** whether the stream stops at `start + minutes` or runs on;
  what happens when it reaches now.
- **mpv:** whether it can seek in an archive stream (`.ts` and `.m3u8`),
  and whether `duration` and `time-pos` are known.
- **Bounds:** what a start before the archive's start, or in the future,
  answers.
- Results in SPEC 6.1 as `Verified against the user's provider
  (2026-10-xx, catch-up ticket 01)`, rounded and naming no hosts.

## Acceptance

- [x] Each question above answered in SPEC 6.1, or marked as unanswerable
  with why.
- [x] The milestone's Proposed section updated where an answer changes it,
  and ticket 02's or 05's scope with it.
- [x] Every request rate-limited as 6.2; no credentials or hosts in logs,
  docs or commits.

## Notes

- Done 2026-10-02 with `vidicon --probe-archive <stream_id>` (devtools,
  `src/probe.rs`), run against a recently played channel with an archive.
  `redact` now also masks `/timeshift/` paths, which carry the credentials
  like `/live/`; the client gained `timeshift_url`, `timeshift_php_url` and
  `simple_data_table` for tickets 03 and 06.
- The start's zone was settled without watching: no stream carries a DVB
  clock, but a start in the future is a 404, and UTC clock readings just
  under and just over the server's offset bracket its zone. Whether the archive starts
  where the guide says is left to ticket 03's check by hand.
- Changed the plan: `xmltv.php` reaches only about a day back, so older
  days became ticket 06. mpv seeks natively, so ticket 05 is a check.
- Left open for ticket 04: what a `.ts` start over does at now.

## Out of scope

Any user-facing change. A devtools probe added for this may stay if small.
