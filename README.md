<p align="center">
  <img src="assets/vidicon.svg" width="112" alt="Vidicon app icon">
</p>

<h1 align="center">Vidicon</h1>

<p align="center"><em>A fast, keyboard-driven IPTV player for Linux.</em></p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/licence-GPL--3.0--or--later-blue" alt="Licence: GPL-3.0-or-later"></a>
  <img src="https://img.shields.io/badge/Rust-1.92%2B-orange?logo=rust" alt="Rust 1.92+">
  <a href="#install"><img src="https://img.shields.io/badge/Nix-flake-5277C3?logo=nixos&logoColor=white" alt="Nix flake"></a>
  <img src="https://img.shields.io/badge/Linux-Wayland%20%7C%20X11-informational?logo=linux&logoColor=white" alt="Linux: Wayland or X11">
  <img src="https://img.shields.io/badge/source-Xtream%20Codes-555" alt="Source: Xtream Codes">
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#first-run">First run</a> ·
  <a href="#keys">Keys</a> ·
  <a href="#reporting-a-bug">Reporting a bug</a> ·
  <a href="#disclaimer">Disclaimer</a>
</p>

Vidicon browses an Xtream Codes provider's live channels, guide, movies and
series, and plays them in mpv.

![Vidicon: browsing channels, searching, the guide grid and movies](docs/screenshots/demo.gif)

Vidicon ships no channels, playlists or streams, and points to none. You
bring your own provider account ([Disclaimer](#disclaimer)).

## What it does

- Live channels by the provider's categories, with what is on now and next.
- A guide grid, scrollable through the days the provider sends.
- Fuzzy search over channels, movies and series.
- Favourites, recently played, and a last-channel key.
- Catch-up on channels the provider archives.
- Scheduled recordings, made by ffmpeg.
- Movies and series, with resume and downloads.
- External XMLTV guide sources, matched to channels by id or name.
- Reconnects a dropped stream, and lists a channel's other listings.
- Keyboard first (vim keys and arrows), with full mouse support.

It stays fast with large providers: on 50k channels the first frame is
under 300 ms from the local database (about 190 ms measured), and search
filters in under 8 ms a keystroke.

![The guide grid](docs/screenshots/guide-grid.png)

<table>
  <tr>
    <td width="50%"><img src="docs/screenshots/search-all-kinds.png" alt="Search across channels, movies and series"><br><sub>Search across channels, movies and series</sub></td>
    <td width="50%"><img src="docs/screenshots/series.png" alt="A series and its episodes"><br><sub>A series and its episodes</sub></td>
  </tr>
  <tr>
    <td width="50%"><img src="docs/screenshots/movies.png" alt="Movies"><br><sub>Movies</sub></td>
    <td width="50%"><img src="docs/screenshots/keys.png" alt="The keys panel"><br><sub>Every key, one press of <code>?</code> away</sub></td>
  </tr>
</table>

## What it does not do

- **M3U playlists or Stalker portals.** Xtream Codes is the only source
  (`docs/adr/0001-xtream-only-source.md`).
- **Play video itself.** mpv opens in its own window, driven over its IPC
  socket (`docs/adr/0002-external-mpv-over-ipc.md`).
- Windows, macOS, mobile or TV.

## Install

With Nix (flakes):

```sh
nix run github:emzbtw/vidicon
```

The package brings mpv, ffmpeg and the Inter font, and installs a desktop
entry. An mpv or ffmpeg already on your `PATH` comes first, with its config
and scripts.

Without Nix, you need:

- Rust 1.92 or newer, to build
- mpv, for playback
- ffmpeg, for recordings and downloads
- fontconfig
- Inter 4.0 or newer (older versions lack Inter Display, and headings fall
  back to another font)
- a Wayland or X11 session

```sh
cargo build --release --no-default-features
./target/release/vidicon
```

`--no-default-features` leaves out the developer tooling (screenshots, key
replay, the `--probe-*` flags).

## First run

With no credentials file, Vidicon opens a form for the provider's server
URL, username and password, checks the account, and saves it. Settings
(`,`) adds more providers, edits them, and adds guide sources.

| What | Path |
|---|---|
| Credentials | `$XDG_CONFIG_HOME/vidicon/credentials.toml` (`0600`, in a `0700` directory) |
| Settings | `$XDG_CONFIG_HOME/vidicon/config.toml` |
| Database | `$XDG_DATA_HOME/vidicon/vidicon.db` |

`--credentials <path>` or `VIDICON_CREDENTIALS` reads the credentials from
another file. Vidicon refuses a credentials file that other users can read.

## Keys

| Key | Does |
|---|---|
| `j` `k` or arrows | Move |
| `Enter` | Play |
| `/` | Search (`Tab` widens it to movies and series) |
| `t` | Guide grid, or back to the list |
| `f` | Favourite |
| `-` | Last channel |
| `Space` | Pause / resume |
| `m` | Mute |
| `F` | Fullscreen |
| `R` | Record what plays, or the selected programme in the guide |
| `<` `>` | A day back / ahead in the guide |
| `r` | Refresh |
| `,` | Settings |
| `q` | Quit |

Press `?` in the app for all of them.

## Reporting a bug

Say which view you were in and what you pressed. Logs go to stderr:

```sh
RUST_LOG=vidicon=debug vidicon 2> vidicon.log
```

Logs leave out passwords and stream URLs, but they name channels,
programmes and image hosts. Read the log before you attach it.

Every Xtream provider differs a little. If something fails only on your
provider, say so.

## Disclaimer

Vidicon is a player. It ships no channels, playlists, streams or guide
data, and points to no sources of them. It is not affiliated with any IPTV
provider, or with Xtream Codes.

You are responsible for having the right to watch what your provider
streams. Issues asking where to find streams or providers are closed.

## Credits

Vidicon is built on:

- [Slint](https://slint.dev): the UI toolkit.
- [mpv](https://mpv.io): playback.
- [FFmpeg](https://ffmpeg.org): recordings and downloads.
- [SQLite](https://sqlite.org), through
  [rusqlite](https://github.com/rusqlite/rusqlite): the local database.
- [Inter](https://rsms.me/inter/) by Rasmus Andersson (OFL-1.1): the
  typeface.
- [femtovg](https://github.com/femtovg/femtovg): the renderer. A patched
  copy is in `vendor/femtovg` (`vendor/README.md`).
- [nucleo](https://github.com/helix-editor/nucleo), from the Helix editor:
  fuzzy matching.
- [tokio](https://tokio.rs) and [reqwest](https://github.com/seanmonstar/reqwest):
  the async runtime and HTTP.

`Cargo.lock` has the full list of dependencies.

<a href="https://slint.dev">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/slint/MadeWithSlint-logo-light.svg">
    <img alt="Made with Slint" src="docs/slint/MadeWithSlint-logo-dark.svg" height="60">
  </picture>
</a>

## Licence

GPL-3.0-or-later (`LICENSE`).
