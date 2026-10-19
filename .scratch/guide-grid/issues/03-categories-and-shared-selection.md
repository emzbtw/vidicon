# 03: Categories and shared selection

Status: done. Blocked by: 01.

## Goal

Switch category without leaving the grid, and move between the grid and
the main view without losing your place.

## Context

`docs/milestones/guide-grid.md`. SPEC 8.2 (library, order, manage mode).
Ticket 01's grid.

## Scope

- `[` and `]`, and Tab and Shift-Tab: previous/next category, in the
  library's order and with its hidden categories left out: All, Favourites,
  Recently played, then the shown categories. Favourites keep their manual
  order and Recently played its recency, as in the main view.
- The grid's header names the category, with a count of guide channels.
- Shared selection: `t` opens the grid on the main view's category and
  selected channel (or the first guide channel after it, if that one has no
  guide), at now. `t` back puts the main view on the grid's category and
  its selected channel.
- A play or a favourite in the grid shows in the main view's recently
  played and favourites at once.
- Keys panel and SPEC 8.2, 8.3.

## Acceptance

- [x] Tests: the next guide channel when the selected one has none, and
  the category order with hidden categories.
- [x] Round trip checked with `--keys`: select in the main view, `t`, move,
  `]`, `t`, and the main view is on the new category and channel.

## Out of scope

A category sidebar in the grid. Manage mode from the grid (`e` stays in
the main view).
