# Reference IPTV players: ynoTV, Fred TV (open-tv), IPTVnator

Researched 2026-09-29 from primary sources only: shallow clones of each repo,
their manifests and in-repo docs, GitHub releases, and GitHub issues (via `gh`).
All code links are permalinks to the commit that was read:

| Repo | Commit read | Latest release | License |
| --- | --- | --- | --- |
| [tbeezy/ynotv](https://github.com/tbeezy/ynotv) | `be73521` (2026-09-22) | v2.5.6, 2026-09-23 | AGPL-3.0 ([LICENSE](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/LICENSE)) |
| [Fredolx/open-tv](https://github.com/Fredolx/open-tv) | `1bfa78a` (2026-04-20) | v1.9.1, 2026-01-19 | GPL-2.0 ([LICENSE](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/LICENSE)) |
| [4gray/iptvnator](https://github.com/4gray/iptvnator) | `bcc6186` (2026-09-29) | v0.24.0, 2026-09-24 | MIT ([LICENSE.md](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/LICENSE.md)) |

Link prefixes used below (all permalinks):

- `Y/` = `https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/`
- `O/` = `https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/`
- `I/` = `https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/`

Every link is written out in full so it can be clicked.

Issue comment counts were taken on 2026-09-29 with
`gh issue list --state all --search "sort:comments-desc"`.

---

## 1. ynoTV (tbeezy/ynotv)

Youngest of the three (repo created 2026-02-17) and by far the most feature-heavy.
Fast release cadence: v2.5.6 is the Cargo version
([Cargo.toml#L3](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/Cargo.toml#L3)).

### 1.1 Tech stack and platforms

- **Tauri v2 + Rust backend, React 19 + TypeScript UI**, built with Vite, pnpm monorepo
  (`packages/core`, `packages/local-adapter`, `packages/ui`, `packages/app`)
  ([package.json](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/package.json),
  [packages/ui/package.json#L38](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/ui/package.json#L38)).
  UI state uses zustand and list virtualization uses `@tanstack/react-virtual`
  ([packages/ui/package.json#L18](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/ui/package.json#L18), [#L43](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/ui/package.json#L43)).
- **Platforms: Windows only in practice.** README badge says Windows
  ([README.md#L7](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/README.md#L7));
  the release workflow runs only on `windows-latest` and publishes `.exe`/`.msi`
  ([release-tauri.yml#L18](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/.github/workflows/release-tauri.yml#L18));
  the v2.5.6 release ships a single `ynoTV_2.5.6_x64-setup.exe`. There is macOS code
  (`mpv_macos.rs`, `mpv_render_mac.rs`, objc2 deps in
  [Cargo.toml](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/Cargo.toml))
  but no macOS build is published. No Linux, mobile, web or TV target was found.

### 1.2 Source types and parsing

- **M3U/M3U8**: parsed in TypeScript in the webview. The whole body is fetched via
  `@tauri-apps/plugin-http`, then `content.split('\n')` and a line loop extracts
  `#EXTM3U url-tvg`, catch-up header defaults and `#EXTINF` attributes
  ([m3u-parser.ts#L198-L230](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/local-adapter/src/m3u-parser.ts#L198-L230),
  called from [sync.ts#L2639](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/ui/src/db/sync.ts#L2639)).
  Not streaming: the whole playlist is held as a string and an array.
- **Xtream Codes**: TS client for `player_api.php` actions (`get_live_categories`,
  `get_live_streams`, VOD, series, `get_short_epg`)
  ([xtream-client.ts#L1-L10](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/local-adapter/src/xtream-client.ts#L1-L10), [#L590-L595](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/local-adapter/src/xtream-client.ts#L590-L595)).
- **Stalker / MAG portal**: a ~2,900-line TS client with MAC-based handshake, token
  refresh and retry on auth failure
  ([stalker-client.ts#L13-L17](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/local-adapter/src/stalker-client.ts#L13-L17), [#L590-L660](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/local-adapter/src/stalker-client.ts#L590-L660)).
- **XMLTV**: parsed in Rust with `quick-xml` as a single streaming pass through an
  optional gzip decoder; batches are spooled and swapped in only after the download
  is verified, so a partial download never replaces good data
  ([epg_streaming.rs#L1-L17](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/epg_streaming.rs#L1-L17),
  invoked from [epg-streaming.ts#L256](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/ui/src/services/epg-streaming.ts#L256)).
  A legacy regex-based TS XMLTV parser still exists
  ([xmltv-parser.ts#L20-L30](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/local-adapter/src/xmltv-parser.ts#L20-L30)).
  Multiple EPG sources per playlist are supported (README features list,
  [README.md#L39-L41](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/README.md#L39-L41)).
- **EPG matching**: tvg-id first, then normalized-name fuzzy matching that strips
  provider prefixes (`prime:`, `il:`), brackets and superscript badges like `ᴴᴰ`
  ([epg_streaming.rs#L579-L625](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/epg_streaming.rs#L579-L625)),
  plus an EPG editor with a tunable string-similarity matcher
  ([EpgEditorModal.tsx#L3009](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/ui/src/components/EpgEditorModal.tsx#L3009)).
- **Others**: Jellyfin, Stremio addons, Nuvio, local media folders with TMDB matching
  ([README.md#L42-L52](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/README.md#L42-L52)).

### 1.3 Playback engine

mpv, embedded in the app window, with two engines selectable on Windows:

- **Sidecar (default on Windows)**: a bundled `mpv` executable spawned as a Tauri
  sidecar with `--wid=<HWND>` so it renders as a child window, controlled over a
  named-pipe JSON IPC
  ([mpv_windows.rs#L1-L14](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/mpv_windows.rs#L1-L14), [#L412](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/mpv_windows.rs#L412), [#L455](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/mpv_windows.rs#L455)).
  The Tauri window is `transparent: true`, so the React UI overlays the mpv window
  ([tauri.conf.json#L23](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/tauri.conf.json#L23)).
- **In-process libmpv** via the `libmpv2` crate (always on macOS, opt-in via the
  `playerEngine` setting elsewhere)
  ([lib.rs#L265-L290](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/lib.rs#L265-L290),
  [mpv_core.rs#L1-L2](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/mpv_core.rs#L1-L2)).
- **Multiview** slots 2 to 4 use a separate libmpv software-render path: frames are
  rendered to RGBA buffers and streamed over Tauri IPC channels into `<canvas>`
  elements, with backpressure
  ([mpv_canvas.rs#L1-L3](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/mpv_canvas.rs#L1-L3), [#L272-L273](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/mpv_canvas.rs#L272-L273)).
- A popout mpv window and external players are also supported
  ([mpv_popout.rs](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/mpv_popout.rs), README).

### 1.4 Storage and large lists

- **SQLite** at `%LOCALAPPDATA%\com.ynotv.app\app.db`, holding channels, categories,
  EPG (README says a 7-day window), VOD, DVR, etc.; settings in a separate
  `settings.json` store
  ([README.md#L159-L176](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/README.md#L160-L176)).
  WAL mode ([db/index.ts#L595](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/ui/src/db/index.ts#L595)),
  indexes on `channels(source_id)`, `name COLLATE NOCASE`, `epg_channel_id`
  ([db/index.ts#L1107-L1110](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/ui/src/db/index.ts#L1107-L1110)).
  Search uses `LIKE` queries, not FTS
  ([sqlite-adapter.ts#L950](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/ui/src/db/sqlite-adapter.ts#L950)).
- Two SQLite access paths: the UI uses `tauri-plugin-sql` directly, and bulk writes go
  through Rust `rusqlite` commands (`bulk_upsert_channels`, etc.) to cut IPC overhead
  ([db_bulk_ops.rs#L1-L5](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/db_bulk_ops.rs#L1-L5),
  [bulk-ops.ts#L193](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/ui/src/services/bulk-ops.ts#L193)).
  Both have "database is locked" retry wrappers, and EPG writes are serialized
  behind a mutex because concurrent source syncs were thrashing the single SQLite
  writer ([epg_streaming.rs#L30-L40](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/epg_streaming.rs#L30-L40),
  [db_bulk_ops.rs#L13-L30](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/db_bulk_ops.rs#L13-L30)).
- EPG indexes are dropped and recreated around bulk inserts
  ([epg_streaming.rs#L3209-L3228](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/epg_streaming.rs#L3209-L3228)).
- The project documents databases reaching ~5 GB (from "Cache Entire EPG" re-inserting
  a global EPG on every sync) that hang startup, and ships a recovery screen
  ([docs/recovery-build.md#L1-L25](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/docs/recovery-build.md#L1-L25)).
- No published benchmark for 50k+ channel lists was found.

### 1.5 Notable features (README, spot-checked in code)

From [README.md#L39-L80](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/README.md#L39-L80):

- **Catch-up** for M3U (`catchup-source` templates, append/shift/flussonic)
  ([m3u-catchup.ts#L1-L6](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/local-adapter/src/m3u-catchup.ts#L1-L6)), Xtream and Stalker.
- **Timeshift** from mpv's demuxer cache (`demuxer-cache-state`), not a disk buffer
  ([useTimeshift.ts#L12-L17](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/ui/src/hooks/useTimeshift.ts#L12-L17)).
- **DVR**: scheduled and manual recording with FFmpeg child processes and a cron scheduler
  ([dvr/recorder.rs#L1-L4](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/dvr/recorder.rs#L1-L4)).
- **Multiview** layouts `main | pip | 2x2 | bigbottom | sbs`
  ([useMultiview.ts#L5](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/ui/src/hooks/useMultiview.ts#L5)).
- **Failover groups** / automatic stream fallback, backup DNS, stream probe and bulk
  channel health checks ([stream_probe.rs](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/stream_probe.rs)).
- **Casting**: Chromecast via a patched `rust_cast` plus mDNS discovery
  ([cast.rs#L1-L10](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/cast.rs#L1-L10)).
- **Remote control**: phone remote served by an embedded axum HTTP/WebSocket server
  with token pairing ([web_server.rs#L1-L30](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/web_server.rs#L1-L30));
  gamepads via gilrs (XInput) and raw HID
  ([Cargo.toml](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/Cargo.toml)).
- Favorites, custom groups, playlist editor with M3U export, EPG editor, watchlist
  with auto-switch, TMDB/Trakt/Simkl, Discord presence, OpenSubtitles, 24 languages.
- **Parental controls: none found.** A grep for parental/PIN matched only Simkl
  device-code login, not a content lock.

### 1.6 Architecture

- Process model: Tauri main process (Rust) + WebView2 renderer (React) + mpv
  (sidecar process or in-process libmpv) + FFmpeg processes for DVR.
- Data flow: provider fetching and M3U/Xtream/Stalker parsing happen in TypeScript in
  the renderer (`local-adapter`), orchestrated by
  [db/sync.ts](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/ui/src/db/sync.ts)
  (~4,800 lines), then rows go to SQLite through Rust bulk commands. XMLTV goes the
  other way: Rust downloads, parses and writes directly, emitting progress events.
- Rust `lib.rs` is ~5,800 lines and registers modules for mpv, DVR, EPG, cast,
  web server, gamepad, Jellyfin, tray, etc.
  ([lib.rs#L241-L330](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/packages/app/src-tauri/src/lib.rs#L241-L330)).

### 1.7 Known pain points (from issues)

- **Embedded video geometry**: video off-centre in the embedded player
  ([#97](https://github.com/tbeezy/ynotv/issues/97), 24 comments, most-discussed),
  fullscreen exit problems ([#156](https://github.com/tbeezy/ynotv/issues/156)),
  "mpv player in the background" ([#145](https://github.com/tbeezy/ynotv/issues/145)),
  popout issues ([#83](https://github.com/tbeezy/ynotv/issues/83)). This is the cost of
  overlaying a transparent webview on a separate mpv window.
- **Startup/DB size**: app vanishes after adding an Xtream portal
  ([#168](https://github.com/tbeezy/ynotv/issues/168)), traced to huge databases
  ([recovery-build.md](https://github.com/tbeezy/ynotv/blob/be73521c1d0b365444fafbfe09608e0c7733e9d1/docs/recovery-build.md)).
- **Freezes and memory**: freezes with 7 Stalker sources
  ([#211](https://github.com/tbeezy/ynotv/issues/211)); 10 GB RAM after v2.4.0
  ([#184](https://github.com/tbeezy/ynotv/issues/184), fixed, back to ~100 MB);
  WebView2 GPU process at 99% caused by a theme
  ([#96](https://github.com/tbeezy/ynotv/issues/96)); random closes
  ([#131](https://github.com/tbeezy/ynotv/issues/131)).
- **Channel start latency**: slower zapping after v2.4.0, root cause was the
  "Normalize Loudness" audio filter ([#227](https://github.com/tbeezy/ynotv/issues/227)).
- **EPG mapping**: requests for manual mapping by name
  ([#36](https://github.com/tbeezy/ynotv/issues/36)), multiple EPG URLs
  ([#39](https://github.com/tbeezy/ynotv/issues/39), [#46](https://github.com/tbeezy/ynotv/issues/46)),
  source priority not working ([#171](https://github.com/tbeezy/ynotv/issues/171)),
  EPG overrides reset on source update ([#240](https://github.com/tbeezy/ynotv/issues/240)).
- **DVR reliability**: scheduled recording stops when playback is active
  ([#67](https://github.com/tbeezy/ynotv/issues/67)), recordings disappearing
  ([#121](https://github.com/tbeezy/ynotv/issues/121)).
- **Provider bans**: suspected IP blocks from request volume
  ([#110](https://github.com/tbeezy/ynotv/issues/110)).

---

## 2. Fred TV, formerly Open TV (Fredolx/open-tv)

Most-starred desktop project of the three by Rust-first design (4,043 stars), small
and deliberately minimal. Single maintainer; last push 2026-06-14.

### 2.1 Tech stack and platforms

- **Tauri v2 + Rust (edition 2024) backend, Angular 17 UI** with Angular Material and
  ng-bootstrap ([Cargo.toml](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/Cargo.toml),
  [package.json#L19-L33](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/package.json#L19-L33)).
- **Desktop**: Windows `.msi`, macOS universal `.dmg`, Linux `.deb`/`.rpm` (v1.9.1
  release assets), plus Flatpak, AUR, MS Store and a Docker image
  ([README.md#L5-L19](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/README.md#L5-L19), [#L53-L80](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/README.md#L60-L87)).
- **Mobile**: README links App Store and Google Play, but this repo has no mobile
  target; the author has a separate `Fredolx/fred-tv-mobile` repo ("Fred TV for
  Android and iOS") that was not examined here.

### 2.2 Source types and parsing

- **M3U file / M3U link**: links are downloaded to a temp file first
  ([m3u.rs#L173-L195](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/m3u.rs#L173-L195)),
  then read line by line with a `BufReader` and inserted inside a single SQLite
  transaction, followed by `ANALYZE`. Attributes are pulled with per-field regexes
  (`tvg-name`, `tvg-id`, `tvg-logo`, `group-title`), and `#EXTVLCOPT` sets
  per-channel origin/referrer/user-agent
  ([m3u.rs#L22-L40](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/m3u.rs#L22-L40), [#L51-L117](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/m3u.rs#L51-L117)).
  User edits (favorites, hidden) are preserved across refresh via a
  preserve/restore step (same lines).
- **Xtream Codes**: Rust client for `get_live_streams`, `get_vod_streams`,
  `get_series`, `get_series_info`, categories
  ([xtream.rs#L31-L39](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/xtream.rs#L31-L39)).
- **EPG**: Xtream only, fetched per channel on demand with `get_simple_data_table`
  ([xtream.rs#L512-L529](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/xtream.rs#L512-L529)).
  **No XMLTV parser exists.** `epg.rs` only polls for "program is live" desktop
  notifications ([epg.rs](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/epg.rs)).
  The request is still open ([#147](https://github.com/Fredolx/open-tv/issues/147)),
  as is external EPG URL for Xtream ([#286](https://github.com/Fredolx/open-tv/issues/286), [#220](https://github.com/Fredolx/open-tv/issues/220)).
- **Stalker**: not implemented. `stalker.rs` is an empty file
  ([stalker.rs](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/stalker.rs));
  the MAC-login request was closed as "the app is coming"
  ([#70](https://github.com/Fredolx/open-tv/issues/70)).
- **Custom channels/groups/sources**, shareable between users
  ([share.rs](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/share.rs)).

### 2.3 Playback engine

- **External mpv process, one window per stream.** The app spawns the `mpv` binary
  with `tokio::process::Command`, passing `--hwdec=auto`, `--vo=gpu-next`,
  `--profile=high-quality` (optional), `--cache`, HTTP header fields, user agent,
  `--stream-record=` for recording, and a yt-dlp path
  ([mpv.rs#L19-L43](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/mpv.rs#L19-L43), [#L70-L128](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/mpv.rs#L70-L128)).
  Errors are read from mpv stdout after exit.
- No IPC control of mpv: the app only waits for exit or kills it. It enforces the
  provider's max concurrent streams by killing the oldest stream
  ([utils.rs#L186-L201](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/utils.rs#L186-L201)).
- mpv, ffmpeg and yt-dlp are runtime dependencies users must install themselves on
  macOS/Linux (Windows `.msi` bundles mpv)
  ([README.md#L38-L59](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/README.md#L38-L52)).

### 2.4 Storage and large lists

- **SQLite via rusqlite + r2d2 pool (max 20 connections)**
  ([sql.rs#L24-L32](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/sql.rs#L24-L32)),
  schema with indexes on name, source, group, favorite, media type, hidden
  ([sql.rs#L88-L103](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/sql.rs#L88-L103)),
  migrations via `rusqlite_migration`.
- **UI never holds the full list.** Every view is a paged SQL query, 36 items per
  page, with `LIKE %keyword%` search (optionally per-keyword AND)
  ([sql.rs#L22](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/sql.rs#L22), [#L454-L537](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/sql.rs#L454-L537)).
  This is why the README can claim "super low RAM usage" and "instant search"
  ([README.md#L31](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/README.md#L31)).
  No FTS; a leading-wildcard `LIKE` cannot use the name index, so search is a scan
  (inference from the SQL).
- No EPG storage, since EPG is fetched live per channel.

### 2.5 Notable features

From [README.md#L26-L36](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/README.md#L26-L36) and the command list in
[lib.rs](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/lib.rs):

- Record while watching (mpv `--stream-record`); VOD download manager.
- **Catch-up**: Xtream `timeshift.php` URLs built from EPG entries
  ([xtream.rs#L570-L594](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/xtream.rs#L570-L594)); no M3U catch-up found.
- **Re-stream**: FFmpeg remuxes a channel to local HLS (`-c copy -f hls`) served by an
  embedded warp HTTP server for other devices on the LAN
  ([restream.rs#L33-L70](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/restream.rs#L33-L70)).
- Favorites (with backup/restore), history, hide channels/groups, multiple sources,
  keyboard/TV-remote navigation, program-start notifications.
- Multiple simultaneous streams only as separate mpv windows (limited by max-streams).
- No parental controls, casting, multiview UI or grid EPG found.

### 2.6 Architecture

- Tauri main process (Rust) does all fetching, parsing and SQL; the Angular webview is
  a thin paged view calling commands like `search`, `get_xtream`, `play`
  ([lib.rs](https://github.com/Fredolx/open-tv/blob/1bfa78a181166025a6b977d783f895e819ff720f/src-tauri/src/lib.rs)).
  Playback is fully delegated to external mpv processes; FFmpeg processes for re-stream.
- Whole backend is ~4,900 lines of Rust, of which `sql.rs` is ~1,800.

### 2.7 Known pain points (from issues)

- **External-player dependency**: mpv won't launch from Flatpak
  ([#47](https://github.com/Fredolx/open-tv/issues/47)), macOS "No such file or
  directory" ([#61](https://github.com/Fredolx/open-tv/issues/61)), M1 playback
  ([#21](https://github.com/Fredolx/open-tv/issues/21)), deprecated library
  ([#328](https://github.com/Fredolx/open-tv/issues/328)). The standard support answer
  is "install latest mpv/ffmpeg/yt-dlp" ([#378](https://github.com/Fredolx/open-tv/issues/378)).
- **Playback stability**: streams closing without error
  ([#79](https://github.com/Fredolx/open-tv/issues/79)), freezing after the rename
  release ([#360](https://github.com/Fredolx/open-tv/issues/360)), ffmpeg/RTSP errors
  ([#143](https://github.com/Fredolx/open-tv/issues/143)), fps drop when window
  unfocused ([#89](https://github.com/Fredolx/open-tv/issues/89)), audio-track switch
  breaks audio ([#378](https://github.com/Fredolx/open-tv/issues/378), open).
- **Missing EPG/guide** is the top long-running feature gap
  ([#64](https://github.com/Fredolx/open-tv/issues/64), [#147](https://github.com/Fredolx/open-tv/issues/147),
  [#116](https://github.com/Fredolx/open-tv/issues/116), [#227](https://github.com/Fredolx/open-tv/issues/227)).
- **Xtream robustness**: "Too many Xtream requests failed"
  ([#275](https://github.com/Fredolx/open-tv/issues/275), open; [#136](https://github.com/Fredolx/open-tv/issues/136)),
  login failures ([#144](https://github.com/Fredolx/open-tv/issues/144)),
  M3U parse errors ([#67](https://github.com/Fredolx/open-tv/issues/67), [#78](https://github.com/Fredolx/open-tv/issues/78)).
- **State persistence**: hidden categories not persisting
  ([#352](https://github.com/Fredolx/open-tv/issues/352)).

---

## 3. IPTVnator (4gray/iptvnator)

Oldest (2019) and most-starred (7,231). Large Nx monorepo with extensive in-repo
architecture docs under `docs/architecture/`, which are cited below alongside code.

### 3.1 Tech stack and platforms

- **Electron 43 + Angular 22 + NgRx**, Nx 23 workspace, `better-sqlite3` with
  drizzle-orm ([package.json](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/package.json),
  [AGENTS.md#L3](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/AGENTS.md#L3)).
- **Desktop**: Windows (nsis x64/ia32), macOS (x64/arm64), Linux (AppImage, deb, snap,
  rpm, pacman, flatpak; x64/arm64/armv7l)
  ([electron-builder.json](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/electron-builder.json), v0.24.0 release assets).
- **Web/PWA**: installable PWA plus a Docker image bundling the PWA and a
  `web-backend` proxy for M3U parsing and Xtream/Stalker requests
  ([README.md#L122-L140](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/README.md#L120-L140),
  [docs/architecture/pwa-self-hosted.md](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/pwa-self-hosted.md)).
  EPG, embedded MPV, external players, downloads and remote control are desktop-only.
- No native mobile or TV app.

### 3.2 Source types and parsing

- **M3U/M3U8** (file, text, URL): parsed with a pinned fork of
  `freearhey/iptv-playlist-parser` that adds a `radio` attribute, strips `|User-Agent=`
  suffixes into headers, keeps `#KODIPROP` lines, and removes URL validation (long JWT
  URLs broke parsing, #1189)
  ([m3u-playlist-module.md#L146-L158](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/m3u-playlist-module.md#L146-L158)).
  Refresh runs in a one-shot `worker_threads` worker, parsing the whole string
  (`parse(result.data)`)
  ([playlist-refresh.worker.ts#L138](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/apps/electron-backend/src/app/workers/playlist-refresh.worker.ts#L138)).
- **Xtream Codes**: separate Electron (SQLite-backed) and PWA data sources, selected by
  a capability check
  ([xtream-portal-compatibility.md#L1-L25](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/xtream-portal-compatibility.md#L1-L25)).
- **Stalker/Ministra**: live, radio, VOD, series, favorites
  ([stalker-portal.md#L1-L25](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/stalker-portal.md#L1-L25),
  [stalker-epg.md](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/stalker-epg.md)).
- **XMLTV**: streaming SAX parse with `saxes` in a worker thread, optional gunzip,
  written to SQLite
  ([epg-streaming-parser.ts#L1](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/apps/electron-backend/src/app/workers/epg-streaming-parser.ts#L1), [#L89-L112](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/apps/electron-backend/src/app/workers/epg-streaming-parser.ts#L89-L112),
  [epg-optional-gunzip.ts#L2](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/apps/electron-backend/src/app/workers/epg-optional-gunzip.ts#L2),
  [epg-parser.worker.ts](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/apps/electron-backend/src/app/workers/epg-parser.worker.ts)).
  Playlist-declared, manual and local-file EPG sources are supported
  ([m3u-playlist-module.md#L917](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/m3u-playlist-module.md#L917), [#L1167](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/m3u-playlist-module.md#L1167)).
- **EPG matching**: manual mappings first, then key chain `tvg.id -> tvg.name -> name`
  ([m3u-playlist-module.md#L1350-L1356](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/m3u-playlist-module.md#L1350-L1356)).
  No fuzzy normalization comparable to ynoTV was found.
- **DASH + ClearKey** via Shaka Player, driven by `#KODIPROP` license lines
  ([m3u-playlist-module.md#L1461](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/m3u-playlist-module.md#L1461)).

### 3.3 Playback engine

Multiple engines, chosen per user setting and per URL:

- **Built-in web players**: ArtPlayer (default), Video.js, plain HTML5 `<video>`, and
  an audio player for radio
  ([m3u-playlist-module.md#L1261-L1272](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/m3u-playlist-module.md#L1261-L1272)).
  URL routing: `.m3u8` to hls.js, `.mpd` to Shaka, `.ts` and extension-less
  `/live/user/pass/1` URLs to mpegts.js, everything else to native `<video>`
  ([playback-source-routing.ts#L19-L46](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/libs/playback/util/src/lib/playback-source-routing.ts#L19-L46)).
  Codec support is therefore limited to what Chromium decodes (see pain points).
- **External players**: mpv, VLC, IINA (desktop)
  ([README.md#L37](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/README.md#L37)).
- **Embedded mpv (experimental)** through a native Node addon, different per OS:
  macOS libmpv render API into an `NSOpenGLView` (the `wid` path rendered black);
  Windows libmpv with `wid` on a child `HWND`; Linux a separate `mpv --wid` process on
  an X11/Xwayland child window with JSON IPC, because loading libmpv in-process
  clashed with Electron's own FFmpeg/GL symbols. Native Wayland is not implemented.
  An experimental "frame-copy" engine renders in a helper process and copies frames
  via shared memory into a WebGL canvas
  ([embedded-mpv-native.md#L5-L60](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/embedded-mpv-native.md#L5-L60)).

### 3.4 Storage and large lists

- **SQLite** (`better-sqlite3` + drizzle) at `~/.iptvnator/databases/iptvnator.db`
  ([m3u-playlist-module.md#L26-L31](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/m3u-playlist-module.md#L27-L30)),
  schema in [schema.ts](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/libs/shared/database/src/lib/schema.ts).
  Earlier versions (v0.19) stored sources in IndexedDB; there is a one-way migration
  ([m3u-playlist-module.md#L16-L50](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/m3u-playlist-module.md#L16-L50)).
- **M3U channels are stored as one JSON blob** in `playlists.payload`
  ([schema.ts#L26-L58](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/libs/shared/database/src/lib/schema.ts#L26-L58),
  [playlist.operations.ts#L238](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/apps/electron-backend/src/app/database/operations/playlist.operations.ts#L238))
  and loaded in full into the NgRx store as `channels: Channel[]`
  ([m3u-playlist-module.md#L469-L490](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/m3u-playlist-module.md#L469-L490)).
  Even saving favorites rewrote the whole payload until it was avoided
  ([m3u-playlist-module.md#L600-L606](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/m3u-playlist-module.md#L600-L606)).
  Xtream catalogs, by contrast, are normalized into `categories`/`content` tables.
- **Scale handling**: CDK virtual scroll for "90,000+ channels", OnPush, 300 ms search
  debounce ([m3u-playlist-module.md#L9](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/m3u-playlist-module.md#L9), [#L1053-L1063](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/m3u-playlist-module.md#L1053-L1063)).
  An E2E benchmark imports generated 10k, 50k and 100k channel M3Us
  ([m3u-playlist-module.md#L190-L200](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/m3u-playlist-module.md#L190-L200));
  no result numbers are committed.
- **Worker threads**: heavy non-EPG SQLite work moved off Electron's main thread into
  a long-lived DB worker after it froze the UI
  ([sqlite-db-worker.md#L10-L30](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/sqlite-db-worker.md#L10-L30)).
- **EPG** in `epg_channels`/`epg_programs` with channel/time-range composite indexes
  ([schema.ts#L234-L270](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/libs/shared/database/src/lib/schema.ts#L234-L270))
  and **FTS5** tables for EPG program and content title search
  ([connection.ts#L236](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/libs/shared/database/src/lib/connection.ts#L236), [#L305-L306](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/libs/shared/database/src/lib/connection.ts#L305-L306)).
  Start/stop are stored as ISO text.

### 3.5 Notable features

From [README.md#L27-L75](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/README.md#L27-L75), cross-checked against docs:

- **Catch-up** for M3U (`catchup-source`, `shift`, `tvg-rec` fallbacks with `utc`/`lutc`)
  ([m3u-playlist-module.md#L1284-L1310](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/m3u-playlist-module.md#L1284-L1310)), desktop only.
- **Recording**: live recording only through embedded mpv (`recordings` table,
  capability-gated) ([schema.ts#L350-L365](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/libs/shared/database/src/lib/schema.ts#L350-L365),
  [embedded-mpv-native.md#L172](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/embedded-mpv-native.md#L172)). No scheduled DVR found.
- **VOD/series** with download manager, TMDB enrichment (user's own key), resume.
- **Parental lock**: PIN-protected category locks with idle relock
  ([parental-lock.md#L1-L30](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/parental-lock.md#L1-L30)). The only one of the three with this.
- **Remote control**: a mobile web remote served by Electron over HTTP (channel
  up/down, number select, volume)
  ([remote-control.md#L1-L30](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/remote-control.md#L1-L30)).
- Global search, favorites (per-playlist and global), recently viewed, command palette,
  multi-channel EPG grid, 19 languages.
- **Not found**: multiview, casting (Chromecast/AirPlay), scheduled recording.

### 3.6 Architecture

- Electron main process (IPC handlers, EPG handlers) + one long-lived DB worker thread
  + per-job EPG parser and playlist refresh workers + Angular renderer (NgRx /
  Signal Store) + optional native addon and mpv helper/process
  ([sqlite-db-worker.md#L30-L120](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/sqlite-db-worker.md#L30-L120)).
- Same Angular app compiled for PWA, where a `web-backend` does what the Electron main
  process does ([pwa-self-hosted.md](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/pwa-self-hosted.md)).
- Nx apps: `electron-backend`, `web`, `web-backend`, `remote-control-web`, plus
  Xtream and Stalker mock servers for tests; libs by domain (`epg`, `playback`,
  `playlist`, `portal`, `shared`, `ui`, `workspace`).

### 3.7 Known pain points (from issues)

- **Browser codec/format limits** are the dominant theme: "Every video player fails
  to play except VLC" ([#732](https://github.com/4gray/iptvnator/issues/732)), "No video"
  ([#59](https://github.com/4gray/iptvnator/issues/59)), "Cannot play any channel"
  ([#39](https://github.com/4gray/iptvnator/issues/39)), `output=ts` links
  ([#63](https://github.com/4gray/iptvnator/issues/63)), live M3U8 not working in
  Video.js/ArtPlayer ([#622](https://github.com/4gray/iptvnator/issues/622), open),
  "media could not be loaded" ([#504](https://github.com/4gray/iptvnator/issues/504)),
  AC3 audio request ([#949](https://github.com/4gray/iptvnator/issues/949)), 4K
  ([#541](https://github.com/4gray/iptvnator/issues/541)), ClearKey
  ([#1243](https://github.com/4gray/iptvnator/issues/1243)).
- **PWA/self-hosted**: CORS and mixed content block streams; iOS PWA issues
  ([#220](https://github.com/4gray/iptvnator/issues/220), 21 comments, top issue;
  [#525](https://github.com/4gray/iptvnator/issues/525) open; [#269](https://github.com/4gray/iptvnator/issues/269)).
  Maintainer confirms the browser fetches streams directly, so CORS-less providers
  can't play (comment on #525).
- **External/embedded mpv friction**: mpv path/install problems
  ([#248](https://github.com/4gray/iptvnator/issues/248), [#420](https://github.com/4gray/iptvnator/issues/420)),
  VLC on Windows ([#315](https://github.com/4gray/iptvnator/issues/315), [#369](https://github.com/4gray/iptvnator/issues/369)),
  Linux embedded mpv mispositioned and no fullscreen
  ([#1145](https://github.com/4gray/iptvnator/issues/1145)).
- **EPG**: tvg-id matching failures with iptv-org EPGs
  ([#114](https://github.com/4gray/iptvnator/issues/114)), app freezes importing a
  large EPG ([#690](https://github.com/4gray/iptvnator/issues/690)), timezone
  ([#786](https://github.com/4gray/iptvnator/issues/786)), EPG offset
  ([#50](https://github.com/4gray/iptvnator/issues/50)), multiple EPG sources
  ([#141](https://github.com/4gray/iptvnator/issues/141)).
- **Stalker**: repeated "not working" reports
  ([#343](https://github.com/4gray/iptvnator/issues/343), [#686](https://github.com/4gray/iptvnator/issues/686), [#755](https://github.com/4gray/iptvnator/issues/755)).
- **Provider bans** from request volume, possibly per-channel Xtream EPG fetches
  ([#682](https://github.com/4gray/iptvnator/issues/682)).
- **Memory**: RAM load ([#195](https://github.com/4gray/iptvnator/issues/195)).
- **UX churn**: complaints after a player UI redesign
  ([#1667](https://github.com/4gray/iptvnator/issues/1667)).

---

## 4. Comparison table

| | ynoTV | Fred TV (open-tv) | IPTVnator |
| --- | --- | --- | --- |
| Shell / backend | Tauri 2, Rust | Tauri 2, Rust | Electron 43, Node |
| UI | React 19 + zustand | Angular 17 | Angular 22 + NgRx |
| Published platforms | Windows only | Win, macOS, Linux (+ separate mobile repo) | Win, macOS, Linux (many arches), PWA/Docker |
| M3U parsing | TS in renderer, whole string, line loop | Rust, `BufReader` line stream into one SQLite txn | JS parser fork, whole string, in worker |
| Xtream | Yes | Yes | Yes |
| Stalker | Yes | No (empty stub) | Yes |
| XMLTV | Yes, Rust `quick-xml` streaming, gzip | No (Xtream per-channel EPG only) | Yes, `saxes` streaming in worker, gzip |
| EPG matching | tvg-id + normalized fuzzy name + editor | n/a | manual map, then tvg-id, tvg-name, name |
| Playback | Embedded mpv (sidecar `--wid` or libmpv) | External mpv process per stream | hls.js / mpegts.js / Shaka / Video.js / ArtPlayer; external mpv/VLC/IINA; experimental embedded mpv |
| Storage | SQLite (WAL), normalized | SQLite, normalized, paged queries | SQLite; M3U as JSON blob, Xtream normalized |
| Full list in memory? | Virtualized UI over SQL | No, 36-row pages | Yes for M3U (NgRx array) + virtual scroll |
| Search | SQL `LIKE` | SQL `LIKE` (keyword mode) | FTS5 (EPG, titles) + in-memory filter |
| Catch-up | M3U, Xtream, Stalker | Xtream only | M3U (desktop), portals |
| Timeshift | mpv demuxer cache | No | Via catch-up only |
| Recording | Scheduled DVR (FFmpeg) | mpv `--stream-record` | Embedded mpv only |
| Multiview | Up to 4 (2x2, PiP, etc.) | Separate windows | No |
| Casting | Chromecast | No (LAN HLS re-stream instead) | No |
| Remote | Phone remote (WS), gamepad | TV remote keys | Mobile web remote |
| Parental lock | Not found | Not found | PIN category lock |
| License | AGPL-3.0 | GPL-2.0 | MIT |
| Top issue themes | Embedded video geometry, DB bloat, freezes | External mpv setup, stream drops, no EPG | Browser codec limits, CORS/PWA, EPG matching |

---

## 5. Gaps a new player could fill

Each point is marked **[Evidence]** when it restates findings above, or
**[Inference]** when it is a judgement drawn from them.

1. **Native Linux (Wayland) with embedded playback.** [Evidence] ynoTV publishes no
   Linux build; IPTVnator's embedded mpv has no native Wayland path and forces X11 via
   Xwayland ([embedded-mpv-native.md](https://github.com/4gray/iptvnator/blob/bcc6186c8830a6d76c8a563b49033935d62b2984/docs/architecture/embedded-mpv-native.md));
   Fred TV only launches external mpv windows. [Inference] A player that embeds mpv
   (libmpv render API into a GL/Vulkan surface the toolkit owns) and works on Wayland
   has little competition among these three.
2. **Avoid the "separate window over a transparent webview" pattern.** [Evidence] It
   drives ynoTV's most-discussed bugs (#97, #156, #145) and IPTVnator's Linux
   embedded bugs (#1145); IPTVnator hit black video with `wid` on macOS.
   [Inference] Rendering mpv into a surface owned by the UI toolkit (libmpv render API,
   as ynoTV macOS and IPTVnator macOS do) removes a whole bug class.
3. **mpv/FFmpeg-class codec coverage by default.** [Evidence] IPTVnator's built-in
   web players cannot play many IPTV streams (AC3, HEVC, raw TS quirks; #732, #59,
   #622, #949), and Fred TV depends on the user installing mpv/ffmpeg/yt-dlp (#47, #61).
   [Inference] Bundling libmpv so it "just plays" is a key differentiator.
4. **Bounded, streaming ingestion for huge playlists and EPGs.** [Evidence] Fred TV
   streams M3U line by line into one transaction; ynoTV and IPTVnator parse whole M3U
   strings; IPTVnator keeps full M3U lists in memory as a JSON blob; ynoTV reports
   multi-GB databases from repeated EPG caching. Both streaming XMLTV parsers are good
   references. [Inference] Combine: streaming M3U + streaming XMLTV, normalized
   tables, time-window pruning of EPG, integer timestamps, and paged/virtualized
   queries so memory stays flat at 100k channels.
5. **Proper full-text search.** [Evidence] ynoTV and Fred TV use `LIKE '%q%'`;
   IPTVnator uses FTS5 only for EPG and titles. [Inference] FTS5 (trigram tokenizer
   for substring matches) over channels, groups, VOD and EPG is cheap to add from the
   start.
6. **EPG matching as a first-class feature.** [Evidence] EPG mapping is a recurring
   issue in all three (ynoTV #36, #171, #240; IPTVnator #114; Fred TV has no XMLTV).
   ynoTV's normalized-name matcher and manual editor are the most complete reference.
   [Inference] Persisted manual overrides that survive source refresh (ynoTV #240
   shows this breaking), multiple EPG sources with priority, and per-source time
   offset.
7. **Provider-friendly networking.** [Evidence] All three have IP-ban or
   "too many requests" reports (ynoTV #110, Fred TV #275, IPTVnator #682).
   [Inference] Rate limiting, caching of per-channel EPG calls, respecting
   max-connections (Fred TV's kill-oldest approach is a good start) and a real
   player User-Agent reduce bans.
8. **Features only one player has.** [Evidence] Parental lock (IPTVnator only),
   scheduled DVR, multiview and casting (ynoTV only, Windows only), LAN re-stream
   (Fred TV only). [Inference] A cross-platform player with even a subset (DVR +
   multiview + parental lock) would be unique among these three.
9. **Keep scope and startup lean.** [Evidence] ynoTV's breadth brings regressions
   tied to rarely-used features (loudness filter slowing zapping #227, a theme pegging
   the GPU #96, 10 GB RAM #184). Fred TV's paged, minimal design is praised for speed
   (#355 opening comment). [Inference] A small core with optional integrations, plus
   performance tests on large generated fixtures (as IPTVnator does for 10k/50k/100k
   M3Us), guards against this.
10. **Unknowns worth checking before relying on them.** Not verified here:
    actual memory/time numbers for any player at 50k+ channels (no committed
    results), Fred TV's mobile app internals (separate repo), and whether ynoTV's
    macOS code path works (no published build).

## 6. External EPG sources (researched 2026-10-12)

For the EPG sources milestone (`docs/milestones/epg-sources.md`).

- **IPTVnator** (pinned at
  [6fe684a](https://github.com/4gray/iptvnator/tree/6fe684af5190a1e75032cd655d862c43d9c36396)):
  a global list of XMLTV URLs, plus per-playlist lists for M3U only
  (since v0.22, [#707](https://github.com/4gray/iptvnator/issues/707)).
  Local files and gzip (magic bytes) work. For Xtream it never reads
  `xmltv.php`: it calls `get_short_epg` per stream, falling back to the
  global XMLTV, or the reverse with one global "prefer uploaded EPG"
  switch. Sources are merged unranked, duplicates collapsed on
  (channel, start, title). Matching: manual mapping, exact id, exact then
  case-insensitive display name; no fuzzy match. A "Map EPG channel"
  picker came in v0.23 ([#395](https://github.com/4gray/iptvnator/issues/395)).
  One global display-only offset; per-channel offset is a request
  ([#1676](https://github.com/4gray/iptvnator/issues/1676)). Pain points:
  no guide when ids differ ([#114](https://github.com/4gray/iptvnator/issues/114)),
  a 250 MB file reaching 6 GB of RAM ([#696](https://github.com/4gray/iptvnator/issues/696)),
  imports killed at about 500k programmes ([#784](https://github.com/4gray/iptvnator/issues/784)),
  duplicates ([#203](https://github.com/4gray/iptvnator/issues/203)), a
  removed source's programmes left behind ([#1513](https://github.com/4gray/iptvnator/issues/1513)),
  hours-off guides ([#50](https://github.com/4gray/iptvnator/issues/50),
  [#1508](https://github.com/4gray/iptvnator/issues/1508)), manual refresh
  not bypassing the 12 h freshness check ([#1799](https://github.com/4gray/iptvnator/issues/1799)).
- **ynoTV** (pinned at
  [6ebb41c](https://github.com/tbeezy/ynotv/tree/6ebb41cb755fc1ecfd47fe4ef54369469429e8c8)):
  a waterfall, the provider's guide first, then extra URLs in order, each
  filling only channels still without programmes
  ([sync.ts](https://github.com/tbeezy/ynotv/blob/6ebb41cb755fc1ecfd47fe4ef54369469429e8c8/packages/ui/src/db/sync.ts)).
  Per channel: a tvg-id override, a timeshift and a pin to one source
  ([epg-overrides.ts](https://github.com/tbeezy/ynotv/blob/6ebb41cb755fc1ecfd47fe4ef54369469429e8c8/packages/ui/src/services/epg-overrides.ts)).
  Automatch cleans names and scores them, accepting a cleaned name only
  when exactly one EPG channel has it, with undo
  ([epgChannelMatch.ts](https://github.com/tbeezy/ynotv/blob/6ebb41cb755fc1ecfd47fe4ef54369469429e8c8/packages/ui/src/utils/epgChannelMatch.ts)).
- **open-tv** (pinned at 1bfa78a): no XMLTV, per-channel
  `get_simple_data_table` on demand; external EPG is an open request
  ([#220](https://github.com/Fredolx/open-tv/issues/220)).
- **Taken for Vidicon:** an ordered list per provider with pins (ynoTV),
  per-source rather than global preference, the "exactly one" name rule
  without fuzzy scoring, shifts applied at import rather than on display,
  conditional fetches with per-source limits, and remove deleting a
  source's rows in one transaction.
