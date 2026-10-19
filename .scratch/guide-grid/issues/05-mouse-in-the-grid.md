# 05: Mouse in the grid

Status: done. Blocked by: 02.

## Goal

Full mouse support in the grid, with horizontal and vertical movement that
do not fight.

## Context

`docs/milestones/guide-grid.md`, SPEC 8.4 (mouse in the main view). The
`ListView` pans vertically on click-drag (`mouse-drag-pan-enabled`). Its
content must not get a scrollable width (the channel column would move).

## Scope

- Click selects a programme; double-click plays its channel.
- Shift+wheel and a touchpad's horizontal swipe move the time.
- Drag: locks to the direction it starts in, after a few pixels. Vertical
  pans the channels as now; horizontal moves the shared time offset, the
  header with it.
- Time stops at the kept window's ends.
- Clicks on the category header's arrows switch category.
- Update SPEC 8.4.

## Acceptance

- [x] Each input checked by hand on a mouse and a touchpad; the user
  confirms the drag lock feels right.
- [x] Tests for the direction lock (the pure decision from a drag's first
  movement).

## Out of scope

Kinetic scrolling beyond what Slint gives. Pinch zoom.
