# Milestone: onboarding

First written as "only needed if Vidicon is published". Planned
2026-10-12, after settings (`settings.md`), whose file-writing it uses.
Tickets in `.scratch/onboarding/`. v1 reads credentials from a file only
(SPEC 4.1, ADR 0004); this milestone adds a UI over the same file.

## Why now

Adding, editing or removing a provider means editing
`credentials.toml` by hand and restarting, and a missing or too-open
file stops the app with a message on stderr, unseen when it is started
from a launcher.

## Requirements

- **Storage stays file-based (ADR 0004):** the form writes
  `credentials.toml` through the settings mechanism, with `0700`
  directory and `0600` file permissions. A managed file (read-only or a
  Nix store symlink) makes the Providers section read-only, saying so.
  No system keyring.
- **Providers section of Settings:** the providers, with add, edit and
  remove. The `p` switcher gains an "Add provider…" row that opens the
  form there.
- **First-run screen:** with no `credentials.toml`, or one with no
  provider, a full-window screen explains what is missing and shows the
  same add-provider form. Once a provider is saved, the main screen and
  the first refresh (`refresh-panel.md`) take over.
- **Add-provider form:** display name, server URL, username, password.
  Before saving it runs the account check (SPEC 6.3) through the
  rate-limited client (6.2) and shows expiry and max connections.
  - Wrong credentials (`auth` 0) block saving.
  - An expired account, an unreachable server or a reply that is not
    Xtream warn, naming which, with `Save anyway`.
- **Provider id:** made from the display name at creation (`My provider`
  → `my-provider`, `-2` and up when taken) and never changed. Editing
  changes name, URL, username and password; a new URL or username keeps
  all data, and the next refresh reconciles as now.
- **Changes apply live:** a new provider shows in the switcher at once;
  new credentials are used from the next request.
- **Remove:** confirm dialog with counts. Deletes the provider's block
  and its bulk data (channels, EPG, movie and series lists, guide
  tables) and cancels its scheduled recordings. Keeps its favourites,
  progress and recording history, so a provider re-added with the same
  id gets them back, unless "Also delete favourites, progress and
  recording history" is ticked. Recorded and downloaded files stay on
  disk either way. Settled in ticket 02: the box reads `Also delete
  favourites, progress and history` and also covers recently played,
  category and channel rules and kept tags. Settled in ticket 03:
  removing the last provider shows the setup screen.
- **Too-open permissions:** the refusal (ADR 0004) shows in the window,
  naming the file and the `chmod`, with a `Fix permissions` button that
  sets `0600` on the file and `0700` on its directory and carries on.
  The button shows only when the user owns the file and it is not in the
  Nix store. Nothing is read until it is fixed.
- The password field is masked, with a key to show it, as the account
  dashboard's `1`–`3`.
- SPEC 4.1, 6.3, 8.2, 8.3, 8.4 updated.

## Proposed (settle in the tickets)

- The id slug: lowercase ASCII letters, digits and `-`; a name with none
  of those gets `provider`.
- URL normalised to its origin (4.1) on save, a path or query given
  being dropped with a note.
- The first-run screen's checks of the file run in the same order as at
  start today: missing, permissions, parse, then providers. Settled in
  ticket 03, which calls it the setup screen (the first-run screen is
  the refresh panel's, SPEC 8.2).
