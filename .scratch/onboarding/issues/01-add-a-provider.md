# 01: Add a provider

Status: done. Blocked by: settings 01.

## Goal

A provider added in Settings › Providers is checked, written to
`credentials.toml` and usable at once.

## Context

`docs/milestones/onboarding.md`, `docs/milestones/settings.md`. SPEC
4.1, 6.2, 6.3 (account check), 8.2 (provider picker), ADR 0004.
`src/config.rs`, `src/xtream.rs`, `src/info.rs`, `src/ui.rs`.

## Scope

- Providers section: the providers listed, and an add form: name, URL,
  username, password (masked, a key shows it).
- The account check through the rate-limited client, showing expiry and
  max connections. `auth` 0 blocks; expired, unreachable or not Xtream
  warns with `Save anyway`.
- Id slug from the name, `-2` and up when taken; URL to its origin
  (settle both in this ticket).
- Written with `0600` and `0700`, through settings 01's writer; a
  managed file makes the section read-only.
- Live: the switcher lists it at once. "Add provider…" row in `p`.
- Credentials never in logs or errors (4.1); a test covers the form's
  paths.
- The demo provider accepts one invented account and refuses others.
- SPEC 4.1, 8.2, 8.3 updated.

## Acceptance

- [x] Tests: slugging and collisions; each check outcome; file mode
  after a write; comments kept; no credential in log output.
- [x] Checked against the demo provider: add, switch to it, play.

## Out of scope

Edit and remove (02), first run (03).

