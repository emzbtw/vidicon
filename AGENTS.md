# AGENTS.md

Project-specific instructions. Inherits from `~/.claude/CLAUDE.md` (Claude Code)
or `~/.pi/agent/AGENTS.md` (Pi), whichever your tool reads globally. Behavioural
guidelines there still apply and are not repeated here.

## What this is

**Vidicon**: a fast, keyboard-driven IPTV player for Linux (NixOS, Wayland).
Built for one user first, may be published later. Licensed GPL-3.0-or-later
(`LICENSE`), matching Slint's open licence.

- `SPEC.md`: the v1 specification. Authoritative; read it before starting
  work. If it disagrees with an ADR, raise it.
- `CONTEXT.md`: the domain glossary. Use its terms.
- `.scratch/v1/`: v1 tickets with blocking edges. One ticket per session;
  clear context between tickets.
- `.scratch/guide-grid/`: guide grid milestone tickets, same rules.
- `.scratch/recording/`: recording milestone tickets, same rules.
- `.scratch/catch-up/`: catch-up milestone tickets, same rules.
- `.scratch/vod/`: movies and series milestone tickets, same rules.
- `.scratch/m3u/`: M3U playlists milestone tickets, same rules.
- `.scratch/stream-recovery/`: stream recovery milestone tickets, same rules.
- `.scratch/themes/`: themes milestone tickets (Noctalia palettes), same
  rules.
- `.scratch/refresh-panel/`, `.scratch/settings/`, `.scratch/onboarding/`,
  `.scratch/epg-sources/`: those milestones' tickets, same rules.
- `.scratch/website/`: website milestone tickets (`site/`, Astro and
  Starlight on GitHub Pages), same rules.
- `docs/adr/`: settled decisions and the reasoning. Don't relitigate them;
  if one looks wrong, say so.
- `research/reference-players.md`: notes on ynoTV, open-tv and IPTVnator,
  the players this one is reacting against.
- `docs/releasing.md`: releasing a version (bump, tag, AUR) and keeping the
  release CI's install tests current. Read it before any version bump.
- `site/`: the website (Astro and Starlight), deployed to
  `https://emzbtw.github.io/vidicon/` by `.github/workflows/site.yml`.

## Settled

- **Rust.**
- **Xtream Codes API was the only source in v1.** Data model is multi-provider;
  v1 UI has one active provider plus a switcher. ADR 0001, amended for external
  XMLTV guide sources (EPG sources milestone) and for M3U playlists as a
  second source type, live channels only (M3U milestone).
- **v1 scope: live channels, EPG (now/next), favourites.** No VOD, series or
  catch-up yet. EPG timeline grid is a later milestone:
  `docs/milestones/guide-grid.md`.
- **Playback is an external mpv driven over JSON IPC.** The app never renders
  video. ADR 0002.
- **SQLite, streaming import, paged queries.** Never hold the full channel list
  or EPG in memory. ADR 0003.
- **Refresh:** serve local data instantly, background refresh when older than
  12 h (6 h selectable), check every 10 to 15 min while running, manual
  refresh key. Rate-limit provider requests.
- **Favourites:** flat reorderable list, schema ready for named groups
  (parked 2026-10-02, SPEC 2), name fallback when provider IDs change.
- **Input:** keyboard first (vim keys and arrows), full mouse support.
- **Credentials** live in a file outside the repo with strict permissions.
  ADR 0004.
- **No real provider data in the repo**, in code, tests, docs, screenshots
  or commit messages: no hosts, account details, or channel, category or
  programme names and numbers from the provider, and no real broadcaster
  names either. Invent them (the brands in `tools/demo/` are a ready set).
  Measurements taken against the provider are rounded (`about 58k
  channels`) and name no third-party hosts.
- **Font:** Inter (Inter Display for headings), installed system-wide.
- **UI toolkit: Slint.** `.slint` markup plus Rust. Use `ListView` directly,
  never a component inheriting it (that silently disables virtualisation).
  ADR 0005.

