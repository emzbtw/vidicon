# Milestone: M3U playlists

Planned 2026-10-22, after v0.1.1. Tickets in `.scratch/m3u/`. Amends
ADR 0001. Released as v0.2.0.

## Why now

Xtream Codes is the only source (ADR 0001), so a user whose channels
come as an M3U link or file cannot use Vidicon at all. That covers public
lists, providers that hand out only a playlist URL, and hand-edited
exports. ADR 0001 planned for M3U as "another source type, behind the
same provider abstraction". How the reference players parse M3U, and
where they broke: `research/reference-players.md` section 7.

## Requirements

- **An M3U provider sits beside Xtream providers** in the switcher,
  scoped by `provider_id` like every other row. Every entry is a live
  channel: no movie or series kinds, however the URL or group looks.
- **Config, in `credentials.toml`:** `type = "m3u"` on a `[[provider]]`,
  Xtream when absent. `url` takes an `http(s)` URL or a local path.
  `username` and `password` are required for Xtream only. Playlist URLs
  often carry a token, so they are under the permission check (ADR 0004)
  and the redactor, as EPG source URLs are.
  ```toml
  [[provider]]
  id = "lists"
  name = "Public lists"
  type = "m3u"
  url = "https://lists.example.net/tv.m3u"  # or a path
  user_agent = "Example/1.0"                # optional
  ```
- **What an M3U provider has:** live list, categories, search,
  favourites, recently played, the guide (its playlist's feeds and
  external EPG sources), recording, reconnect and alternates, the
  category allowlist, channel rules and kept tags, all unchanged. What it
  lacks is hidden, not shown disabled: the account dashboard and expiry
  colours, catch-up, movies and series, and the provider's `xmltv.php`.
- **Parsing:** streamed line by line into SQLite (ADR 0003), never the
  whole list in memory. An entry runs from `#EXTINF` to the next
  non-empty line that is not a `#` line; that line is the URL, with no
  scheme or length check. `#EXTVLCOPT` lines in between and `#KODIPROP`
  lines above the `#EXTINF` attach to the entry; other `#` lines and
  blank lines are skipped. A URL with no `#EXTINF` is kept, named after
  its URL path's last segment. Tolerated: a UTF-8 BOM, CRLF, attribute
  values double-quoted, single-quoted or unquoted, commas inside quoted
  values (the title is everything after the last comma outside quotes).
- **Fields:** the title as `name`; `tvg-id` as `epg_channel_id`;
  `tvg-logo`, `tvg-icon` or `logo` as `icon_url`; `group-title` or
  `group` as the category, its first value when `;`-separated, else
  "Uncategorised"; `tvg-chno` or `tvg-ch` as `num`, else file order.
  `tvg-name` is kept for guide matching and identity.
- **Identity:** `stream_id` is a stable 63-bit hash of `tvg-id`, else
  `tvg-name`, else the title, plus an occurrence index for duplicates
  (several streams with one `tvg-id`). The favourites name fallback
  covers channels whose key still changes.
- **The stream URL** is stored in a nullable `channel.url`, set for M3U
  channels only.
- **Stream headers:** `#EXTVLCOPT:http-user-agent=`, `http-referrer=`
  and `http-origin=`, and a Kodi-style `url|User-Agent=...&Referer=...`
  suffix (cut off the URL), are passed to mpv (`user-agent`, `referrer`,
  `http-header-fields`) and to the recorder's ffmpeg. The provider's
  `user_agent` is the default for streams that set none, and fetches the
  playlist. Unset, it is `vidicon/<version>` as now. Other `#EXTVLCOPT`
  and `#KODIPROP` lines are ignored.
- **Guide:** the `url-tvg` or `x-tvg-url` feeds in the `#EXTM3U` header
  (comma-separated, any number) take the `provider_guide` slot that
  `xmltv.php` takes for Xtream, so the EPG sources order, pins, mappings
  and shifts all apply. Nothing is written into `credentials.toml`.
  Matching adds `tvg-name` as a key after `tvg-id`. Guide feeds, these
  and external EPG sources alike, may be xz-compressed as well as gzip
  or plain; `</tv>` is still required.
- **Refresh:** a URL is one streamed GET on the existing schedule (12 h,
  or 6 h), the `r` key and the rate limits of SPEC 6.2. There is no
  account, so no connection-count check. A local path is re-read when its
  mtime changes.
- **Rejected playlists** keep the old channels and show the reason in
  the refresh panel: no `#EXTM3U` at the start, a download shorter than
  its `Content-Length`, 0 entries, or an HLS stream playlist (`#EXT-X-`
  lines: "this is a stream, not a channel list"). Entries without a URL
  are skipped and counted (`12 entries skipped`); they never reject the
  playlist.
- **Providers form:** a type choice, Xtream or M3U playlist. M3U shows
  one URL-or-path field and an optional user agent, and saving fetches
  the playlist once to check it. A URL shaped like an Xtream panel's
  playlist (`get.php?username=...&password=...`) offers to add it as
  Xtream instead, the fields filled, which also gives the guide,
  catch-up and VOD.
- **Code:** a `Source` enum (Xtream or M3U) decides import and stream
  URLs; the UI asks the provider what it supports (`archive`, `vod`,
  `account`). The existing direct uses of the Xtream client stay. No
  trait for two sources.
- ADR 0001 amended. SPEC 2, 3, 4.1, 5, 6, 7.1, 7.2, 8.2, 9 updated as
  the tickets land; `CONTEXT.md` gains "M3U provider". The README and
  the site drop M3U from what Vidicon does not do. The demo provider
  (`tools/demo/`) serves an invented playlist.

## Proposed (settle in the tickets)

- Duplicate ids: the occurrence index breaks when entries are reordered;
  a URL hash (ynoTV) breaks when a token rotates. Ticket 01 fetches a
  list twice and counts which changes less. Carried into 01.
- `tvg-name` in the identity: only if it is stable between fetches.
  Carried into 01.
- xz decoder: a pure-Rust crate, so packages gain no library. Carried
  into 05.

## Out of scope

- **Catch-up from `catchup`, `catchup-source` and `tvg-shift`
  attributes** (ynoTV). Ticket 01 counts them; a later milestone if they
  are common.
- **The `radio` flag** (IPTVnator): mpv plays audio-only streams as
  they are.
- **DASH with ClearKey from `#KODIPROP`**: licence handling beyond what
  this player is for.
- **Pasted text as a source**, and a **playlist editor or M3U export**
  (ynoTV): a file covers both.
- **Scanning entry URLs for Xtream ids** (ynoTV): the form's `get.php`
  check covers the common case.
