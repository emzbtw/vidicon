# 23: Search index build fails while an import writes

Status: done. Blocked by: none.

## Goal

The search index is always built after a channel import, however long the
EPG import that follows holds the writer.

## Context

SPEC 8.5, ADR 0003. Seen once in ticket 20 against the 50k demo provider,
right after an `r` refresh:

```
ERROR vidicon::ui] building search index: database is locked
```

`rebuild_index` in `src/ui.rs` opens its own connection with `store::open`.
`store::open` runs `migrate`, which starts an `IMMEDIATE` transaction, so it
takes the write lock even when there is nothing to migrate. WAL lets readers
through, but not a second writer: the build waits on the busy timeout (5 s)
and fails if the writer holds on longer.

The index is rebuilt on `Update::Channels`, and the EPG import starts right
after the channel import (SPEC 7.2), so the two race on every refresh. The
demo's EPG import (about 350k programmes) took about 5 s. The real
provider's guide is bigger (78 MB, SPEC 6.1), so this may fail on most real
refreshes. Not checked against the real provider.

When it fails, nothing retries: `index` stays `None` and search shows
"Indexing channel names…" until the next channel import, up to
`refresh_hours` later.

## Scope

- Open the index builder's connection without taking the write lock: a
  read-only open that skips migrations (the UI's own connections have
  already migrated), or a migration check that only takes the lock when
  `user_version` is behind.
- Check the launch path too: the index is built at launch while a
  background refresh may start.
- A test that builds the index while another connection holds a write
  transaction.

## Acceptance

- [x] Test: the index builds while a writer holds the lock for longer than
      the busy timeout. `builds_while_an_import_writes` in `src/search.rs`
      failed first with `database is locked` after 5.0 s, as in the log.
- [x] Against the 50k demo provider, `search index` is logged after every
      `r` refresh (five in a row), with no `database is locked`. Six in a
      row, release build: built in 82 to 136 ms each time, while the EPG
      import was still writing.

## Outcome

`store::open_reader` opens the database read-only and skips migrations;
the index builder uses it, at launch and after each channel import. The
UI's own connection has migrated before the first build, so nothing is
lost. Retrying a failed build was not needed once the lock was gone.

## Out of scope

Retrying a failed build for other errors.
