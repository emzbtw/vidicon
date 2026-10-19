# 05: Pins, mappings and channel shifts

Status: done. Blocked by: 04.

## Goal

A channel whose guide is wrong or missing can be pointed at the right
source and channel by hand.

## Context

`docs/milestones/epg-sources.md`, ticket 04. SPEC 5 (favourites name
fallback), 8.2 (detail panel), 8.3.

## Scope

- A key on the detail panel (milestone Proposed: `E`) with pin to a
  source, with its own shift, and map to a source's channel through a
  searchable picker.
- The ambiguous list in Settings › EPG opens the same picker.
- Pins and manual mappings stored per provider channel, surviving
  re-imports and following the favourites name fallback.
- A new mapping marks its source for re-import at the next refresh, and
  the panel says so.
- A pin to a disabled source is skipped and kept.
- Keys panel row. SPEC 5, 8.2, 8.3 updated.

## Acceptance

- [x] Tests: pin overrides order; pin shift applied; mapping survives a
  re-import and a provider id change; re-import marked.
- [x] Checked against the demo provider's mismatched feed. Demo checked
  2026-10-13 with `--keys`, `other.xml.gz` (Shift -60) after the
  provider's guide: 61 matched, 41 ambiguous; `E` on channel 1
  (ambiguous) showed both sources; Enter opened the picker searched for
  `northwind sport`, both duplicates listed; Enter mapped it, the card
  said `re-import at the next refresh`; the next launch re-imported it
  (37 more programmes) and the channel read its guide `from Demo TV 2`,
  `IN USE`; `p` pinned it, `l` twice moved its programmes 10 minutes in
  the detail panel at once; Settings › EPG's form listed `Ambiguous
  channels…` (40 left), opening the list over Settings; the remove
  confirm offered `Also delete 1 manual mapping` and counted the pin.
  Real keys and the mouse (typing in the picker, row clicks, `Pin` /
  `Unpin`, clicks outside) checked by the user, 2026-10-13; Esc in the
  picker closed the whole card until fixed.

## Out of scope

Fuzzy matching.

## Settled

- `E` opens one card listing every source in order with what it gives
  the channel; Enter maps (the picker), `p` pins or unpins, `h`/`l` step
  the pin's shift by 5 minutes. The user chose this over the milestone's
  two-item menu, 2026-10-13. The same card, a filter added, is the
  mapping picker and Settings › EPG's ambiguous list.
- A pin's shift is added to its source's. A channel can be pinned to the
  provider's guide too.
- A shifted pin reads a copy of its source's programmes under
  `\x01<source>\x01\x01<minutes>\x01<channel>`, inside the source's
  range, so removal and re-import clear it with the rest; the copies are
  made at once from what is stored and again after each import of the
  source, the provider's guide included. A pin without a shift reads the
  source's programmes directly.
- Pins are by source id and go with the source; mappings are by ident,
  so they outlive it, and go only with the confirm's switch.
- A new or forgotten mapping is stored in `source_match` at once and
  marks the source `reimport` (validators cleared): due at the next
  scheduled check or `r`, not at once, so several mappings share one
  fetch. The card, the Settings row and the refresh panel row say so.
- Pins and mappings are written on the UI's writer task (`Write::Guide`),
  as favourites are, not through the refresher.
- Guide tables (catch-up ticket 06) are the provider's guide only, so a
  channel pinned to the provider's guide with a shift gets none.
