# 01: Settings view and saving `config.toml`

Status: done. Blocked by: none.

## Goal

`,` opens a Settings view whose General section edits `config.toml` in
place, applied without a restart.

## Context

`docs/milestones/settings.md`. SPEC 4.2, 8.2 (account dashboard and
Recordings view for the full-window style), 8.3, 8.4. `src/config.rs`,
`src/refresh.rs`, `src/ui.rs`.

## Scope

- Writing through `toml_edit`, keeping comments, order and layout; a
  temporary file renamed over the original with its mode kept (settle
  in this ticket).
- A read-only or Nix store file makes the view read-only with the note.
- The view: `,` (also in the guide grid) and a status bar gear; section
  list (General, Providers, Player, Recording, Downloads, EPG) and
  fields; Tab, `j`/`k`, Enter, Esc as the milestone says; mouse.
  Providers and EPG show as empty for now.
- General: `refresh_hours` toggle and `active_provider` picker, saved on
  confirm and applied live (the check re-armed; the start provider used
  from the next start, as 4.2).
- Unknown keys stay an error; only known keys are written.
- Keys panel row. SPEC 4.2 (the section rule), 8.2, 8.3, 8.4 updated.

## Acceptance

- [x] Tests: edits keep comments and unrelated keys; mode kept; a
  managed file is not written; `refresh_hours` change re-arms the check.
- [x] Checked by screenshot against the demo provider, and a Nix-style
  read-only file.

## Out of scope

The other sections (02), providers (onboarding), EPG (EPG sources).
