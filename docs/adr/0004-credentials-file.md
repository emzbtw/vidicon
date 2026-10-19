# 0004: Credentials in a permission-checked file outside the repo

Date: 2026-09-29. Status: accepted.

## Decision

Provider credentials live in `~/.config/vidicon/credentials.toml` by default,
overridable by flag or env var. The app refuses to start, naming the fix, if the
file is looser than `0600` or its directory looser than `0700` (as `ssh` does
for keys).

Credentials are never logged. Xtream embeds them in stream URLs, so the logger
redacts URLs, and they are kept out of mpv's window title.

## Why

Keeps secrets out of the project and out of the app's general config. A plain
file at a known path fits sops-nix or agenix later with no app changes.

## Alternatives rejected

- **System keyring (Secret Service).** Workable, but adds a runtime dependency
  and doesn't compose with a Nix-managed secrets setup.
- **Plaintext in the app config.** Mixes secrets with settings people share.
