# 0002: Playback via one external mpv driven over JSON IPC

Date: 2026-09-29. Status: accepted.

## Decision

The app never renders video. It spawns a single long-running mpv (the
**Player**) with `--input-ipc-server` and a fixed app ID, and changes channel by
sending `loadfile` over the socket.

- Closing the app closes mpv.
- Closing the mpv window means "stop"; the next play starts a new mpv.
- An app-supplied mpv input config binds channel up/down, typed channel numbers
  and "back to list" in the mpv window, relayed to the app over IPC.
- The fixed app ID lets an Umbriel window rule place the mpv window.

## Alternatives rejected

- **Embedded libmpv (render API inside the UI toolkit).** Best-looking, but
  every overlay becomes our UI to build, and it constrains the toolkit choice.
  On Wayland there is no window embedding (`--wid` is X11-only), so it must be
  the render API. Can be revisited after v1.
- **mpv window layered under a transparent UI** (ynoTV's approach). Its most
  discussed bugs come from this layering.
- **A fresh mpv per stream** (open-tv's approach). Simple but slow, jarring
  channel switches.

## Consequences

- Any Rust UI toolkit works; the toolkit prototype is not constrained by video.
- The user's own `mpv.conf`, hardware decoding and OSC apply.
- mpv is a separate process, so its licence does not constrain ours.
- On the user's tiling compositor (Umbriel) two side-by-side windows are
  natural; on floating desktops this is less polished.
