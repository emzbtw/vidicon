# 05: The playlist's guide

Status: open. Blocked by: 02.

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

- [ ] Tests: header forms; feed order and fallback; `tvg-name` match;
  an xz feed imports; a cut-off xz feed is rejected.
- [ ] Checked against the demo playlist, then by the user with a real
  one. Packages' install tests pass unchanged.
- [ ] `code-review` of the ticket's diff; findings fixed.

## Out of scope

Catch-up from the guide (milestone, Out of scope).
