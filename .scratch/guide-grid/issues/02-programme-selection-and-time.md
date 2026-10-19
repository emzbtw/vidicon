# 02: Programme selection and time

Status: done. Blocked by: 01.

## Goal

The grid selects a programme, not just a channel, and moves through time:
the classic TV guide.

## Context

`docs/milestones/guide-grid.md`. Ticket 01's grid. SPEC 7.1 (kept window:
1 h back to 3 days ahead), 8.2 (the main view's detail panel, for wording).

## Scope

- The selection is a programme on a channel, shown as a highlighted cell.
- `h`/`l` and left/right: previous/next programme on the channel. The
  timeline follows only as far as needed to keep the cell in view.
- `j`/`k` and up/down: the programme on the next channel airing at the
  selected time (the selection's start, or now if that is earlier), so
  moving down a column keeps to one time.
- One shared time offset moves every row; the time header moves with it.
- Jumps: `n` back to now, `<` and `>` a day back or ahead, `P` prime time
  (20:00 today, or tomorrow once past). All stop at the kept window's ends.
- A channel without a programme at the selected time: the gap is selectable
  as "No information", so `j`/`k` never skip a row.
- Detail strip above the grid: channel, title, times, time left (airing),
  starts in or started ago, and up to three lines of description.
- Enter plays the channel live, from any programme.
- Keys panel: a GUIDE group with these keys.
- Update SPEC 8.2 and 8.3.

## Acceptance

- [x] Tests: next/previous programme, the same-time rule for `j`/`k` (gaps,
  overlaps as in SPEC 6.1), each jump at and near the window's ends.
- [x] Screenshots: selection airing now, and a programme tomorrow after `>`.

## Out of scope

Reminders, catch-up and playing from a past programme. Mouse (05).
