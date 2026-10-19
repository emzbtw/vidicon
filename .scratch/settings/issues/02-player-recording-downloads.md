# 02: Player, Recording and Downloads sections

Status: done. Blocked by: 01.

## Goal

Every remaining key in SPEC 4.2 is editable, and a wrong value is
either impossible or refused with its reason.

## Context

`docs/milestones/settings.md`, ticket 01. SPEC 4.2, 9.1 (mpv spawn),
9.4, 9.5, 9.6. `src/player.rs`, `src/recorder.rs`, `src/download.rs`.

## Scope

- Player: `mpv_path`, `extra_mpv_args` (one line, split shell-style;
  settle the shown form).
- Recording: `recordings_dir`, `ffmpeg_path`, `record_before` and
  `record_after` as steppers 0 to 60.
- Downloads: `downloads_dir`.
- Checks on confirm with the reason under the field: absolute path,
  writable, binary found and executable, quoting; `Create it` for a
  missing directory.
- Live apply: the next mpv spawn, recording or download uses the new
  value; what runs keeps going.
- SPEC 4.2, 8.2 updated.

## Acceptance

- [x] Tests: each check's refusal and message; a refused value is not
  written; the next spawn, recording and download use a saved value.
- [x] Checked by screenshot against the demo provider, with a refusal
  showing.

## Out of scope

Providers and EPG sections.
