# 04: EPG section in Settings

Status: done. Blocked by: 02, 03, settings 01, refresh panel 01.

## Goal

Sources are added, ordered, disabled and removed in Settings › EPG, and
each refresh shows them in the refresh panel.

## Context

`docs/milestones/epg-sources.md`, `docs/milestones/settings.md`,
tickets 02 and 03. SPEC 8.2, 8.3, 8.4.

## Scope

- The active provider's sources in order, the provider's guide a row
  among them; `J`/`K` and drag reorder; per row match counts, fetch
  count and cap, last result. URL query strings masked, a key shows them.
- Add: URL or path, limits, shift; checked without fetching, saved last,
  imported at once with match counts and an offer to move it above the
  provider's guide; a failed fetch saves it marked failing.
- Disable and remove as the milestone says, remove keeping manual
  mappings unless ticked. The provider's guide toggle with the
  favourites warning.
- Refresh panel rows per source (unchanged, at cap, failed), each
  with its own progress. Today (ticket 02) sources run inside the EPG
  row, which shows only the provider's guide's progress: sources fetch
  on `xtream::http()`, not the provider's client, so their bytes are
  not counted, and nothing names the source (seen by the user,
  2026-10-13).
- SPEC 8.2, 8.3, 8.4 updated.

## Acceptance

- [x] Tests: add, reorder, disable, remove written to the file and
  applied; remove deletes rows in one transaction; the warning count.
- [x] Checked by screenshot against the demo provider. Demo checked
  2026-10-13: `K` moved `match.xml` above the provider's guide, writing a
  `provider_guide = true` entry; `other.xml.gz` added through the form
  with Shift -60, imported at once (matched 61, ambiguous 41) with the
  move offer under it; the provider's guide's switch off counted the
  favourites; the remove confirm counted 20 channels going back; an `r`
  with `DEMO_FAIL=get_vod_streams:404` and `max_per_day = 3` left the
  panel with `at its cap 3/3 in 24 h, next 19:05` and `✓ unchanged` rows
  under EPG. Drag reorder and keyboard scrolling of the fields checked
  by the user with a real mouse and keys, 2026-10-13.

## Out of scope

Pins and manual mappings (05).


## Settled

- Indexes in the file writers are the order's (`config::epg_order`): the
  provider's guide gets a `provider_guide = true` entry only when it
  moves, another moves above it, or it is edited.
- Settings edits reach the refresher as `Request::Sources`, a pass at
  once: its sync stores the order, deletes a removed source's data in
  one transaction (`store::sync_sources`), and imports a new source.
- The source form keeps values until `Save changes`, the switch
  included, as the provider form does; the guide-off warning shows under
  the switch before saving, and only when some favourite would lose its
  guide.
- Source rows in the refresh panel are `Stage::Source(label)` rows after
  EPG, run by the EPG stage; `Update::Sources` is gone.
- No manual mappings exist until 05, so the remove confirm has no "Also
  delete manual mappings" switch; 05 adds it.
