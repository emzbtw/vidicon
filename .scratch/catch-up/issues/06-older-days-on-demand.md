# 06: Older days on demand

Status: done. Blocked by: 02, 03.

## Goal

The grid shows a channel's whole archive, not only the day `xmltv.php`
reaches back.

## Context

`docs/milestones/catch-up.md`. SPEC 6.1 (`get_simple_data_table`, verified
in ticket 01: about 100 KB per channel, about 4 days back, `has_archive`
flags), 6.2 (politeness), 7.1 (EPG import replaces the provider's
programmes). Tickets 02 and 03.

## Scope

- When the grid's span reaches before the stored past of an archive
  channel on screen, fetch that channel's `get_simple_data_table`: the
  selected row first, then the rest on screen, through the client's gate
  at a lower priority than refreshes (like logos, ticket 13). Rows fill in
  as their tables arrive.
- Store the programmes it adds before what the guide has; a channel is
  fetched at most once per EPG refresh.
- The EPG import (7.1) keeps those older programmes until they leave the
  channel's archive, rather than replacing them.
- `has_archive` from the table decides playable over `archive_days` when
  they disagree.
- SPEC 5, 6.1, 7.1 updated.

## Acceptance

- [x] Tests: the import keeps older programmes and drops them past the
  archive; a table's programmes merged before the guide's; fetch order and
  once-per-refresh against `fake_xtream`.
- [x] Checked by hand against the real provider: 3 days back in the grid
  fills in, and a programme from 2 days ago plays.

## Notes

- 2026-10-02: implemented. `src/tables.rs` is the fetcher: one task for
  all providers, a queue the selected row jumps (asks gather for 100 ms
  first, as a frame's rows arrive together), and per provider the stream
  ids fetched since its `last_epg_refresh`. `Client::guide_table` waits
  behind API requests through the path logos used (`behind_api`).
  Migration 15 adds `programme.archive`; `replayable` takes it over
  `archive_days`, with `Not in this channel's archive` when a table says
  no. The grid's `Guide` asks from `row_data` (`needs_table`), and
  re-asks first when a row is selected.
- The import sets aside ended programmes on archive guide ids in a temp
  table and puts back those before the new guide's first, within the
  archive. That keeps days an earlier `xmltv.php` reached too, not only
  tables'. About 130 ms each way on a copy of the real database (16k set
  aside, 13k put back); a correlated `max(archive_days)` per row was
  replaced by a grouped join, as `channel` has no index on guide id.
- A table's flag on a programme `xmltv.php` also has is lost at the next
  import (the row is replaced), and the channel is fetched again only if
  its rows still reach before the stored past. The older days keep theirs.
- Checked against the demo provider: `jjjjjt <lt><lt><lt>` fetched the
  selected channel's table first, then the others on screen 2 s apart;
  rows filled 3 days back, programmes starting before the archive
  dimmed, and the selected cell updated when its row filled.
- Checked by the user against the real provider: a programme from 2
  days ago played, but the one 2 h after the one selected. The table's
  `start_timestamp` is the server's clock read as UTC, not UTC as ticket
  01 noted: in the user's database the table's programmes matched the
  same titles in `xmltv.php` 2 h earlier, a day or more apart (289
  matches at -2 h, at most 16 at any other shift). Its flags had also
  landed on the wrong programmes (204 of them). Fixed: the fetcher moves
  the table's times back by the server offset from the account check
  (`AccountInfo::server_offset`, now shared with playing), and rows ask
  only once a check has answered. Migration 16 drops the older days and
  flags stored before the fix (on a copy of the user's database: the 600
  rows the tables added, and all 897 flags). `fake_xtream::table` now
  sends server-clock times, and the fetch test checks they are moved.
- Checked again by the user after the fix (2026-10-02): the selected
  programme from 2 days ago plays from its start.

## Out of scope

Fetching tables for channels not on screen.
