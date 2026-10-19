# 12: Detail panel, recently played and player controls

Status: done. Blocked by: 04, 07, 09.

## Goal

The detail panel is useful for every channel, not only the 11% with guide
data, and the Player can be paused, stopped, muted and made fullscreen from
the app.

## Context

SPEC 8.2 (detail panel, list rows, status bar hints), 8.3 (Space, `s`, `m`,
`F`, `-`), 5 (`history`), 9.1 (controls). Mocked up and agreed on
2026-09-29: logo box, facts chips, "No guide" note, recently played, NOW
PLAYING card with time layout "B" (times left, time left right, bar under).
Logos themselves are ticket 13; this ticket draws the initials fallback in
the same boxes.

Mockup: `prototype/detail-panel` branch (`prototypes/detail-panel/README.md`,
screenshots alongside). Throwaway code; the layout and icons are the
reference, not the fake-data wiring.

## Scope

- Detail panel as SPEC 8.2: logo box (initials on a colour tile from the
  name, ASCII letters only), number and category, favourite and format
  chips, name, NOW card or "No guide for this channel", LATER, RECENTLY
  PLAYED, NOW PLAYING. Width 260 to 340 px. Not built in search mode (a
  0 px wide panel wraps text per character and pushes the status bar out).
- List rows: 32 px tile (40 px searching), vertically centred.
- `history` table (migration), written on the writer; last 4 shown, click
  plays; `-` plays the channel before the current one.
- Player: `observe_property` for `pause`, `mute`, `fullscreen`; events to the
  UI; commands `cycle pause|mute|fullscreen` and `quit` for stop.
- NOW PLAYING card: logo, channel, programme; with a guide, times and time
  left above the bar, refreshed at minute boundaries; without, "Watching for
  N min". Icon buttons (SVG in `ui/icons/`), borderless, accent-tinted while
  on, hover and pressed states.
- Status bar: merged hints while playing; compact playing state on the right
  in search mode.

## Acceptance

- [x] Tests: history ordering, pruning to 20, dropping channels gone after an
      import; `-` with zero, one and several entries.
- [x] Tests with the fake IPC socket: observed property changes become
      events; each control sends the right command; stop closes mpv (Stopped).
- [x] Pausing or muting inside mpv updates the app's buttons (manual, real
      mpv).
- [x] Screenshots: guide channel, no-guide channel, paused and muted, search
      mode (`docs/screenshots/`).

## Out of scope

Real logos (13). Seeking (live streams). Volume levels beyond mute.
