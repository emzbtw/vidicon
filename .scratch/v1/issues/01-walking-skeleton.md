# 01: Walking skeleton

Status: done. Blocked by: none.

## Goal

The thinnest end-to-end path through every layer: credentials file to Xtream to
SQLite to a Slint list to mpv playing the selected channel. Crude everywhere,
but real. Later tickets deepen each layer.

## Context

SPEC 3 (architecture, paths), 4.1 (credentials), 6 (Xtream), 9.1 (player).
ADRs 0001, 0002, 0004, 0005. `CONTEXT.md` for terms.

## Scope

- Cargo crate `vidicon` at the repo root; add `target/` to `.gitignore`.
  Module skeleton per SPEC 3 (`config`, `xtream`, `store`, `import`, `player`,
  `ui`), even if some are near-empty.
- `config`: read `credentials.toml` from the SPEC path, the `--credentials`
  flag or `VIDICON_CREDENTIALS`. Enforce `0600` file / `0700` directory, with
  an error naming the `chmod`.
- Credential redactor for URLs (query-string and `/live/<user>/<pass>/`
  forms), used by all logging.
- `xtream`: account check, `get_live_categories`, `get_live_streams` for the
  first provider. Rate limiting per SPEC 6.2 from the start.
- `store`: SQLite file at the SPEC path, a minimal `channel` table; a plain
  (non-streaming) insert is acceptable here, ticket 02 replaces it.
- `ui`: a Slint window with one `ListView` of channel names, `j`/`k`/arrows,
  Enter plays. Inter font.
- `player`: spawn mpv per SPEC 9.1 and send `loadfile`; minimal.
- Update the flake devShell with any new build deps (verify names with
  mcp-nixos).
- **Dev tooling: snapshots**, behind a `devtools` cargo feature (in `default`
  features for `cargo run`; the eventual Nix package builds without it):
  - `--screenshot <path>`: wait for the first frame plus a short settle, save
    the window as PNG via `slint::Window::take_snapshot()`, exit.
  - F12: save a PNG to `$XDG_CACHE_HOME/vidicon/snapshots/<timestamp>.png`.
  - Window contents only (never the desktop); mpv is not captured.

## Verify against the real provider (records facts in SPEC)

- Stream URL shape and preferred extension (`allowed_output_formats`).
- `epg_channel_id` present on live streams (needed by ticket 05).
- Response sizes and timing for the real channel count.
- Tune the rate-limit numbers (SPEC open question 5) and update SPEC 6.2.

## Acceptance

- [x] `cargo run` with a valid credentials file shows the provider's channels
      and Enter plays the selected one in mpv.
- [x] Wrong permissions on the credentials file or its directory: refuses to
      start with a clear message.
- [x] Missing credentials file: clear message with the expected path and a
      pointer to the format (SPEC 4.1), not a crash or stack trace.
- [x] Unit tests: redactor covers both URL shapes; permission check.
- [x] No credentials in any log line, error or window title (checked by
      grepping a debug run's stderr for the username and password).
- [x] SPEC updated with the verified Xtream facts and rate-limit numbers.
- [x] `cargo run -- --screenshot /tmp/x.png` writes a PNG of the window and
      exits; F12 writes one to the snapshots dir. Neither exists in a
      `--no-default-features` build.

## Out of scope

Streaming import, categories UI, EPG, search, favourites, refresh scheduling,
styling.
