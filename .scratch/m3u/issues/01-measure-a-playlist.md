# 01: Measure a playlist

Status: done. Blocked by: none.

## Goal

Know what real M3U playlists hold before the parser is built around
guesses.

## Context

`docs/milestones/m3u.md`, `research/reference-players.md` section 7.
SPEC 3, 5 (`name_norm`, `name_loose`). `src/probe.rs`,
`src/probe/epg.rs` (the same shape for a feed).

## Scope

- `--probe-m3u <url-or-path>`: streams the playlist and prints counts
  only, no URLs, hosts or names: size, entries, entries without
  `#EXTINF` or without a URL, BOM and CRLF, quoting styles, each
  attribute's presence (`tvg-id`, `tvg-name`, `tvg-logo` and aliases,
  `group-title`, `tvg-chno`, `catchup*`, `tvg-shift`, `radio`), groups
  and `;`-separated groups, duplicate `tvg-id`s, `#EXTVLCOPT` and
  `#KODIPROP` kinds, pipe suffixes, URL schemes and longest URL,
  `#EXT-X-` lines, header `url-tvg` feed count and their compression by
  extension.
- Fetched twice, a few minutes apart: how many ids change under the
  occurrence index and under a URL hash, and how many `tvg-name`s
  change (milestone, Proposed).
- Run against the demo playlist and by the user against a public list
  and the provider's `get.php` playlist.
- Findings in SPEC 6 (a new M3U subsection), rounded, naming no host.
- AGENTS.md devtools note for the flag.

## Acceptance

- [x] Run by the user against at least two real playlists; numbers
  recorded.
- [x] The milestone's Proposed entries on ids and `tvg-name` settled or
  carried into 02.
- [x] `catchup` counts recorded, for whether M3U catch-up is worth a
  later milestone.
- [x] `code-review` of the ticket's diff; findings fixed.

## Out of scope

Storing anything.
