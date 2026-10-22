# 06: M3U in the providers form

Status: done. Blocked by: 02, 04.

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
  the fields filled from it. It offers this when the playlist fetch is
  refused too: a panel can refuse `get.php` (HTTP 884, empty body)
  while its API works (SPEC 6.4).
- The first-run setup screen offers both types.
- Settings hides Xtream-only items for an M3U provider (04's
  capabilities).
- SPEC 4.1, 8.2 updated.

## Acceptance

- [x] Tests: the written `credentials.toml` for each type; the `get.php`
  detection, with no credentials kept in the M3U entry when switched.
  Also the form's check (`check_playlist`: channels, rejects, an 884
  refusal, a path). Checked against the demo provider: an M3U add
  writes `type`, `url` and no login at `0600`; a `get.php` link
  switched to Xtream saves its origin and login and never fetches it.
- [x] Checked by the user adding a real playlist from the form.
- [x] `code-review` of the ticket's diff; findings fixed: the hint on
  `Add as Xtream instead` and `Type` while a warning shows, a playlist
  field that could not be cleared, the form's value slots named, a SPEC
  line wrapped. Kept: Downloads stays with an M3U provider active (one
  setting for every provider, SPEC 8.2); the check's fetch is not reused
  by the first refresh, which fetches again.

## Out of scope

Pasted text as a source (milestone, Out of scope).
