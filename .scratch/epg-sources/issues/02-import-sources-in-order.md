# 02: Import sources in order

Status: done. Blocked by: 01.

## Goal

External sources set in `credentials.toml` give channels their guide,
by the provider's source order, everywhere the guide is used.

## Context

`docs/milestones/epg-sources.md`, ticket 01's findings. SPEC 4.1, 5,
6.1, 7.1, 7.2, 8.2, catch-up ticket 06 (guide tables). ADR 0001 as
amended, 0003, 0004. `src/config.rs`, `src/import.rs`, `src/store.rs`,
`src/refresh.rs`, `src/tables.rs`.

## Scope

- `[[provider.epg]]` parsed (`url` or `path`, `shift_minutes`,
  `enabled`; limits are 03's), under the permission check and redactor.
  The order is the file's order, the provider's guide first unless a
  marker says where it goes (settle the form).
- Import: streamed, gzip or plain; the source's channel list kept;
  programmes for matched channels only, in the milestone's window;
  `shift_minutes` applied at import. One transaction per source; a
  failure keeps the last good data and does not fail the provider's
  refresh.
- Matching: exact id, then `name_norm`, then the looser form (ticket
  01's `epg_norm` in `src/probe/epg.rs`), each only on exactly one;
  stored per source, recomputed after its import.
- A source's identity: its URL or path with the query string dropped,
  so a renewed token keeps its mappings (milestone, Proposed; settle).
- Lookup: a channel's guide from the first enabled source with
  programmes for it, whole, wherever the guide is read (detail panel,
  grid, search, recorder, catch-up).
- Catch-up from an external programme by the channel's archive.
- The provider's guide can be disabled (`enabled = false` on a marker
  or provider key; settle), stopping `xmltv.php` and the guide tables
  and deleting its programmes.
- The demo provider serves two invented feeds, one with matching ids,
  one with different ids and shifted times.
- ADR 0001 checked; SPEC 4.1, 5, 6.1, 7.1, 7.2 updated.

## Acceptance

- [x] Tests: order and fallback; exactly-one rule; shift applied;
  failure isolated; disabled source skipped; provider guide off; catch-up
  offered on an external programme within the archive.
- [x] Checked against the demo provider, then by the user with a real
  feed. Demo checked 2026-10-13: 140 channels from `match.xml` placed
  above the provider's guide, 20 more from `other.xml.gz` by the looser
  name, 41 ambiguous, its hour corrected by `shift_minutes = -60`, no
  token in the log or the database. Real feed checked by the user
  2026-10-13: feeds used, each named by its `name` or else its host,
  the provider's guide by the provider's name.

## Out of scope

Conditional requests and limits (03), the Settings UI (04), pins and
manual mappings (05).

## Settled

- The provider's guide's place: a `[[provider.epg]]` entry with
  `provider_guide = true`, first when absent; `enabled = false` on it
  turns it off.
- A source's identity: its URL without query string and fragment, or
  its path. A source gone from the file has its data deleted.
- A new source imports on the next pass, without waiting for the
  provider's guide to go stale.
- The detail panel names a channel's guide source: a block's optional
  `name`, else its host or file name; the provider's name for its own.
