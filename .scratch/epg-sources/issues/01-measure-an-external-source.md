# 01: Measure an external source

Status: done. Blocked by: none.

## Goal

Know how an external XMLTV feed matches the provider's channels before
building on it.

## Context

`docs/milestones/epg-sources.md`, `research/reference-players.md`
section 6. SPEC 3 (`name_norm`), 6.1 (the provider's XMLTV), 7.1.
`src/probe.rs`, `src/import.rs`.

## Scope

- `--probe-epg <url-or-path>`: streams the feed (gzip or plain) and
  prints counts only, no URLs or names: size, channels, programmes, days
  reached back and ahead, time offsets seen; then, against the active
  provider, matches by exact id and by normalised name (unique,
  ambiguous, none), and overlap with channels the provider's guide
  covers.
- Times: compare a few matched programmes with the provider's guide for
  the same channel to spot a shift.
- Findings in SPEC 6.1, rounded, naming no host.
- AGENTS.md devtools note for the flag.

## Acceptance

- [x] Run by the user against a real feed; numbers recorded.
- [x] The milestone's Proposed entries settled or carried into 02.

## Out of scope

Storing anything.
