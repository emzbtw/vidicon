# 04: Player behaviour

Status: done. Blocked by: 01.

## Goal

The Player behaves as ADR 0002 and SPEC 9 describe: one long-running mpv,
fast switching, correct lifecycle, errors surfaced.

## Context

SPEC 9.1, 9.3, 10 (channel switch budget). ADR 0002, ADR 0004 (no credentials
in titles).

## Scope

- Spawn lazily on first play with the SPEC 9.1 arguments plus
  `extra_mpv_args` from `config.toml`; socket under `$XDG_RUNTIME_DIR/vidicon/`.
- Switching sends `loadfile ... replace` to the existing process.
- `force-media-title` set to `<num> <name>`; the URL never becomes the title.
- App quit kills mpv. mpv exiting marks playback stopped; next play respawns.
- Listen for `end-file` with an error reason and report it (channel name,
  reason) to the UI status area.
- `config.toml` loading per SPEC 4.2 (defaults when missing, unknown keys are
  an error), if not already present.

## Acceptance

- [x] Keypress to `loadfile` sent is under 50 ms (log a timing in debug).
- [x] Closing the mpv window, then pressing Enter, starts a new mpv.
- [x] Quitting the app leaves no mpv process behind.
- [x] Tests against a fake IPC socket: spawn, switch, end-file error, exit.
- [x] mpv title shows the channel, never the URL.

## Out of scope

Keys inside mpv (ticket 09).
