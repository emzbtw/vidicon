# 07: Favourites

Status: done. Blocked by: 02, 03.

## Goal

Favourites that can be toggled, reordered, and survive the provider changing
channel IDs.

## Context

SPEC 5 (`favourite`, `name_norm`, fallback rule), 8.3 (`f`, `J`/`K`).

## Scope

- `f` toggles the selected channel. Star marker in rows.
- "Favourites" category lists them in `position` order; `J`/`K` move the
  selected favourite down/up there.
- After each channel import: re-point favourites whose `stream_id` vanished to
  the single channel with the same `name_norm`; if none or several, keep the
  favourite and show it as unavailable (dimmed, not playable).
- `group_id` stays NULL (named groups are later).

## Acceptance

- [x] Tests: fallback with one, zero and several name matches.
- [x] Order persists across restarts.
- [x] Unavailable favourites render dimmed and Enter shows a message instead of
      playing.

## Out of scope

Named groups.
