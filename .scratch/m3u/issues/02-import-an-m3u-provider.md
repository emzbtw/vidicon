# 02: Import an M3U provider

Status: done. Blocked by: 01.

## Goal

A `type = "m3u"` provider in `credentials.toml` shows its channels in
the main screen, refreshed like an Xtream provider's.

## Context

`docs/milestones/m3u.md`, ticket 01's findings. SPEC 3, 4.1, 5, 6.2,
7.1, 7.2. ADR 0001 as amended, 0003, 0004. `src/config.rs`,
`src/import.rs`, `src/store.rs`, `src/refresh.rs`, `src/xtream.rs`
(`redact`, the rate limits).

## Scope

- Config: `type` (`xtream` default, `m3u`), `url` as URL or path,
  `user_agent` optional; `username` and `password` required for Xtream
  only. Under the permission check and the redactor.
- The `Source` enum, chosen per provider, deciding import and where a
  stream URL comes from.
- Schema: nullable `channel.url`, plus whatever ticket 01 settled for
  `tvg-name`.
- Parser and import, as the milestone's Parsing, Fields and Identity
  requirements, streamed into one transaction. Categories from groups,
  "Uncategorised" for none.
- Refresh: one GET with the provider's user agent, on the schedule and
  `r`, under the rate limits; a path re-read on mtime change.
- Rejects (no `#EXTM3U`, short of `Content-Length`, 0 entries,
  `#EXT-X-TARGETDURATION` or `#EXT-X-STREAM-INF`)
  keep the old channels and show in the refresh panel; skipped entries
  are counted there.
- The demo provider serves an invented playlist: duplicates, a blank
  line and a comment mid-entry, CRLF, unquoted attributes, a long URL,
  an entry with no `#EXTINF`, one with no URL, a title with a comma,
  an `#EXT-X-CREDENTIALS` line, and one stream in two groups.
- Performance: a 50k-entry synthetic playlist timed from request to
  commit, and an `m3u_import_peak_memory_does_not_scale_with_list_size`
  test beside the three in `src/import.rs`; numbers in
  `docs/perf/v1.md`.
- ADR 0001 checked; SPEC 3, 4.1, 5, 7.1, 7.2 updated.

## Acceptance

- [x] Tests: each tolerated form parses; each reject keeps the old data;
  ids stable across a re-import and across a duplicate's reorder or
  token change, as ticket 01 settled; favourites survive a refresh.
- [x] Checked against the demo playlist, then by the user with a real
  one. No playlist URL or token in the log, and no playlist URL in the
  database; stream URLs are stored for playback, in a `0600` file.
- [x] `code-review` of the ticket's diff; findings fixed.

## Out of scope

Playback (03), hiding features (04), the guide (05), the form (06).
