# Milestone: EPG guide grid

After v1 (which ships now/next only). Starting point: layout C on the
`prototype/slint` branch. Planned 2026-10-01; tickets in
`.scratch/guide-grid/`. Done 2026-10-01 (tickets 01 to 06).

## Requirements

- **A separate screen**, toggled with `t` from the main three-pane view
  (SPEC 8.2).
- **Rows: channels with a guide only.** About 14% of the provider's
  channels link to the guide (SPEC 6.1), so All would otherwise be mostly
  empty rows. A channel without a guide is still in the main view.
- **Categories:** the library's, in its order: All, Favourites, Recently
  played, then the shown categories (ticket 20). `[` and `]`, or Tab and
  Shift-Tab, switch category.
- **Horizontal time scrolling.** One shared time offset drives every row, so
  the timeline moves as a whole. The channel column and the time header stay
  fixed. Inputs: shift+wheel, touchpad horizontal swipe, horizontal click-drag.
  Don't give the `ListView` a scrollable content width (that would move the
  channel column too).
- **Span:** 3 h across the grid, fixed.
- **Range:** what the guide keeps: from 1 h back to 3 days ahead (SPEC 7.1).
  Going further back waits for catch-up support.
- **Selection: a programme.** `j`/`k` and up/down move between channels at
  the same time; `h`/`l` and left/right go to the previous/next programme on
  the channel, and the timeline follows. Enter plays the channel live, also
  from a programme that has not started (no reminders yet).
- **Jumps:** `n` back to now, `<` and `>` a day back or ahead, `P` prime time
  (20:00 today, or tomorrow once past).
- **Detail strip** above the grid for the selected programme: channel,
  title, times, time left or when it starts, and two or three lines of its
  description (already stored, SPEC 5).
- **Shared selection with the main view.** `t` opens the grid on the main
  view's category and channel, at now; `t` again returns to the channel last
  selected in the grid. Mouse and keys share it, as in the main view.
- **Search:** `/` filters the category's guide channels by name, as in the
  main view (SPEC 8.5); Enter keeps the filter, Esc clears it.
- **Mouse:** click selects a programme, double-click plays. A drag locks to
  the direction it starts in: vertical pans the channels, horizontal moves
  the time.
- **Performance:** 144 fps at 144 Hz while scrolling (ADR 0005). The prototype
  dropped to 134 to 140 fps under a one-row-per-frame stress test.

## Settled 2026-10-01

All of the above, with the user. Horizontal and vertical drag, the one open
question, is settled by the direction lock.
