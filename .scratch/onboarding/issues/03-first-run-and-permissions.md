# 03: First-run and permissions screens

Status: done. Blocked by: 01.

## Goal

A missing, empty or too-open `credentials.toml` shows a screen in the
window that fixes it, instead of an error on stderr.

## Context

`docs/milestones/onboarding.md`, ticket 01. SPEC 4.1, ADR 0004.
`src/config.rs`, `src/main.rs`. Refresh panel 02 takes over once a
provider is saved, if done.

## Scope

- No file, or no provider: full-window screen, what is missing and the
  add form from 01. Saving goes on to the main screen and the first
  refresh.
- Too-open file or directory: names the file and the `chmod`; `Fix
  permissions` sets `0600` and `0700` and carries on. Shown only when
  the user owns it and it is not in the Nix store. Nothing read until
  fixed.
- A parse error shows in the window too, with the line, no values.
- The checks in today's order (settle in this ticket).
- Lift ticket 02's refusal to remove the only provider: removing it
  goes to this screen. Today `ask_remove` in `src/ui.rs` refuses with
  `It is the only provider: add another first`, and
  `config::remove_provider` fails because a file with no provider does
  not load.
- SPEC 4.1, 8.2 updated.

## Acceptance

- [x] Tests: each state chosen correctly; fix applies the modes; no
  button for a store path or another owner's file.
- [x] Checked by screenshot for each screen.

## Out of scope

Packaging, translations.
