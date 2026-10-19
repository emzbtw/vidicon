# 04: Offer an alternate when a channel stays down

Status: parked 2026-10-12 (milestone, Parked), still on 2026-10-18:
the channel seen staying down was a blackout, which is no drop (05).
Left parked after 05 with the user: a blackout shows on screen, `o`
(03) already lists the alternates, and the one seen had none. Pick it
up if blackouts turn out common or `o` gets pressed during them.
Blocked by: 02, 03, 05.

## Goal

When reconnecting gives up, the user is told there is another way to
watch the channel.

## Context

`docs/milestones/stream-recovery.md` (Offer on failure), tickets 02, 03.
SPEC 8.2.

## Scope

- After 02 gives up, or a channel fails to start at all: with alternates,
  the status bar says so with the key, as `Foo is down · 2 other
  listings · o`, `o` opening them in the detail panel (03); without, the
  error as today. If 05 finds blackouts can be told, one counts too.
  It did (SPEC 9.3): black, still and silent together for about 10 s,
  from mpv's `vf-metadata`/`af-metadata` with three `lavfi` filters,
  which need `hwdec=auto-copy` (about 7 % of a core more). Planned with
  the user 2026-10-18: detect only after a drop, never in normal play.
  The provider's 600 s close is already the sign. The retry's
  `loadfile` (02) sets `hwdec=auto-copy` and adds the filters, a new
  load anyway, so no extra decoder restart. Black, still and silent
  for about 10 s, applying the length itself (`blackdetect`'s metadata
  ignores its `d`, SPEC 9.3): the status bar says `Blackout`, with
  `· 2 other listings · o` when there are alternates. Not a blackout:
  the filters stay until the next channel change, where `hwdec` goes
  back to the user's.
- Settle with the user whether a channel that fails to start at all
  (not a drop) gets the offer too.
- An alternate that also fails says so and offers the next.
- SPEC 8.2 updated.

## Acceptance

- [ ] Tests: offer with alternates, none without, failing alternate moves
  on, a user action clears it.
- [ ] Checked with fake mpv against the demo provider.

## Out of scope

Switching by itself (milestone Requirements).
