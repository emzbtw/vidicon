# 02: Archive flag and a guide that reaches back

Status: done. Blocked by: 01.

## Goal

The store knows which channels have an archive and how long, and keeps
their past programmes for that long.

## Context

`docs/milestones/catch-up.md`. SPEC 5 (data model), 6.1, 7.1 (import and
the kept window), ADR 0003 (streaming import, never the whole guide in
memory). `src/import.rs` (`EPG_KEEP_PAST`, `write_epg`), `src/store.rs`,
`src/fake_xtream.rs`, `tools/demo/demo_xtream.py`.

## Scope

- `channel.archive_days INTEGER NOT NULL DEFAULT 0`, a migration like the
  earlier `shown` columns. The channel import sets it from
  `tv_archive_duration` when `tv_archive` is 1 (both arrive as numbers or
  strings), else 0.
- The EPG import keeps a programme that ended up to `archive_days` back
  for channels with an archive, 1 h back for every other. The channel's
  archive days are known before the guide (channels refresh first, 7.2),
  so the import reads them into a map of guide id to days: about 1k
  entries, not the channel list.
- `xmltv.php` reaches about a day back (SPEC 6.1), so this keeps up to
  that; older days are ticket 06.
- The demo provider and `fake_xtream` serve an archive on some channels
  (3 days, one with 7) and past programmes for them.
- SPEC 5 and 7.1 updated.

## Acceptance

- [x] Tests: archive days parsed from numbers, strings, 0 and missing; the
  window per channel (archive, none, two channels sharing a guide id with
  different days takes the longer).
- [x] Measured against the real provider: programmes kept, database size,
  import time, before and after, in `docs/perf/v1.md` (rounded).
- [x] An import still holds memory flat (7.1).

## Notes

- Migration 13 adds `channel.archive_days`; `Stream::archive_days` reads
  it, and `store::archive_days_by_guide` gives the EPG import its map.
  The EPG memory test now gives every guide id an archive.
- Adding the column moved SQLite's plan for the hidden-channel set: with
  no statistics it weighs indexes by the table's width, and picked the
  primary key over `channel_hidden`, reading every channel to find the
  hidden ones (about 35 against 27 ms for All with no rules, measured in
  Python on 50k rows). `NOT_RULED` and `any_hidden` now name the index
  (`INDEXED BY`), and `a_to_z_is_served_from_an_index` caught it.
- The demo provider gives archives to every fourth channel of the
  screenshot set and 1 in 50 after it (channel 6: 7 days), with a day of
  past guide each from a generator of its own, so the rest of the guide
  is unchanged. `fake_xtream::active` has two archives and a past
  programme on an archive's guide id and on one with no channel.
- Not changed, worth knowing for 03: `follow_schedules` matches a
  schedule to the nearest programme with its title within 3 h, and can
  now find a past showing up to 3 h back on a channel with an archive,
  where before only one that ended under 1 h ago was kept. A programme
  still at its time is nearer, so this needs it to have moved.

## Out of scope

Anything in the UI (03).
