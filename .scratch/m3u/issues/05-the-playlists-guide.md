# 05: The playlist's guide

Status: done. Blocked by: 02.

## Goal

An M3U provider's channels have a guide from the feeds its playlist
names, ordered with any external EPG sources.

## Context

`docs/milestones/m3u.md` (Guide), `docs/milestones/epg-sources.md`.
SPEC 6.1, 7.1, 7.2. `src/epg_sources.rs`, `src/config.rs`
(`provider_guide`), `src/import.rs`.

## Scope

- The header's `url-tvg` / `x-tvg-url` feeds, comma-separated, fill the
  `provider_guide` slot; nothing written to `credentials.toml`. Each feed
  is fetched with the conditional requests and limits of EPG sources
  ticket 03, and fails on its own.
- Matching: `tvg-name` as a key after `tvg-id`, before the display
  name, with the exactly-one rule.
- xz for every guide feed, gzip and plain as now; a pure-Rust decoder
  (milestone, Proposed); `</tv>` still required.
- Settings › EPG and the refresh panel show the playlist's feeds in that
  slot.
- Performance: the same feed timed as xz and as gzip, in
  `docs/perf/v1.md`.
- The demo playlist names two invented feeds, one xz.
- SPEC 6.1, 7.1 updated.

## Acceptance

- [x] Tests: header forms; feed order and fallback; `tvg-name` match;
  an xz feed imports; a cut-off xz feed is rejected.
- [x] Checked against the demo playlist, then by the user with a real
  one. Packages' install tests pass unchanged. Demo and real playlist
  checked 2026-10-22; the install tests passed in the v0.2.0 release run.
- [x] `code-review` of the ticket's diff; findings fixed: a feed took
  the guide entry's `name`; xz's 6-byte magic read from one chunk; the
  guide's slot computed twice; an unrelated rustfmt hunk. A test of xz
  in 3-byte chunks found lzma-rust2 0.23 reading block padding with one
  `read`, so reads into it are filled. Kept: `tvg-name` before the name
  at each name step (its exact form, the name's, then the looser
  forms), and an ambiguous `tvg-name` is no match; the `http(s)`-only
  feeds; migration 31 reading M3U playlists again at once.

## Out of scope

Catch-up from the guide (milestone, Out of scope).
