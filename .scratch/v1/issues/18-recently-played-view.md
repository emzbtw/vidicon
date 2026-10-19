# 18: Recently played in the library

Status: done. Blocked by: none.

## Goal

A "Recently played" entry in the library pane that lists more history than
the detail panel's last 4, and can be cleared.

## Context

SPEC 5 (`history`), SPEC 8.2 (detail panel RECENTLY PLAYED). `history`
keeps one row per channel, pruned to `HISTORY_KEPT` (20) per provider in
`src/store.rs`. The library's fixed entries are `Category::All` and
`Category::Favourites` in `src/ui.rs`.

## Scope

- Raise `HISTORY_KEPT` to 50.
- A "Recently played" entry directly under Favourites, latest first, same
  rows as any other channel list. Channels gone after an import already drop
  out of `history`.
- Clear the list, from a key and a clickable control in that view. It needs
  a second press to confirm (the status bar says what the second press does;
  the confirm lapses after a few seconds or on any other key). No dialog.
- The detail panel keeps showing the last 4. After a clear, `-` with the
  Player closed has nothing to play, and says so in the status bar.
- Update SPEC 5, 8.2 and 8.3.

## Acceptance

- [x] Tests: pruning keeps 50; clear empties one provider's history only.
- [x] Playing a channel moves it to the top of the view while it is open.
- [x] Clear needs two presses; a single press followed by another key
      leaves the list alone.
- [x] Empty view shows a one-line explanation, like "No favourites yet."

## Out of scope

Removing single entries. History across providers.
