# 17: Minute refresh cost

Status: done. Blocked by: 11.

## Goal

The once-a-minute time refresh costs the same whatever the size of the list,
and never shows as a hitch.

## Context

`docs/perf/v1.md` (Idle). `tick_every_minute` in `src/ui.rs` calls
`Screen::refresh_times` just after each minute: the rows, the detail panel
and the NOW PLAYING card are re-read so now/next and progress stay current.

Measured in ticket 11 (release build, CPU time over a 61 s idle window that
holds one tick): about 70 ms with All's 50k channels in the list, about
20 ms in a category of 20. The part that grows with the list is
`Channels::refresh`, which calls `row_changed` for every row in the view,
built or not. It runs on the UI thread, so if it lands while the user is
scrolling or typing it can drop frames: 70 ms is 4 frames at 60 Hz and 10
at 144 Hz. Idle CPU stays within SPEC 10's "~0" either way.

Not measured: how much of the 70 ms is one block on the UI thread, and
whether the hitch is visible. Check that first.

Possibly related, from reading the code and not checked: `refresh_times`
calls `show_detail`, which sets a new LATER model, and the LATER list
returns to the top when its model changes. If so, a LATER list the user has
scrolled jumps back every minute.

## Scope

- Measure the tick's block on the UI thread with 50k channels (a debug log
  around `refresh_times` is enough).
- Notify only the rows that can be on screen. Slint does not say which rows
  a `ListView` has built, so either track the range `row_data` was last
  asked for, or derive it from the list's scroll position and height.
- If the LATER list does jump back on the tick, keep its position when the
  channel shown has not changed.

## Acceptance

- [x] The tick costs about the same in All (50k) and in a small category;
      numbers in `docs/perf/v1.md`. About 60 ms of CPU time in both; the
      re-read blocks for 1.1 ms and 0.7 ms.
- [x] Rows on screen still show the new programme and progress after a
      tick, checked across a programme change against the demo provider.
      Snapshots either side of 16:30 on 2026-09-30: five rows and the
      detail panel changed programme, LATER lost its first entry.
- [x] A scrolled LATER list keeps its position across a tick, or a note
      here that it already did. It did not: it went back to the top.
      Snapshots either side of a tick are now pixel-identical there, and a
      channel change still returns it to the top.

## Outcome

The premise was wrong. The re-read is not what costs: notifying a row Slint
has not built takes about 10 ns, so 50k rows add half a millisecond, and
"notify only the rows on screen" was dropped. The CPU time is one 5 to 7 ms
frame and about 20 light frames over the next 150 ms, the same whatever the
list size; the longest event-loop stall measured was 18 ms. `docs/perf/v1.md`
(Idle) has the detail.

Changed: the LATER rows are updated in place and the list returns to the
top when `detail-key` (the channel shown) changes, not when its model does.
`minute refresh` in the debug log times the re-read.

## Out of scope

The detail panel's cost per fill (ticket 16, accepted).
