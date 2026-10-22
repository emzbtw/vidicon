# 03: Play and record M3U channels

Status: open. Blocked by: 02.

## Goal

An M3U channel plays, records and recovers like an Xtream one, with the
headers its playlist asks for.

## Context

`docs/milestones/m3u.md` (Stream headers). SPEC 9, 9.3, 9.4, 9.5,
ADR 0002. `src/player.rs`, `src/recorder.rs`.

## Scope

- Stream URL from `channel.url` for M3U channels.
- Headers from `#EXTVLCOPT` (user agent, referrer, origin) and pipe
  suffixes, stored at import, the provider's `user_agent` as the default:
  to mpv as `user-agent`, `referrer` and `http-header-fields` per
  `loadfile`, and to the recorder's ffmpeg.
- Reconnect (stream recovery ticket 02) and alternates (`o`, ticket 03)
  work as they are; check, do not change.
- SPEC 9 updated.

## Acceptance

- [ ] Tests: the `loadfile` options and ffmpeg arguments for each header
  source; a stream with none gets the default user agent; headers do not
  leak into the next channel.
- [ ] Checked with fake mpv against the demo playlist, then by the user
  playing and recording a real M3U channel that needs a header.
- [ ] `code-review` of the ticket's diff; findings fixed.

## Out of scope

Catch-up attributes (milestone, Out of scope).
