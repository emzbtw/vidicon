# 02: Streaming channel import

Status: done. Blocked by: 01.

## Goal

Channel data lands in the full schema via a streaming import that never holds
the whole list in memory, in one transaction, safely replacing the previous
data.

## Context

SPEC 5 (data model), 7.1 (import). ADR 0003 (including the amendment: only
channel names are kept in memory, for search).

## Scope

- Schema per SPEC 5: `provider`, `category`, `channel` (with `name_norm`,
  `epg_channel_id`, `icon_url`, `extension`), `favourite`, `channel_fts`.
  Migrations from day one (a `user_version` pragma is enough).
- WAL mode; one writer connection on the worker runtime, a separate read
  connection for the UI.
- Deserialise `get_live_streams` as a stream of elements (serde sequence
  visitor), inserting as it goes, in one transaction that replaces the
  provider's channels and categories.
- `name_norm` per SPEC 5.
- Failed import rolls back; previous data untouched.
- Record `provider.last_channels_refresh`.

## Acceptance

Measured (2026-09-29): peak Rust heap during import is 69,598 B for both 5k and
50k channels (the 64 KB read buffer plus one element; SQLite's page cache is
separate and capped). Buffering the body instead reads 21 MB, which the test
catches. Real provider: about 58k channels, 21 MB, streamed and inserted in
612 ms (release); whole-app peak RSS 139 MB, most of it the UI list that
ticket 03 pages.

- [x] Test: import a generated 50k-channel fixture; peak memory does not scale
      with list size (measure, record the number in the ticket when closing).
- [x] Test: a truncated or invalid response rolls back and the old channels are
      still there.
- [x] Test: UI read connection can query during an import (WAL snapshot).
- [x] `name_norm` unit tests.
- [x] Ticket 01's plain insert is gone.

## Out of scope

EPG, favourites fallback logic (ticket 07), refresh scheduling (ticket 08).
