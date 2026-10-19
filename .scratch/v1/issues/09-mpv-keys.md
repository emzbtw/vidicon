# 09: Keys inside mpv

Status: done. Blocked by: 03, 04.

## Goal

Channel surfing works from the mpv window, and one key returns to the list.

## Context

SPEC 9.2 (keybind table, activation spike result). The spike used
`slint` feature `unstable-winit-030`, `WinitWindowAccessor::with_winit_window`,
then `request_user_attention(Some(UserAttentionType::Critical))`; Umbriel
focused the window.

## Scope

- At spawn, add bindings with mpv's `keybind` IPC command (user's
  `input.conf` otherwise untouched), each sending `script-message vidicon
  <action>`.
- Handle `client-message` events:
  - PageUp/PageDown: previous/next channel in the current list (respecting the
    active category and search filter).
  - `0`-`9`: build a channel number; play the channel with that `num` after
    1.5 s idle; show the typed number in the app status.
  - Backspace: raise the app window via xdg-activation.
- App selection follows the channel being played.

## Acceptance

- [x] Manual on Umbriel: PageUp/PageDown surf, typed number plays, Backspace
      focuses the app.
- [x] Tests with a fake IPC socket for message handling and number entry timing.
- [x] User's own mpv bindings for other keys still work.

## Out of scope

Configurable bindings.
