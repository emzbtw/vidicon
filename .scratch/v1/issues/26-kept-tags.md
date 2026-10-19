# 26: Kept region tags

Status: done. Blocked by: none.

## Goal

Say which regions to keep instead of listing every region to hide. One
entry such as `uk` hides every channel whose name is tagged for another
region (`FR: x`, `DE| y`), including regions the provider adds later.
Untagged channels always stay.

## Context

SPEC 5 (channel rules), 8.2 (manage mode), 8.3, 10. Ticket 25's hide
rules work, but on the real provider they grow with every region: about
20 rules so far, all region tags, against about 40 distinct leading tags
still present in shown categories. A plain allowlist of names, like the
category rules, does not work: about 7k of the 18k channels in shown
categories carry no tag, and it would hide all of them. Settled with the
user on 2026-10-01 (option 1 of two; the other, a name allowlist per
category, was rejected because new format categories would not follow it).

## Behaviour

- **Tag:** after NFKC folding and lowercasing, the start of a name
  (leading whitespace skipped) is 2 or 3 letters followed by `:` or `|`,
  optionally with spaces between. Codes joined by `/` count as one tag
  with several regions (`BE/FR:`). Digits are never a tag, so `4K:` and
  `24/7:` are not. Only the start of a name counts.
- **Kept tags:** per provider, none by default. With none, nothing
  changes. With some, a channel is hidden if its name has a tag and none
  of the tag's regions is kept. Channels without a tag are never hidden
  by this.
- A kept tag is stored as its letters only, so `uk`, `UK:` and `uk|` are
  the same entry.
- Ticket 25's hide rules still apply on top: a channel is hidden if either
  hides it. They remain the way to catch tags placed mid-name (`… | FR:`)
  and anything the tag shape misses.
- Hidden channels behave as with hide rules: out of All, their category,
  search and the guide grid; Favourites and Recently played unaffected.
  Rules, not a snapshot, surviving restart and refresh.
- Known cost, accepted: a 3-letter brand prefix (`ABC: x`) looks like a
  tag and is hidden until kept. The counts below show it before a keep
  list is committed to.

## Scope

- Schema: kept tags per provider. `channel.shown` derives from both kept
  tags and hide rules, recomputed by each import and each edit as now.
- Manage mode: a `Keep tags` row beside `Hide names`, chips with `×`, and
  a button for the filter (`Keep tag “uk” · hides N`, or `Stop keeping
  “uk”`), counting live like the hide button. A key for it alongside
  Ctrl+Enter and Ctrl+H; propose the key and the row's wording before
  building. The row should also say how many channels the kept tags hide
  in all.
- Same budgets as ticket 25: All's first frame, scroll and search per
  keystroke within SPEC 10; manage mode keystrokes stay in single-digit ms.
- Update SPEC 5, 8.2 and 8.3, CONTEXT.md (`Kept tag`), and
  `tools/demo/README.md` if the screenshot recipe changes.

## Acceptance

- [x] Tests: tag parsing (`FR: x`, ` fr | x`, `BE/FR: x` are tagged;
      `4K: x`, `24/7: x`, `x | FR: y`, `Harbour One` are not); keeping
      `uk` hides `FR: x` but not `UK: x` or `Harbour One`; `BE/FR: x` stays
      if `fr` or `be` is kept; `UK:` and `uk|` typed as the same entry;
      kept tags and hide rules combine; a new channel tagged for another
      region is hidden after an import; Favourites and Recently played
      unaffected; the guide grid follows.
- [x] Kept tags survive restart and refresh.
- [x] With kept tags: All's first frame, scroll and search per keystroke
      within SPEC 10, numbers in `docs/perf/v1.md`.
- [x] Screenshot of manage mode with a kept tag, demo data.

## Out of scope

Tags placed mid-name. Turning existing hide rules into kept tags
automatically. Per-category keep lists. Configurable tag shapes or regex.
