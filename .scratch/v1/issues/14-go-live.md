# 14: Go live

Status: done. Blocked by: 12.

## Goal

After a pause, the user can see how far behind live the Player is and jump
back to live with one key or click, instead of staying behind for the rest
of the channel.

## Context

SPEC 8.2 (NOW PLAYING card, status bar hints), 8.3 (`L`), 9.2 (`L` inside
mpv). Raised on 2026-09-29: Space resumes where the stream stopped, which
is right for catching up but leaves no way back to live short of switching
channel and back.

Streams are MPEG-TS over HTTP: mpv sees no live edge, and seeking forward
only reaches what it has buffered. Going live therefore reloads the
channel's stream (`loadfile <url> replace`, unpaused), the same command a
channel switch sends. How far behind is not reported by the stream either;
the app counts it: time spent paused since the stream last loaded.

## Scope

- **Behind live:** the Player's time paused since the last `loadfile`,
  from the `pause` property changes already observed (ticket 12). Any
  `loadfile` (play, channel switch, `-`, mpv PageUp/PageDown, go live)
  resets it to zero.
- **Go live:** `L` in the app (not while searching: it types) and `L` inside
  mpv (keybind at spawn, `script-message vidicon live`, as SPEC 9.2).
  Reloads the channel playing with `loadfile ... replace` and unpauses.
  Does nothing while at live, or with mpv closed.
- **NOW PLAYING card:** the heading reads `NOW PLAYING`, `PAUSED`, or
  `BEHIND LIVE · 2 min` while playing behind; `PAUSED · 2 min behind` while
  paused and behind. Minutes, rounded down, `<1 min` under a minute; while paused under a
  minute, plain `PAUSED` (every pause is behind).
  Refreshed on pause changes and at minute boundaries only (idle CPU: no
  per-second redraws).
- **Button order:** pause, go live, stop, mute, fullscreen. The two
  playback-position controls sit together, then stop, then audio and window.
  Go live is always present and dimmed while at live, so the other buttons
  never shift under the mouse. New icon `ui/icons/live.svg` in the style of
  the others.
- **Status bar:** while behind, the playing hints gain `L live` after the
  Space hint: `/ search  space resume  L live  s stop  m mute  - last channel
  q quit`.
- SPEC 8.2, 8.3 and 9.2 already describe the above (updated with this
  ticket); keep them in step if anything changes in implementation.

## Acceptance

- [x] Tests: behind-live accounting: pause and resume sum, a `loadfile`
      resets, pausing twice adds up, time while playing does not count.
- [x] Tests with the fake IPC socket: `L` sends `loadfile <current url>
      replace` and unpauses; does nothing while at live and with no Player;
      the mpv `L` keybind is sent at spawn and its `client-message` goes live.
- [x] Manual, real mpv: pause for over a minute, resume, card shows behind;
      `L` returns to live (the picture jumps ahead) and the card reads
      `NOW PLAYING` again.
- [x] Screenshot: `player-behind-live.png` (paused and behind) against the
      demo provider; others retaken if the card or hints changed.

## Out of scope

Seeking within the buffer (rewind, skip back). Reconnecting after the
provider drops a stream that was paused too long (today mpv ends it; a later
ticket). Catch-up / timeshift from the provider (not in v1).