- **Layout:** three-pane main screen, focus-list style search mode, guide
  strip as the later grid screen (SPEC 8.2). Reference code is on the
  `prototype/slint` branch; `prototype/iced` holds the rejected toolkit.
- **No login screen in v1:** credentials come from the file only. The
  onboarding milestone (planned 2026-10-12) adds a form that writes the
  same file; there is no keyring.

## Open

- Nothing at the moment. Rate limits were settled in ticket 01 (SPEC 6.2).

## Development environment

The Rust toolchain (`cargo`, `rustc`, `clippy`, `rustfmt`, `rust-analyzer`)
comes from this flake's devShell, not from the system. The shell also sets
`LD_LIBRARY_PATH` for the Wayland, xkbcommon, Vulkan, GL and fontconfig
libraries that winit loads at runtime.

`node` and `npm` for the website also come from the devShell. The site
builds with `cd site && npm ci && npm run build`, into `site/dist/`.

Dev tooling (`devtools` cargo feature, on by default): `--screenshot <path>`
saves a PNG of the window after the first frame and exits; `--keys "<seq>"`
replays keys first (e.g. `--keys "l j j /kxn"`), with playback suppressed
unless `--play` is given (pair it with a fake `mpv_path`); F12 saves a snapshot to
`$XDG_CACHE_HOME/vidicon/snapshots/`. Use these to check UI changes visually
instead of asking the user to describe the screen.

`--probe-archive <stream_id>` asks the provider how it serves that
channel's archive and prints shapes, counts and offsets only, no URLs or
names (catch-up ticket 01, SPEC 6.1); it runs headless mpv for a seek test.
`--probe-tables <id,...>` prints what each channel's guide table holds:
programme counts, reach, `has_archive` counts (SPEC 6.1).
`--probe-vod` measures movies and series the same way: lists, info,
ids, streams, mpv, a 20 s download and a few posters (VOD ticket 01).
`--probe-alternates` counts channels with alternates from the database
alone; `--probe-pause <stream_id>` pauses a channel in headless mpv for
1, 5 and 15 min, with and without mpv's reconnect options, about 54 min
on the account's one connection (stream recovery ticket 01).
`--probe-blackout <stream_id>` plays a channel in an mpv window until
it is closed and prints the black, frozen and silent spans ffmpeg's
detect filters find, with bitrate and mpv's CPU (ticket 05).
`--probe-epg <url-or-path>` streams an external XMLTV feed (gzip, xz or
plain) and prints counts only: size, channels, programmes, reach,
time offsets, matches against the first provider's stored channels by
id and by normalised name, and how its times compare with the
provider's guide (EPG sources ticket 01).
`--probe-m3u <url-or-path>` streams an M3U playlist and prints counts
only: entries, line endings, quoting, attributes, groups, duplicates,
option lines, URL shapes and header guide feeds. A URL is fetched again
5 min later to count which identity scheme loses fewer channels (M3U
ticket 01). It needs no credentials.

`VIDICON_FPS=1` scrolls one row per frame (after any `--keys`) and prints
frame stats every second; `VIDICON_FPS=time` in the guide grid moves the
time a minute per frame instead; `RUST_LOG=vidicon=debug` logs the first-frame,
search, `loadfile` and minute-refresh times. `docs/perf/v1.md` has the budgets, the last
numbers and how to re-measure (`docs/perf/guide-grid.md` for the grid).

`vendor/femtovg` is a patched copy of a dependency (`vendor/README.md`).
Leave it as it is: no formatting, no clean-ups.

`docs/screenshots/` showcases the UI, one kebab-case file per view
(`main-guide.png`, `search.png`); replace a file when its view changes.
Committed screenshots show invented data only: take them against the fake
provider in `tools/demo/` (see its README), never the real one.

direnv loads the devShell automatically via `.envrc`. If a command fails with
`cargo: command not found`, the environment did not load: run `direnv allow` in
the repo root, or prefix the command with `nix develop -c`.
