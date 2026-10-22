# 06: M3U in the providers form

Status: open. Blocked by: 02, 04.

## Goal

An M3U provider can be added, edited and removed from the app, as an
Xtream one can.

## Context

`docs/milestones/m3u.md` (Providers form), `docs/milestones/onboarding.md`.
SPEC 4.1, 8.2. `src/config.rs` (file writing), `src/ui.rs`,
`ui/app.slint`.

## Scope

- A type choice in the add and edit form; M3U shows a URL-or-path field
  and an optional user agent. The type is fixed once saved.
- Saving fetches the playlist once and reports a reject with its reason,
  as the form reports a failed Xtream login.
- A `get.php?username=...&password=...` URL offers to add it as Xtream,
  the fields filled from it.
- The first-run setup screen offers both types.
- Settings hides Xtream-only items for an M3U provider (04's
  capabilities).
- SPEC 4.1, 8.2 updated.

## Acceptance

- [ ] Tests: the written `credentials.toml` for each type; the `get.php`
  detection, with no credentials kept in the M3U entry when switched.
- [ ] Checked by the user adding a real playlist from the form.
- [ ] `code-review` of the ticket's diff; findings fixed.

## Out of scope

Pasted text as a source (milestone, Out of scope).
