# 19: A–Z sort toggle

Status: done. Blocked by: 18.

## Goal

One toggle that switches both the library pane and the channel list between
the provider's order and alphabetical order.

## Context

SPEC 5 (`category.sort`, `channel.num`, `name_norm`), SPEC 10 (budgets).
Provider category names usually carry a region prefix (`UK|`, `US:`), so
A–Z in the library groups categories by prefix.

## Scope

- One key (suggest `a`) and a small clickable indicator in the library
  header showing the current order. Persisted in `app_state`, shared by all
  providers.
- A–Z sorts by `name_norm`, ties by provider order, so quality tags and
  superscripts do not scatter names.
- Fixed library entries (All, Favourites, Recently played) stay at the top.
  Favourites keep their manual order and Recently played its recency order.
  Search results stay best match first.
- The selection stays on the same channel or category across a toggle.
- Indexes for the A–Z queries so All (about 58k channels) still meets the
  first-frame and scroll budgets.
- Update SPEC 8.2 and 8.3.

## Acceptance

- [x] Tests: A–Z order for categories and channels; ties keep provider
      order; Favourites and Recently played ignore the toggle.
- [x] The toggle survives a restart.
- [x] All in A–Z: first frame and 144 Hz scroll within SPEC 10, numbers in
      `docs/perf/v1.md`.

## Out of scope

Other sort orders (number, recently added). Per-pane toggles.
