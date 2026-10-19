# 03: Switch to an alternate

Status: done. Blocked by: 01. Parked 2026-10-12 (milestone, Parked),
picked up again 2026-10-18.

## Goal

The user sees a channel's other listings and plays one instead.

## Context

`docs/milestones/stream-recovery.md` (Alternates, Switch by hand,
Parked), ticket 01's counts, ticket 02's run. SPEC 5 (`name_loose`),
6.1 (looser name form), 8.2, 8.3, 9.2, 9.3.

In ticket 02's run a channel showed a blackout picture for 10 min at a
time. The user found the same channel listed under another tag (`4K:`
against `US:`) by hand; `name_norm` keeps that tag, so it had no
alternate. A key that cycled through alternates unseen was dropped for a
list: with tags dropped, an alternate can be another region's feed, and
the user wants to choose.

## Scope

- Store: `channel.name_loose`, the looser name form (SPEC 6.1), set by
  the import and filled by migration 28; empty for a divider
  (`##### SPORT #####`). Index `(provider_id, name_loose, num)`.
  `store::alternates`: a channel's other shown listings by it, those
  with its `name_norm` first, then its category's, then by number; hidden
  ones checked row by row, as it runs on each move.
- Detail panel: a chip `+N listings` beside the format chip; `o` or a
  click opens them in LATER's place (`OTHER LISTINGS`), Enter or a click
  plays one and the list stays open on it; Esc or `o` closes. Rows are
  read only when opened, at most 50.
- Not inside mpv: mpv's `o` is its own.
- Recently played records what actually plays (it did already).
- `--probe-alternates` counts by the looser form.
- SPEC 5, 6.1, 8.2, 8.3, 9.2 and 9.3, the keys panel, CONTEXT
  (Alternate) updated.

## Acceptance

- [x] Tests: alternates found by the looser name (tags dropped), hidden
  ones and dividers left out, `+1` not matched, order, limit; migration
  28 fills stored channels; the probe's counts.
- [x] Checked with fake mpv against the demo provider (its 4K and 8K
  copies of the UK channels), and by the user against the real one
  (2026-10-18: the blacked-out channel lists the alternate they had
  found by hand, and playing it works).

## Out of scope

Offering an alternate on a drop (04). Switching by itself (milestone
Requirements). Matching `RAW` against a name without it: `RAW` stays in
the name (SPEC 5), so `UK: Harbour One RAW` is not an alternate of
`4K: Harbour One`.
