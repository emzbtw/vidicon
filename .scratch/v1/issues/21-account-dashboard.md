# 21: Account dashboard

Status: done. Blocked by: none.

## Goal

A popup in the middle of the window, opened by clicking the account section
of the status bar, showing everything the provider says about the account.

## Context

SPEC 6.1 (account check), 6.3, ADR 0004 (credentials). `xtream.rs` parses
only `auth`, `status`, `exp_date` and `max_connections` from `user_info`.
The response also carries `active_cons`, `is_trial`, `created_at`,
`allowed_output_formats`, a provider `message`, and `server_info` (URL,
ports, timezone, server time).

## Secrets

- Username and password come from the credentials already in memory, never
  from the response. They are never written to the DB, logs or status bar.
- Username, password and server URL are masked until clicked, one at a
  time. Reveals reset when the popup closes, and it opens masked, so a
  snapshot (F12, `--screenshot`) shows masks unless something was revealed.
- Built, tested and screenshotted against `tools/demo/` and the fake in
  `src/fake_xtream.rs` only (extend both with the new fields). Never run
  against the real provider during development; checking it there is the
  user's job.

## Scope

- Design first: two or three mockups built in Slint against the demo
  provider, screenshots to the user, one picked before the real build.
- Parse the extra fields (all optional; panels omit and mistype them).
- Opening runs one fresh account check (through the provider's limiter) so
  active connections are current; show the cached values meanwhile.
- Close with Esc, a close button, or a click outside. A key to open it too.
- Update SPEC 6.3, 8.2 and 8.3.

## Acceptance

- [x] Tests: parsing with fields missing, as strings and as numbers.
- [x] Masked by default; each reveal is per field and gone after reopening.
- [x] No credential value appears in logs at `RUST_LOG=vidicon=debug`.
- [x] Screenshot of the chosen design in `docs/screenshots/`, demo data.

## Out of scope

Editing credentials (onboarding milestone). Copy to clipboard.
