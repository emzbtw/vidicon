# Vidicon: v1 specification

Vidicon is a fast, keyboard-driven IPTV player. This file is authoritative for v1. Decisions and
their reasoning live in `docs/adr/`; terms are defined in `CONTEXT.md`. If this
spec and an ADR disagree, raise it.

## 1. Purpose

A fast, good-looking, keyboard-driven IPTV player for Linux, for one user
first. It exists because the reference players (`research/reference-players.md`)
look poor and slow down with large provider lists.

### Success criteria

- Browsing a 50k-channel provider feels instant: first frame under 300 ms from
  cached data, search filters in under 16 ms per keystroke, list scrolling holds
  the monitor's refresh rate (60 Hz and 144 Hz).
- The app is pleasant enough to use daily instead of the reference players.

### Non-goals (v1)

- VOD, series, catch-up, recording, multiview, casting.
- M3U playlists and Stalker portals (ADR 0001).
- Rendering video in-process (ADR 0002).
- Windows, macOS, mobile, TV.
- Packaging for other users, translations, onboarding.

## 2. Scope

### v1

- One or more Xtream providers stored; one active at a time, with a switcher.
- Live channels grouped by the provider's categories, plus "All" and
  "Favourites".
- Now/next for every channel from the provider's XMLTV.
- Fuzzy search over channel names.
- Favourites: toggle, flat reorderable list.
- Playback in an external mpv, fast channel switching, with pause, stop,
  mute and fullscreen controls in the app.
- Recently played channels and a last-channel key.
- Channel logos (moved into v1, 2026-09-29): `docs/milestones/channel-logos.md`.
- Background refresh; manual refresh.
- Added 2026-09-30 (tickets 18 to 22): a Recently played library view, an
  A–Z sort toggle, a category allowlist, an account dashboard, and expiry
  warning colours.

### Later milestones (not v1)

- EPG guide grid: `docs/milestones/guide-grid.md` (done 2026-10-01).
- Recording, after the guide grid: `docs/milestones/recording.md` (done
  2026-10-01).
- Named favourite groups (schema supports them from v1). Parked
  2026-10-02: Recently played and search cover daily use, and no
  favourites are kept. Revisit once the favourites list is long enough
  to want splitting. Sketch: `docs/milestones/favourite-groups.md`.
- Catch-up, after recording: `docs/milestones/catch-up.md` (done
  2026-10-02).
- Movies and series (VOD), after catch-up: `docs/milestones/vod.md`
  (done 2026-10-04).
- Stream recovery: reconnect a dropped channel, alternates:
  `docs/milestones/stream-recovery.md` (planned 2026-10-06). Alternates
  parked 2026-10-12, then picked up again 2026-10-18 once the looser name
  form found far more of them (9.3): ticket 03 lists them in the detail
  panel (8.2); offering one on a drop (ticket 04) stays parked.
- Planned 2026-10-12, in this order: a refresh panel
  (`docs/milestones/refresh-panel.md`); a settings view writing
  `config.toml` (`docs/milestones/settings.md`); onboarding, a providers
  form and setup screen over `credentials.toml`
  (`docs/milestones/onboarding.md`); external EPG sources
  (`docs/milestones/epg-sources.md`).
- M3U playlists as a second source type, by URL or path, live channels
  only: `docs/milestones/m3u.md` (done 2026-10-22).

## 3. Architecture

Single process, three concerns on separate threads:

```
 UI thread (Slint event loop)
   │  reads pages of rows          ▲ state updates via slint::invoke_from_event_loop
   ▼                               │
 Store (SQLite, WAL) ◄── writes ── Worker runtime (tokio)
                                     ├─ Xtream client (rate-limited HTTP)
                                     ├─ Importer (streaming JSON, M3U + XMLTV)
                                     ├─ Refresh scheduler
                                     └─ Player controller (mpv JSON IPC)
```

- **UI never blocks on network or import.** It only runs short paged queries
  against SQLite on its own read connection.
- **One writer connection**, owned by the worker runtime. WAL mode lets the UI
  keep reading the previous snapshot while an import commits.

### Modules

| Module | Responsibility |
|---|---|
| `config` | Paths, `config.toml`, credential file loading and permission check. |
| `xtream` | HTTP client for `player_api.php` and `xmltv.php`, and an M3U provider's playlist; rate limiting; credential redaction. |
| `m3u` | Reads an M3U playlist an entry at a time (6.4, M3U ticket 02). |
| `store` | Schema, migrations, paged queries, search. |
| `import` | Streams provider responses into the store in one transaction per import. |
| `refresh` | Decides when to refresh; runs imports; reports status. |
| `player` | Spawns and drives the single mpv process. |
| `recorder` | Starts and stops the `ffmpeg`s that make scheduled recordings (9.5), and queues downloads (9.6). |
| `download` | Downloads a movie or episode over HTTP, resuming a part (9.6). |
| `ui` | Slint markup and the Rust glue: models, key handling, selection. |

### Paths

| What | Path |
|---|---|
| Settings | `$XDG_CONFIG_HOME/vidicon/config.toml` |
| Credentials | `$XDG_CONFIG_HOME/vidicon/credentials.toml` (override: `--credentials <path>` or `VIDICON_CREDENTIALS`) |
| Noctalia palette | `$XDG_CONFIG_HOME/vidicon/noctalia-palette.toml`, read with `theme = "noctalia"` (8.1) |
| User themes | `$XDG_CONFIG_HOME/vidicon/themes/<name>.toml`, read with `theme = "<name>"` (8.1) |
| Database | `$XDG_DATA_HOME/vidicon/vidicon.db`, set `0600` on open: an M3U channel's URL can carry the account |
| mpv socket | `$XDG_RUNTIME_DIR/vidicon/mpv.sock` |
| Logs | stderr only in v1 |

## 4. Configuration

### 4.1 Credentials (ADR 0004)

```toml
[[provider]]
id = "main"                     # stable local id, used in the database
name = "My provider"
url = "http://example.com:8080" # origin only
username = "..."
password = "..."

[[provider.epg]]                # guide sources, in order (EPG sources ticket 02)
url = "https://epg.example.net/guide.xml.gz"  # or path = "/srv/epg/guide.xml"
name = "World feed"             # optional, else the host or file name
shift_minutes = 0               # optional, added to every programme's times
enabled = true                  # optional
every_hours = 12                # optional, a url's, at least 1 (EPG sources ticket 03)
max_per_day = 20                # optional, a url's, at least 3; default none

[[provider.epg]]
provider_guide = true           # where the provider's own xmltv.php goes; first if absent

[[provider]]                    # an M3U playlist (M3U ticket 02)
id = "lists"
name = "Public lists"
type = "m3u"                    # "xtream" when absent
url = "https://lists.example.net/tv.m3u"  # or an absolute path
user_agent = "Example/1.0"      # optional
```

- The file is checked at start in this order (onboarding ticket 03):
  it is there, its modes are strict, it parses, it names a provider. The
  first that fails shows the setup screen (8.2) in place of the main
  screen; a probe flag (`--probe-*`) prints the same on stderr instead,
  a missing file with the expected path and format. A file with no
  `[[provider]]` tables, empty or only comments, loads and names none.
- A file mode looser than `0600`, or a directory looser than `0700`, is
  refused before anything is read. The refusal names the file and the
  `chmod` to run. `Fix permissions` on the setup screen sets `0600` on
  the file and `0700` on the directory, whichever is open to others,
  then reads it. It is offered only when the user owns both and the file
  does not resolve into the Nix store.
- A parse error names the line and never quotes a value. A duplicate
  `id` is refused the same way.
- **Provider types** (M3U ticket 02): `type` is `xtream`, the default,
  or `m3u`. An Xtream provider needs `username` and `password` and
  takes no `user_agent`. An M3U provider's `url` is an `http(s)` URL or
  an absolute path to its playlist, and `user_agent`, when given, is
  sent for the playlist instead of `vidicon/<version>`. A playlist URL
  often holds a token, so it is as secret as a password: logs and errors
  name the playlist by its host or file name, and the database never
  holds it. Each refused key names the provider, never its value.
- **Guide sources** (EPG sources ticket 02): each `[[provider.epg]]`
  after its `[[provider]]` is an external XMLTV feed, plain, gzipped or
  xz (M3U ticket 05),
  by an `http(s)` `url` or an absolute `path`, or, with `provider_guide =
  true`, the place of the provider's own `xmltv.php` in the order. The
  order is the file's, the provider's guide first when no entry places
  it. A channel's guide comes whole from the first enabled source with
  programmes for it (5). `shift_minutes` (the provider's guide's
  included) is added to every programme's times at import. `enabled =
  false` skips a source and keeps its data, so turning it back on needs no
  fetch; on the provider's guide it also stops `xmltv.php` and the guide
  tables (6.1) and deletes the provider's programmes. An entry with
  neither or both of `url` and `path`, a relative path, another scheme,
  `provider_guide` twice or a source twice is refused, naming the entry
  by its number, and so is `every_hours` below 1, `max_per_day` below 3
  (2 are kept for `r`, 7.2) or either on a path or the provider's guide. A source's identity is its URL without its query string
  and fragment, or its path, so a renewed token keeps its data; a source
  gone from the file has its data deleted, its pins with it, while its
  manual mappings stay by its identity (5, EPG sources ticket 05). URLs often carry a token, so
  they are as secret as the password: logs, errors and the detail panel
  name a source by its `name`, else the URL's host (`epg.example.net`) or
  the path's file name, and never quote the URL. The UI (Settings › EPG,
  the refresh panel, the detail panel) calls an unnamed source on the
  provider's own host by the provider's name, and a name met again in
  the order gets ` 2`, ` 3` and on, the provider's guide keeping the plain
  name (`Demo TV`, `Demo TV 2`). The
  provider form (8.2) keeps a provider's sources as they are. The file
  is read at start, so a source edited there is used from the next
  start; each refresh (7.2) stores the order its provider's client has.
- **An M3U provider's guide** (M3U ticket 05, 7.1): it has no
  `xmltv.php`, so its `provider_guide` place holds the guide feeds its
  playlist's `#EXTM3U` line names, nothing written to the file. Its
  `enabled` and `shift_minutes` apply to each feed; each is otherwise
  an unnamed URL source due when older than `refresh_hours` (7.2), and
  `[[provider.epg]]` entries work as for Xtream. A feed is called by its
  URL's file name (`uk.xml.xz`), as a playlist's feeds often share one
  host, else by its host; a name met again gets ` 2` as above. A feed an
  entry names already (by identity, above) is that entry's, fetched once:
  so a feed is named, or given its own shift and limits, by adding its
  URL as an entry, in the file or with Settings › EPG's "Add source…".
  It then sits where the entry does, and stays if the playlist drops it.
- Settings › EPG (EPG sources ticket 04, 8.2) adds, edits, reorders,
  turns off and removes the shown provider's sources through the
  settings writer (4.2), under the same checks and modes as the
  provider form. A new entry goes last in the provider's
  `[[provider.epg]]` tables; an edit changes its values in place, a value
  back to its default dropped unless the file gave it, and an added key
  goes last in its table; a move swaps whole entries with their comments;
  a removal moves the comments above it to the next entry. The provider's
  guide gets a `provider_guide = true` entry only when it moves, another
  moves above it, or it is edited. The provider's client takes the saved
  sources at once, and its refresher runs a pass at once (7.2), which
  stores the order, skips a source turned off and deletes a removed one's
  data in one transaction, and imports a new one with one fetch.
- Settings › Providers adds a provider (onboarding ticket 01, 8.2). Its
  `id` is made from the name once and never changed: lowercase ASCII
  letters and digits, accents dropped (`Télé Nord` → `tele-nord`), any
  other run of characters one `-`, `provider` if that leaves nothing,
  `-2` and up when taken. The URL is cut to its origin on entry:
  `http://` when no scheme is typed, and a path, query or login dropped,
  which the field's note says (a pasted playlist link holds the
  password). Before saving, the account check (6.3) runs on the new
  provider's own rate-limited client (6.2), which it keeps once saved:
  `auth` 0 refuses (`The provider refused this username and password`),
  as does HTTP 513, which some providers send for a wrong password;
  an expired or disabled account, no answer (or a 429 or other 5xx), a
  401 or 403 (`The server refused access`, often a stale address), or a
  reply that is not an Xtream account warns, naming which, with `Save
  anyway`. Saving appends a `[[provider]]` table at the end of the file
  through the settings writer (4.2), keeping the rest as it was, refused
  unless the result loads; the file is written `0600` (the temporary
  file has that mode from its creation) and its directory created or
  set to `0700`. A managed file (4.2) makes the section read-only with
  its note. The provider starts at once: its refresh scheduler runs, the
  recorder, guide tables, info and logos use its client, and it is in
  the switcher.
- **An M3U provider in the form** (M3U ticket 06): the form's type,
  Xtream or M3U, is chosen when adding and fixed once saved.
  An M3U one's playlist is an `http(s)` URL or an absolute path to a
  file that can be read, checked on entry without fetching and kept as
  typed; its user agent is optional. Before saving, the playlist is
  fetched once on the new provider's client, or read from its path, and
  read through without storing anything: one the import refuses (7.1:
  no `#EXTM3U`, an HLS stream, no channels) is refused (`The playlist
  was refused: <why>`), one not fetched (no answer, an HTTP error, a
  cut download, a file that cannot be opened) warns with `Save anyway`.
  Its first refresh then fetches it again. Saved, its table has `type =
  "m3u"`, `url` and any `user_agent`, never a `username` or `password`;
  an edit sets them in place, a cleared user agent removing the key, and
  fetches only when the URL or user agent changed. A playlist URL that
  is an Xtream panel's (`get.php` with a non-empty `username` and
  `password` in its query) shows `Add as Xtream instead`, before the
  check and after it, as a panel can refuse `get.php` while its API
  works (6.4). It switches the form to Xtream with the URL's origin,
  username and password, the link and the user agent dropped, and
  `Check and save` then checks the account as above.
- Settings › Providers edits and removes a provider (onboarding ticket
  02, 8.2). Editing changes its name, URL, username and password; the id
  never changes, and a new URL or username keeps all its data, the next
  refresh reconciling as now. A change to only the name saves without a
  check; a new URL, username or password runs the account check first,
  on a client of its own, as adding does. Saving changes the values in
  the provider's table, each keeping its spacing and trailing comment,
  under the same writer and checks as adding. The provider's client then
  uses them from its next request, under the same limits and backoff
  (6.2), and the next import stores the new name. Removing deletes the
  provider's table, the comments above it moving to the table after it,
  or the file's end, then its data (5). Removing the last leaves a file
  with no provider and shows the setup screen (onboarding ticket 03). A pinned
  `active_provider` (4.2) naming it is cleared, else the next start
  would refuse it. The removed provider's refresh scheduler stops and
  anything of it playing stops. If it was shown, the next provider is
  shown (the one before when it was last).
- Credentials never appear in logs, error messages, the UI, or mpv's title.
  Every URL passed to the logger goes through a redactor that strips the
  username and password from both query strings and `/live/<user>/<pass>/`
  paths.

### 4.2 Settings

```toml
active_provider = "main"
refresh_hours = 12          # 12 or 6
mpv_path = "mpv"            # optional
extra_mpv_args = []         # appended to the app's own args
recordings_dir = "/srv/tv"   # where R records, absolute (no ~); default
                            # $XDG_VIDEOS_DIR/Vidicon/Recordings, else
                            # ~/Videos/Vidicon/Recordings
downloads_dir = "/srv/films" # where d downloads (VOD ticket 10); default
                            # $XDG_VIDEOS_DIR/Vidicon/Downloads, else
                            # ~/Videos/Vidicon/Downloads
ffmpeg_path = "ffmpeg"      # records scheduled programmes (9.5)
record_before = 2           # minutes a scheduled recording starts early
record_after = 5            # and runs late
theme = "default"           # "noctalia", a built-in or a user theme (8.1)
```

Missing file means defaults. Unknown keys are an error (catches typos).
Every key is in a section of the Settings view (8.2), and a new key
names the section it goes in.

The Settings view (settings ticket 01) edits this file in place with
`toml_edit`, keeping comments, order and layout, and writes only the keys
above. A save re-reads the file, changes the one key (keeping the old
value's spacing and trailing comment; a removed key's comments above it
stay, on the next key), and refuses the result unless it still loads.
It writes a temporary file beside the original and renames it over it
with the original's mode, so a crash never leaves half a file; a symlink
stays one, the file it points at replaced. A file that is a symlink into
the Nix store, or read-only to the user (or in a directory that is), is
managed: its values show, a note says why (`config.toml is managed by
Nix; edit it there`, `config.toml is read-only to you, so nothing here
saves`), and nothing saves. The app reads the file at start and after
its own saves, never watching it; the one file it watches is the
chosen theme's file, Noctalia's palette or a user theme (8.1). A saved
value applies without a restart:
`refresh_hours` re-arms every provider's check, run at once on the new
age; `theme` re-colours the window; `active_provider` is used from the
next start.

Typed values are checked before they are written (settings ticket 02),
a refusal naming why: `recordings_dir` and `downloads_dir` must be
absolute (`Must be an absolute path`), a folder (`/srv/tv: not a
folder`) the user can write to (`/srv/tv: not writable by you`); one that
does not exist (`/srv/tv: does not exist`) is created on asking.
`mpv_path` and `ffmpeg_path` are a name found on `PATH` (`mpv: not found
on PATH`) or an absolute path (`Must be an absolute path, or a name on
PATH`) to an executable file (`/opt/mpv: not found`, `/opt/mpv: not
executable`). `extra_mpv_args` is typed as one line split as a shell
would (`extra args: unmatched quote`) and written as the TOML array it
gives. `record_before` and `record_after` step from 0 to 60. The next
mpv spawn uses a saved `mpv_path` and `extra_mpv_args`, the next
recording or download the folders, `ffmpeg_path` and padding; the mpv,
recordings and download running keep going, and schedules already made
keep their padding. A value that passed but fails on use (a folder
removed later) is reported where it is used.

Start provider (ticket 10): `active_provider` if set, which pins it; else the
provider last switched to with `p` (kept in the database, never written to
this file); else the first `[[provider]]`. Provider `id`s must be unique.

## 5. Data model

All provider data is scoped by `provider_id` (ADR 0001).

```
provider   (id TEXT PK, name, last_channels_refresh INTEGER, last_epg_refresh INTEGER,
            last_movies_refresh INTEGER, last_series_refresh INTEGER,
            playlist_mtime INTEGER NULL, playlist_guides TEXT NULL)
category   (provider_id, kind, category_id TEXT, name, sort INTEGER, shown INTEGER,
            PK (provider_id, kind, category_id))
channel    (provider_id, stream_id INTEGER, num INTEGER, name, name_norm,
            category_id, epg_channel_id, icon_url, extension, name_loose,
            PK (provider_id, stream_id))
channel_fts  FTS5 over channel.name (search prefilter, see 8.4)
movie      (provider_id, stream_id INTEGER, num INTEGER, name, name_norm, category_id,
            rating REAL, poster_url, extension, added INTEGER,
            year INTEGER, length INTEGER, genre, plot, actors, director, info_at INTEGER,
            PK (provider_id, stream_id))
series     (provider_id, series_id INTEGER, num INTEGER, name, name_norm, category_id,
            year INTEGER, rating REAL, cover_url, genre, plot, actors, director,
            episode_run_time INTEGER, last_modified INTEGER,
            seasons INTEGER, last_year INTEGER, info_at INTEGER,
            PK (provider_id, series_id))
episode    (provider_id, series_id INTEGER, season INTEGER, episode_num INTEGER,
            stream_id INTEGER, title, extension, length INTEGER, sort INTEGER,
            PK (provider_id, series_id, stream_id))
programme  (provider_id, epg_channel_id, start INTEGER, stop INTEGER, title, description,
            archive INTEGER NULL, PK (provider_id, epg_channel_id, start))
favourite  (provider_id, kind, stream_id INTEGER, name_norm, group_id INTEGER NULL,
            position INTEGER, name, PK (provider_id, kind, stream_id))
app_state  (key TEXT PK, value)   -- not provider data, e.g. last_provider
history    (provider_id, kind, stream_id INTEGER, played_at INTEGER,
            PK (provider_id, kind, stream_id))
progress   (provider_id, kind, stream_id INTEGER, series_id INTEGER NULL,
            season INTEGER NULL, episode_num INTEGER NULL, pos INTEGER,
            length INTEGER NULL, watched INTEGER, at INTEGER,
            PK (provider_id, kind, stream_id))
category_rule     (provider_id, kind, pattern, PK (provider_id, kind, pattern))
category_override (provider_id, kind, category_id TEXT, name, shown INTEGER,
                   PK (provider_id, kind, category_id))
channel_rule      (provider_id, pattern, PK (provider_id, pattern))
kept_tag          (provider_id, tag, PK (provider_id, tag))
recording  (id INTEGER PK, provider_id, stream_id INTEGER, channel, guide_id NULL,
            title NULL, start INTEGER, stop INTEGER, padded_start INTEGER,
            padded_stop INTEGER, state, file NULL, note NULL,
            trashed INTEGER)
download   (id INTEGER PK, provider_id, kind, stream_id INTEGER, series_id NULL,
            season NULL, episode_num NULL, title, episode NULL, extension,
            state, file NULL, total INTEGER NULL, note NULL, added INTEGER,
            ended INTEGER NULL, trashed INTEGER)
```

`channel` also has `shown INTEGER` (ticket 25), derived like
`category.shown`; see the channel rules and kept tags below,
`archive_days INTEGER` (catch-up ticket 02), `guide_id` (EPG sources
ticket 02, below), and `url` and `tvg_name` (M3U ticket 02, below).

```
epg_source     (id INTEGER PK, provider_id, ident, position INTEGER, enabled INTEGER,
                shift_minutes INTEGER, imported INTEGER NULL, error NULL,
                etag NULL, last_modified NULL, mtime INTEGER NULL,
                fetched INTEGER NULL, retry_at INTEGER NULL,
                UNIQUE (provider_id, ident))
source_request (source_id INTEGER, at INTEGER)
source_channel (source_id INTEGER, channel_id, names, icon_url)
source_match   (source_id INTEGER, stream_id INTEGER, guide_id NULL,
                PK (source_id, stream_id))
source_pin     (provider_id, stream_id INTEGER, source_id INTEGER,
                shift_minutes INTEGER, name_norm, PK (provider_id, stream_id))
source_mapping (provider_id, ident, stream_id INTEGER, channel_id, name_norm,
                PK (provider_id, ident, stream_id))
```

`epg_source` also has `reimport INTEGER` (EPG sources ticket 05, below).

- **Guide sources** (EPG sources ticket 02, 4.1): `epg_source` is each
  provider's order, `ident` empty for its own `xmltv.php`, the others'
  identity (4.1), with when each last imported and why its last import
  failed; for fetch limits (EPG sources ticket 03, 7.2) the feed's `ETag`
  and `Last-Modified`, a path's modification time in ns, when it was
  last asked and until when it backs off. A new `shift_minutes` clears
  the validators and the time, so the next fetch imports. `source_request`
  holds each request to a source in the last 24 h, older ones dropped as
  one is added. `source_channel` is a source's whole channel list, display
  names one per line, for the mapping picker (ticket 05).
  `source_match` is which feed channel each provider channel matched,
  `guide_id` NULL when more than one did (ambiguous). A source's
  programmes are in `programme` with the provider's, under the guide id
  `\x01<source id>\x01<feed channel id>`: XML cannot hold `\x01`, so it
  never meets a provider's guide id, and one source's programmes are one
  key range. `channel.guide_id` is the guide id the channel's programmes
  are under: its `epg_channel_id` from the provider's guide, or a
  source's, from the first enabled source in the order with programmes
  for it, NULL when none has. Every import and every change to the order
  sets it again, writing only the channels whose guide changed; the
  detail panel, the grid, search, the recorder and catch-up read
  programmes by it. Matching, per source and redone after each of its
  imports: an exact `epg_channel_id`, then the display name as
  `name_norm`, then, only when that finds nothing, the looser form (6.1);
  each step counts only on exactly one feed channel. An M3U channel's
  `tvg-name` is tried before its name at each name step (M3U ticket 05).
- **Pins and manual mappings** (EPG sources ticket 05, 8.2): a pin puts
  one source, the provider's guide included, first for one channel, with
  `shift_minutes` added to the source's own; a pin to a source turned
  off is kept and skipped, and the channel follows the order meanwhile;
  one whose source has no programmes for the channel is skipped too. A
  pin without a shift reads the source's programmes as they are; with
  one, a copy of them moved by it, under the guide id
  `\x01<source id>\x01\x01<minutes>\x01` and the channel's id in the
  source (its `epg_channel_id` for the provider's guide): a feed channel
  id cannot start with `\x01`, so the copies are one range inside the
  source's. The copies are made from what is stored when the pin is set
  and again after each import of the source. A manual mapping names a
  source's channel for one provider channel, by the source's `ident`,
  so a source removed and added again with the same identity gets its
  mappings back; matching takes it first while the feed has that
  channel, else matches as before. A new or forgotten mapping is stored
  in `source_match` at once, so the guide changes then if the source has
  the channel's programmes already, and marks the source `reimport`:
  the next refresh fetches it whole (its validators cleared), which keeps
  the programmes of what it matches now, and clears the mark. Both
  follow the favourites fallback after each channel import: one whose
  channel is gone moves to the one channel with its `name_norm`, if
  exactly one, and is dropped if that channel has one already; otherwise
  it is kept. Removing a source deletes its pins; its mappings stay
  unless the confirm's "Also delete" is on (8.2). Written on the writer
  connection like favourites.

- **M3U channels** (M3U ticket 02, 6.4, 7.1): an M3U provider's
  entries are all live channels. `url` is the entry's stream URL as the
  playlist gives it, without a `|` suffix of headers, NULL on an
  Xtream channel; `tvg_name` its `tvg-name`, for guide matching.
  `user_agent`, `referrer` and `origin` are the stream's headers (M3U
  ticket 03, 9.1), NULL where the playlist sets none.
  `stream_id` is a stable 63-bit hash (FNV-1a, the same in every build)
  of the entry's key, `tvg-id`, else `tvg-name`, else the title: the
  key's alone for the first entry with that key, so a backup added after
  a channel leaves its id alone; for a later one, with its URL's hash
  and its place among the entries sharing both.
  Categories are the groups, by name as their id, in the order first
  met. `extension` is empty and `archive_days` 0.
  `provider.playlist_mtime` is a playlist path's modification time in
  ns when last read, NULL for a URL. `provider.playlist_guides` is the
  guide feeds its header named at the last import, one URL a line, NULL
  for none (M3U ticket 05); unlike the playlist's own URL they are kept,
  as stream URLs are, since a refresh fetches them on their own
  schedule.
- Times are Unix seconds, UTC. Display converts to local time.
- **Kinds** (VOD ticket 02): `kind` is `live`, `movie` or `series`. The
  provider numbers each kind's categories on its own, so the kind is part
  of a category's key, and of its override's. `movie` and `series` hold
  the lists as `get_vod_streams` and `get_series` give them (6.1): a
  movie's `rating` (out of 10), poster (`stream_icon`), `extension`
  (`container_extension`) and `added`; a series' `year` (from
  `release_date`), `rating`, cover, `genre`, `plot`, `actors` (`cast`),
  `director`, `episode_run_time` (minutes) and `last_modified`. Numbers
  arrive as numbers or strings; an empty one is NULL. The movie list has
  no year, length, genre or plot: those come with a movie's info (VOD
  ticket 04), stored on its row when fetched (`info_at`): `year` from
  `releasedate`, `length` in seconds from `duration`, `genre`, `plot`,
  `actors` and `director`. The next import of the list replaces the row,
  and the info with it. A series' info (VOD ticket 05) is its episodes,
  stored per series when fetched: `season` (the `episodes` key),
  `episode_num`, the stream id (`id`), the provider's whole `title`,
  `extension` and `length` in seconds from the episode's `duration`, in
  the provider's order (`sort`); on its row, how many seasons have
  episodes, the latest episode's year (`air_date`), and `info_at`. A
  series import clears every episode with the rows. A title's
  `name_norm` is a channel's (below).
- `name_norm`: NFKC-folded (real providers tag names with superscripts such
  as `ᴴᴰ`, which fold to `HD`), lowercased, whitespace-collapsed, trailing
  quality tags stripped (`SD`, `HD`, `FHD`, `UHD`, `4K`, `8K`, `HEVC`, `H265`,
  `LQ`, resolutions like `1080p`). `+1`, `RAW` and `VIP` stay. Used for the
  favourites fallback and A–Z order (8.2).
- `name_loose`: a channel's name in the looser form (6.1), for its
  alternates (8.2, stream recovery ticket 03); empty for a divider, a
  name that starts and ends with `#` (`##### SPORT #####`), which
  providers list as a channel to head the ones after it. Set by the
  import; migration 28 filled it for the channels stored before it.
- **Favourites and history across kinds** (VOD ticket 11): a favourite
  or a play is a channel, a movie or a series, by `kind` as categories
  are, and the kind is in the key: series ids equal about 1k live and
  13k movie ids (6.1). A series' `stream_id` is its `series_id`; playing
  one of its episodes plays the series. Rows from before are live.
- **Favourites survive provider ID changes:** after an import of a
  kind's list, any favourite of that kind whose `stream_id` no longer
  exists is re-pointed at the one title of the kind with the same
  `name_norm`, if exactly one exists; other kinds' names are never
  matched. Otherwise it is kept but shown as unavailable.
- `group_id` is always NULL in v1.
- `favourite.name` is the title's display name, refreshed by each import
  of its kind, so an unavailable favourite still shows what it was (ticket
  07).
- Favourites are written on the worker's writer connection like imports
  (3), so a toggle during an import lands when the import commits.
- `history`: one row per channel, movie or series played, `played_at`
  updated on each play; pruned to the 50 most recent per provider, of all
  kinds. Written like favourites. A title gone after an import of its
  kind drops out of recently played. Clearing it
  (8.3) empties the active provider's rows only.
- **Category allowlist** (ticket 20), per provider and per kind (VOD
  ticket 02): live, movie and series categories are named differently
  (6.1), so each kind has rules of its own. With no rules every
  category of that kind shows. With rules, a category shows if its name contains any
  rule's pattern, compared case-insensitively and NFKC-folded (`hd` matches
  `ᴴᴰ`). A `category_override` wins over the rules both ways: it hides a
  category a rule shows, or shows one no rule covers. Rules are patterns,
  not a snapshot of matches, so a category the provider adds later follows
  them.
- `category.shown` is derived: recomputed from the rules and overrides by
  each import, of any kind, and each edit, so the library and All read it
  directly. A channel, movie or series whose category the provider does
  not list is never hidden.
- **Overrides survive category ID changes** like favourites: after an
  import, an override of that kind whose `category_id` no longer exists is
  re-pointed at the one category of the kind with the same `name_norm` of
  its name, if exactly one exists; otherwise it is kept in case the category returns.
  `category_override.name` is refreshed by each import. Rules and overrides
  are written on the writer connection like favourites.
- **Channel rules** (ticket 25), per provider, none by default, live only: hide
  channels by name, for providers that file channels by format rather than
  region (an `8K|` category kept for its UK channels that also holds `FR:`
  ones). A rule hides every channel whose name contains it at the start of
  a word: at the start of the name, or after anything but a letter or
  digit, folded like category rules. So `FR:` hides `FR: x`, `x | FR: y`
  and `BE/FR: z` but not `AFR: x`; a plain substring would also catch
  `AFR:`, and a prefix would miss tags placed mid-name. Rules, not a
  snapshot: channels the provider adds later follow them.
- **Kept tags** (ticket 26), per provider, none by default: say which
  regions to keep instead of listing every region to hide. A name's tag is
  its start, after folding and leading whitespace: 2 or 3 letters followed
  by `:` or `|`, spaces allowed before it, several codes joined by `/`
  (`BE/FR:` has two). Digits are never a tag (`4K:`, `24/7:`), and only the
  start of a name counts (`x | FR: y` has none). With kept tags, a channel
  is hidden if its name has a tag and none of its codes is kept; a channel
  without a tag is never hidden by them, as many names carry none. Stored as
  the letters only, so `uk`, `UK:` and `uk|` are one entry. A 3-letter
  brand prefix (`ABC: x`) reads as a tag and is hidden until kept; manage
  mode's counts show it. Tags the provider adds later are hidden too.
- `channel.shown` is derived: recomputed from the channel rules and kept
  tags by each channel import and each edit; a channel is hidden if either
  hides it, so channel rules still catch tags placed mid-name. Hidden channels leave All, their category,
  search and the guide grid; Favourites and Recently played show them
  regardless. A category whose channels are all hidden still shows, with
  none. Rules are written on the writer connection like favourites.
- `archive_days`: how many days back the provider keeps the channel's
  archive (catch-up), 0 for none: `tv_archive_duration` when `tv_archive`
  is 1, else 0. Both arrive as numbers or strings. Set by each channel
  import; the EPG import keeps past programmes by it (7.1). A partial
  covering index holds the channels with one (about 1k of 58k), so the
  guide grid reads how far back a category reaches in 0.5 ms (8.2,
  catch-up ticket 03).
- `programme.archive`: whether the provider's archive has the programme,
  `has_archive` from its channel's `get_simple_data_table` (6.1, catch-up
  ticket 06); NULL where no table said. Where set, it decides whether a
  past programme plays (8.2) over the channel's `archive_days`, so one
  starting just before the window, or missing from the archive within
  it, is told apart. A channel with no archive plays none whatever its
  guide id's flags say.
- **Recordings** (recording ticket 02, 9.5): one row per scheduled
  programme. `channel` is the channel's name when scheduled, for the file
  name and for a channel since gone; `guide_id` (its `channel.guide_id`) and
  `title` match it to its programme. `start`/`stop` are the guide's,
  `padded_*` the times actually recorded. `state` is `scheduled`,
  `recording`, `done`, `failed`, `missed` or `cancelled`; `file` is set
  once it starts; `note` says why one failed, or why a done one was cut
  short. Written by the recorder on the writer connection; an EPG import
  moves schedules with their programmes (7.1). Recordings of what plays
  (9.4) are rows too, with no padding, and `stop` equal to `start` until
  they end. `trashed` marks a finished one whose file `d` moved to the
  trash (8.2); a new recording that takes its file name forgets it.
- **Progress** (VOD ticket 07): where a movie or episode was stopped,
  `kind` `movie` or `episode`, by stream id as history is, so it outlives
  the imports that clear episodes. An episode keeps its series, season
  and number, to find the next one after an import, and by season and
  number if the provider renumbered its streams. `pos` is mpv's
  `time-pos` in seconds, saved every 5 s while it plays and when it
  stops, is replaced or the app quits; `length` is mpv's `duration`.
  **Watched** past 90 % or in the last 5 minutes, whichever comes first,
  the 5 minutes never more than a quarter of the length (so a short one
  is not watched as it starts), or when mpv reaches its end. A watched
  one starts again from 0 and stays watched when watched again. A
  position before 60 s is 0, and such a play keeps the row's `at`, so a
  glance at an earlier episode does not make it the series' latest. A
  series' **next episode** is its latest played (by `at`, ignoring those
  at 0 never watched): that one while part way, else the one after it in
  `episode` order, into the next season at a season's end; after its
  last, the series is finished. `w` (8.3) marks one watched (as if it
  ended: from 0, `at` now) or unwatched, which forgets its row, resume
  point and all; of several marked at once, the latest is the furthest
  on (by season and number). Clearing Continue watching stores when,
  per provider, in `app_state` (`continue_cleared:<provider id>`); the
  list then holds only what was played after it.
- **Downloads** (VOD ticket 10, 9.6): one row per movie or episode
  queued, `kind` `movie` or `episode` by stream id, as progress is; an
  episode keeps its series, season, number and name for its file name.
  `title` is the movie's or series' name. `state` is `queued`,
  `downloading`, `done` or `failed`; a cancelled one is forgotten.
  `file` is the finished file's path, fixed at the first start; until
  done it is written as `<file>.part`, whose size is how far it got.
  `total` is the file's size once the provider has said. The queue runs
  in `added` order, and a failed one queued again goes to its end.
  `trashed` marks a done one whose file `d` moved to the trash, as for
  recordings; a new download that takes its file name forgets it.
- **Removing a provider** (onboarding ticket 02, 4.1): the recorder
  stops its running recordings, kept as cancelled with what they wrote,
  and pauses its download, the part kept; then one transaction deletes
  its `provider` row (the refresh times), categories, channels (and
  their `channel_fts` rows), movies, series, episodes and programmes,
  marks its scheduled recordings `cancelled` and forgets its queued and
  downloading downloads, and clears `last_provider` when it names it.
  Its guide pins go with its sources. Its personal data stays:
  favourites, progress, history, recordings and downloads past
  scheduled or queued, category rules and overrides, channel rules,
  kept tags, manual guide mappings and `continue_cleared:<id>`, so a
  provider added again with the same id gets them back. With "Also delete" they
  go too. Files on disk are never touched, `.part` files included.

## 6. Xtream client

### 6.1 Endpoints used

| Purpose | Request |
|---|---|
| Account check | `GET {url}/player_api.php?username=&password=` (returns `user_info`, `server_info`) |
| Categories | `...&action=get_live_categories` |
| Channels | `...&action=get_live_streams` |
| Full EPG | `GET {url}/xmltv.php?username=&password=` |
| Stream | `{url}/live/{username}/{password}/{stream_id}.{ext}` |
| Channel guide, past included (catch-up) | `...&action=get_simple_data_table&stream_id=` |
| Archive (catch-up) | `{url}/timeshift/{username}/{password}/{minutes}/{YYYY-MM-DD:HH-MM}/{stream_id}.{ext}` |

`ext` comes from the account's `allowed_output_formats`, preferring `ts`, then
`m3u8`. The mapping between a channel and its programmes is
`channel.epg_channel_id` = XMLTV `<channel id>`.

**Verified against the user's provider (2026-09-29, ticket 01):**

- `allowed_output_formats` is `["m3u8", "ts", "rtmp"]`, so `ts` is used. Both
  `.ts` and `.m3u8` stream URLs work; the provider answers with a redirect to
  a different streaming host, which mpv follows.
- `epg_channel_id` is set on only about 8k of about 58k live streams (14%); the rest
  are null or empty. Channels without it simply have no EPG (ticket 05).
- Sizes and timing: account check 0.5 KB in 150 ms; about 900 categories, 82 KB in
  470 ms; about 58k live streams, 21 MB in 520 ms. Inserting them (one
  transaction, debug build) took 340 ms.

**XMLTV, verified 2026-09-29 (ticket 05):**

- `xmltv.php` is 78 MB of plain XML (not gzipped) in 3.6 s. It covers about 3.4k
  `<channel id>`s; about 120k of its 230k programmes fall in the kept window (7.1),
  about 50 MB of database.
- About 6.6k of the 8k channels with an `epg_channel_id` match a guide id
  exactly; the rest name ids the guide does not have.
- Matching is exact, case included. Over 300 guide ids differ from another only by
  case, and more than half of those pairs carry different schedules, so
  folding case would mix guides.
- The guide has overlapping programmes on some channels. "On now" is the
  latest-starting programme still airing; the detail panel's "later" list
  only has programmes that start after now.

**Archive, verified 2026-10-02 (catch-up ticket 01)** with
`vidicon --probe-archive <stream_id>` (devtools), which prints shapes and
counts only:

- `get_live_streams` marks about 1k of about 58k streams `tv_archive` 1,
  nearly all with `tv_archive_duration` 3, a few 7.
- `xmltv.php` reaches back about a day: of about 3.6k guide ids, nearly all
  start 6 to 24 h back, about 400 1 to 3 days, none further. It cannot fill
  a 3-day archive by itself.
- `get_simple_data_table` for one channel: about 100 KB in about 100 ms, an
  `epg_listings` array of about 240 programmes reaching about 4 days back
  and 5 ahead. Fields `id`, `epg_id`, `channel_id`, `title` and
  `description` (base64), `lang`, `start`/`end` (clock strings),
  `start_timestamp`/`stop_timestamp` (the server's clock read as UTC, see
  below), `now_playing`, `has_archive`. Those
  in the archive window (3 days on that channel) have `has_archive` 1.
- **The archive URL's start is the server's local time**, the zone in
  `server_info.timezone`, not UTC and not the user's: a start just under
  the server's offset ahead of UTC was served, one just over it was a 404. The offset follows from `server_info.time_now` against the
  local clock, rounded to 15 min. The start has minute resolution.
- `/timeshift/` and `streaming/timeshift.php` (same parameters as query
  pairs) both work, `.ts` and `.m3u8`; the first byte arrives in about
  0.4 s for `.ts`. An `.m3u8` playlist took 1.5 to 5 s and lists 1-minute
  segments with relative URLs and `#EXT-X-ENDLIST`.
- `minutes` is honoured: 1 minute asked for is a body of 60 s of video that
  ends; served faster than real time.
- Out of the window, a start 10 days back or a day ahead, is a 404.
- **Running into now** (start over): `.m3u8` lists the segments up to now,
  with `#EXT-X-ENDLIST`, so it ends there. `.ts` ends there too (checked
  by hand 2026-10-02, catch-up ticket 04: a start over seeked forward
  past now ended), though the probe saw it stream past its 64 MB cap
  (about 4 min of video in 30 s) while short of now. Start over asks for
  the programme to its guide stop plus `record_after`, as a past one.
  When that stream ends (mpv's `end-file` with `eof`, for the latest
  `loadfile` only), the app plays the channel live and the status bar
  says `CAUGHT UP: LIVE`; in the guide grid a selection on that channel
  goes back to now, as with `L` (8.3). A past programme's end is left as
  mpv leaves it.
- No stream carries a broadcast clock (DVB TDT/TOT), live or archive, so
  where an archive starts in the programme can only be checked by eye.
- mpv seeks in both: `seekable` true, +300 s and −120 s land within a few
  seconds. `duration` is exact for `.m3u8` (the playlist's) and meaningless
  for `.ts` (about 25 h for a 45-minute programme).

**Older days** (catch-up ticket 06): `xmltv.php` stops about a day back,
so the guide grid asks for a channel's `get_simple_data_table` when the
page it reads a row for starts in the past, before the first programme
stored on the row's guide id, and the channel's archive reaches into
that page (also when a gap there is genuine). Only rows Slint has built
ask, so channels off screen are not fetched; the selected row's ask goes
first, the rest in the order the rows were read. Each channel is fetched
at most once per EPG refresh (the provider's `last_epg_refresh`), failed
or not. The table's programmes that end before the guide id's first
programme, and after `now - archive_days`, are added with their
`has_archive` (5); one at the same start as a stored programme only
gives it its flag. **Its times are the server's clock**, not UTC: on the
user's provider (2026-10-02) a table's programme matched the same title
in `xmltv.php` 2 h earlier, a day or more apart, 289 times against at
most 16 at any other shift, and played 2 h late. They are moved back by
the server offset (Playing the archive, below), so no table is asked
for before the provider's account check answers. Titles and descriptions are base64. Rows read their
guide again as each table arrives, and a selection on that row is made
again (a gap may now be a programme). A channel with an archive but no
programme in `xmltv.php` is not in the grid (8.2), so it never asks.
Their tables are empty too: checked 2026-10-02 with `vidicon
--probe-tables <id,...>` (devtools, counts only) on 6 of the 62 shown
archive channels without a guide (5 with no guide id, 57 with one
`xmltv.php` lacks), against a channel with a guide (about 470
programmes). So they stay out of the grid; they play live from the list.

**Playing the archive** (catch-up ticket 03): the `/timeshift/` form, with
the live stream's `ext`. The start is the programme's guide start, down to
the minute, in the server's clock: its offset is the account check's
`time_now` read as if UTC against the clock when the check ran, rounded to
15 min, and kept with that provider's check (each launch makes one; until
it answers, catch-up says it waits for it). `minutes` covers the programme
to its guide stop plus `record_after` (4.2), for a guide running late.
mpv's `time-pos` starts at 0 at the URL's start (checked on the demo
provider), so the position in the programme is that start plus
`time-pos`. The offset is the one at the check, so a programme on the far
side of a daylight saving change in the server's zone starts an hour off:
twice a year, for at most the archive's days, and the guide's own times
are right. Accepted rather than carrying a time zone database.

**Movies and series, verified against the user's provider (2026-10-02,
VOD ticket 01)** with `vidicon --probe-vod` (devtools), which prints
shapes, counts, sizes and timings only. Two runs, a debug build, parsing
the lists as they stream in.

- **Categories:** `get_vod_categories` about 440, `get_series_categories`
  about 380, each about 40 KB in about 400 ms. Fields `category_id` (a
  numeric string), `category_name`, `parent_id` (a number). Their ids
  share nothing with the live ones or with each other.
- **Names do not follow the live ones' prefixes.** Live categories begin
  with a region and a bar (`UK| `); movie categories mostly with two
  letters and a dash (`EN - `, about 60 %) or with no prefix at all, and
  series categories mostly with no prefix. None of the user's 5 live
  category rules matches a movie or series category, so under shared
  rules every movie and series would be hidden.
- **Lists:** `get_vod_streams` about 183k movies, 71 MB in 14 to 15 s;
  `get_series` about 49k series, 68 MB in about 7 s. Both are a JSON list
  of objects. Movie fields: `stream_id` and `num` numbers; `name`,
  `stream_type`, `container_extension`; `category_id` a numeric string and
  `category_ids` a list; `added` a numeric string (Unix time); `rating` a
  numeric string or empty, `rating_5based` a number or numeric string;
  `stream_icon` (the poster); `tmdb` a number, string or empty; `trailer`,
  `is_adult`, `custom_sid`, `direct_source` (empty). No year, length,
  genre or plot. Series fields: `series_id` and `num` numbers; `name`,
  `cover`, `plot`, `cast`, `director`, `genre`; `releaseDate` and
  `release_date` (the same, `YYYY-MM-DD` or empty); `last_modified`,
  `rating`, `rating_5based`, `episode_run_time` (minutes), `tmdb`, all
  numeric strings; `backdrop_path` a list, empty or null;
  `youtube_trailer`, `category_id`, `category_ids`.
- Every title has a `category_id` the category list knows. The largest
  movie category holds about 9.5k titles, the median about 125; for series
  about 3.3k and 43.
- `container_extension`: `mkv` about 74 %, `mp4` 24 %, a few `avi`, `m4v`,
  `ts`, one `flv`. Episodes have their own (`mp4` or `mkv` seen).
- **Info:** `get_vod_info&vod_id=` 1 to 3 KB in about 80 to 330 ms, an
  object with `info` (`name`, `o_name`, `plot` = `description`, `cast` =
  `actors`, `director`, `genre`, `country`, `releasedate` `YYYY-MM-DD`,
  `rating`, `duration` `HH:MM:SS`, `duration_secs`, `runtime`, `bitrate`,
  `cover_big` = `movie_image`, `backdrop_path` a list, `tmdb_id`, `age`,
  `status`, sometimes `episode_run_time` or `kinopoisk_url`) and
  `movie_data` (`stream_id`, `name`, `container_extension`, `added`,
  `category_id`). Cast and country are empty on some. **`duration_secs`
  is unreliable:** of 3 movies one had 5400, one 0, and one 93 for a file
  mpv measured at 5594 s (minutes, not seconds). Length comes from mpv
  once playing. `duration` matched mpv's length on the movies the user
  played (2026-10-03, VOD ticket 04), so the detail panel and the row
  show it; the app never reads `duration_secs`.
- `get_series_info&series_id=` 15 to 25 KB in about 70 ms (once 6 s).
  Keys `info` (as the list's fields), `seasons` and `episodes`.
  `seasons` is a list of `season_number` (a number), `name`, `overview`,
  `air_date`, `episode_count`, covers; **it describes the show, not what
  the provider has**: one series listed 3 seasons and served only the
  first. `episodes` is an object keyed by season number (`"1"`), each a
  list of `id` (a numeric string: the stream id), `episode_num` and
  `season` (numbers), `title` (the provider's whole title, longer than
  the episode's name), `container_extension`, `added`, and `info`
  (`duration_secs` a number, `duration`, `air_date`, `rating`, `bitrate`,
  `movie_image`, `video` and `audio` objects, an `id` that is not the
  stream id). Season pills come from `episodes`' keys (VOD ticket 05);
  `seasons` is never read. The 3 series
  sampled had 6 to 10 episodes each, one season each, too few to say how
  many episodes there are in all.
- **Ids:** movie `stream_id`s (about 11k to 2.2M) never equal a live one.
  `series_id`s (1 to about 51k) equal about 1k live stream ids and about
  13k movie ids, so favourites and history need the kind in their key.
  The episodes sampled (24) collided with nothing, but share the movies'
  number range.
- **Streams:** `/movie/{username}/{password}/{stream_id}.{container_extension}`
  and `/series/{username}/{password}/{episode id}.{container_extension}`
  both answer 302 to a different streaming host, as live does. Ranges are
  honoured there: `bytes=` at the start and the middle each answer 206 with
  a `Content-Range` giving the full size (2.8 GB for a 93-minute movie,
  110 MB for a 32-minute episode), first byte in 0.15 to 0.8 s. The
  `Accept-Ranges` header carries a byte range rather than `bytes`, so
  ranges are tried, not read from it. `Content-Type` is the container's
  (`video/x-matroska`, `video/mp4`).
- **mpv:** `time-pos` starts at 0; `duration` is the file's (5594 s,
  1901 s); `seekable` is true and +300 s and −120 s land within 6 s.
  `loadfile <url> replace -1 start=600` (mpv 0.41) starts at 600 s, so a
  resume is one `loadfile`.
- **Downloads:** a plain GET ran at about 18 MB/s (370 MB in 20 s), so a
  movie of 3 GB takes about 3 min. `active_cons` read 0 before and 5 s
  into it, so the account check does not show a VOD connection.
  `max_connections` is 1 on this account; whether a download beside a
  playing stream is refused was not tried, to keep from tripping the
  provider's limit.
- **Posters:** every `stream_icon` and `cover` is on a third party host:
  for movies about 122k distinct URLs on 116 hosts, the largest holding
  about 69 % and the next three 19, 9 and 2 %; for series about 37k on 91
  hosts, the largest about 81 %. About 77 % `https`, the rest `http`, and
  3 `data:` URIs. 16 sampled all answered: JPEG, nearly all 600×900 (a
  few 667×1000 or 154×231), 15 to 190 KB, about 100 KB typical, in 12 to
  180 ms.
- **Search size:** about 232k movie and series names, 7 MB as stored
  (about 31 bytes each), four times the live list. They fit in memory as
  live search keeps its names; per keystroke time grows with the count
  (3 to 6 ms at 58k, 8.5), so 183k movies is near SPEC 10's 16 ms.

**External XMLTV, verified 2026-10-13 (EPG sources ticket 01)** with
`vidicon --probe-epg <url-or-path>` (devtools), which prints counts
only. Two feeds the user has, a world one and a European one, against
the provider's about 57k stored channels (about 17.7k shown, 1.4k of
those with the provider's guide). A debug build.

- **Size:** 19 and 25 MB gzipped (a `.gz` file, not `Content-Encoding`),
  175 and 192 MB of XML, read and parsed in 7 to 9 s. About 18k and
  16.5k channels, about 2.3 display names each; about 530k and 520k
  programmes. Every programme time is `+0000`.
- **Reach:** nearly every channel starts 6 to 24 h back, a few hundred
  3 to 7 days. Ahead, at most about 1 day (world) and 2 days (Europe),
  so the 3 days kept ahead (7.1) covers both.
- **Ids hardly match:** about 0.5k (world) and 1.9k (Europe) channels
  match by exact `epg_channel_id`. Names do the work.
- **Names, world feed:** `name_norm` alone matches about 21.9k
  channels, 1.1k ambiguous. Also dropping region prefixes, brackets and
  punctuation matches fewer, about 20.1k with 5.2k ambiguous: regional
  variants of a channel collapse into one name. `name_norm` first and
  the looser form only when it finds nothing matches the most, about
  23.2k with 2.4k ambiguous. Of the shown channels that rule matches
  about 14.8k (0.3k by id), 0.8k are ambiguous and 2.1k have none;
  about 13.5k of them have no guide from the provider today.
- **Names, European feed:** the same order: about 19.9k by `name_norm`
  alone (0.9k ambiguous), 18.1k by the looser form (4.5k), 20.7k by
  both in turn (1.9k). It matches few shown channels, about 1.5k of
  17.7k, as most shown categories are outside Europe; there the looser
  step adds the most, about 1.3k by name against 1.1k by `name_norm`
  alone.
- **Times:** programmes near now were paired by title with the
  provider's guide on channels both have. World feed: matched by id,
  176 of 194 channels agree exactly, 7 are 5 h 30 min off; matched by
  name, 201 of 286 agree, 29 are 2 h off and 12 are 3 h off. European
  feed: by id 353 of 379 agree, by name 303 of 339, the rest scattered
  within 2 h. The world feed's name matches are off more often: the
  same programmes on a regional variant, or the provider's guide off on
  those channels, which the probe cannot tell apart. So a source's own
  shift is 0 on both feeds, and per-channel shifts (pins) are where the
  offsets show.
- **Looser name form** (EPG sources ticket 02): `name_norm` after
  dropping a leading 2 to 4 letter or digit tag closed by `|`, `:`, `]`,
  `)` or `-` (`UK| `, `[UK] `, `US: `), bracketed text, and punctuation
  other than `+`. It crosses regions by design: `UK: Northwind Sport` and
  `4K: Northwind Sport` are one name there. A channel's alternates use it
  too (`channel.name_loose`, 5; 8.2).
- **xz feeds** (M3U ticket 05): playlists name `.xml.xz` feeds as well
  as gzipped ones (a list in `research/reference-players.md` section 7
  names four), so every guide source may be xz. The same synthetic
  80 MB feed imports in 1.5 to 1.8 s from xz (6.8 MB) against about
  1.1 s from gzip (9.3 MB): unpacking xz takes about 0.5 s of it
  (`docs/perf/v1.md`). A background import, so it stays.

### 6.2 Politeness

All three reference players have reports of provider IP bans.

- At most one request in flight per provider.
- At least 2 s between requests to the same provider.
- On 429 or 5xx: exponential backoff starting at 30 s, capped at 30 min.
- A fixed, honest `User-Agent` (`vidicon/<version>`).
- The account check runs once per refresh, not per request. It also runs
  once at launch, so expiry and max connections show while cached data is
  fresh (ticket 08).
- Playback does not count against these limits (mpv opens the stream).
- External guide sources (EPG sources ticket 03) are not the provider's
  and have limits of their own, set per source (4.1, 7.2): `every_hours`
  between scheduled fetches, an optional `max_per_day`, and a 429 or 503
  with `Retry-After` (seconds or an HTTP date) backing that source off
  until then, `r` included. Without `Retry-After` the response is a
  failure like any other. Nothing is tuned to any one EPG service.
- Guide tables (catch-up ticket 06, 6.1) are provider API requests under
  these limits, waiting behind any other API request as the provider's
  own logos do (below), so a refresh never waits on them. About 100 KB
  each, one per channel on screen.
- A movie's info (VOD ticket 04, 6.1) is the same: `get_vod_info` for
  the selected movie only, the latest selection first, behind any other
  API request, each movie once per import of the list. At most 3 wait;
  older ones are dropped, so holding `j` through a list asks for little.
  A failed one is asked again when the movie is selected again. A
  series' `get_series_info` (VOD ticket 05) goes the same way, in the
  same queue, each series once per import of the series list.
- Channel logos (ticket 13): a logo on the provider's own host goes through
  that provider's limiter, behind any API request. Most logos are on third
  party hosts (2026-09-29: 91% on one image host, 26 hosts, about
  14.5k distinct URLs for about 58k channels); those get their own limit per host:
  at most 2 requests in flight, at least 250 ms between request starts, the
  same 429/5xx backoff, and the same `User-Agent`. The gap was added after
  fast scrolling drew 429s from an image host (2026-09-29).
- Posters (VOD ticket 06) go the same way, in the same queues per host as
  logos, so a host serving both still has 2 in flight. They have their
  own disk cache (`$XDG_CACHE_HOME/vidicon/posters/`, 256 MB, pruned to
  80 % least recently used first): kept as JPEG no taller than 240 px,
  the detail panel's 120 px at 2x, about 12 KB each, so it holds about
  20k. A broken one (4xx, not an image) is not asked again for a week;
  a `data:` URI shows the tile.

Tuning (ticket 01): a full channel refresh is three requests, so the 2 s gap
costs about 4 s of background time, which the user never waits on. The first
real refresh saw no 429 or 5xx. The numbers above stay as they are; loosen
them only if a refresh becomes user-visible.

### 6.3 Account status

Show expiry date and max connections from `user_info`. Expired or disabled
accounts block refresh and show a clear message; cached data stays browsable.
Wrong credentials answer `auth` 0, or on some providers HTTP 513; the
add and edit forms' check (4.1) refuses both.

`max_connections` also limits scheduled recordings (9.5): mpv's stream, if
it plays from the provider, and each running recording take one. A missing,
mistyped or non-positive value counts as 1, as does any before the first
check answers. A download (9.6) takes one too, but only one nothing else
needs: playback or a recording that needs it pauses the download.

The status bar's expiry warns ahead (ticket 22). More than 7 days out, or
with no expiry, the status bar shows nothing; within 7 local calendar days
it shows a relative expiry: `Expires in 5 days`, `Expires tomorrow 18:00`,
`Expires today 18:00`. 4 to 7 days are amber (`warn`); 3 days or less, and
an expired or disabled account, red (`danger`). An expiry passing while the
app runs shows `Expired` at once, before the next check. The minute tick
re-reads it; there is no timer of its own.

The account dashboard (ticket 21, 8.2) shows the rest of the check: status,
expiry, `created_at`, `is_trial`, `active_cons` of `max_connections`,
`allowed_output_formats` and the provider's `message`, and from
`server_info` the timezone, `time_now`, ports and protocol. Every field is
optional and may come as a string or a number; a mistyped one is left out
(`server_info` is sometimes `[]`). The response also echoes the username and
password, and `server_info.url` is the provider's host: none of those are
parsed. Opening the dashboard asks the provider's scheduler for one account
check, through its limiter (6.2), refreshing only data that is stale; the
last check shows meanwhile. Opened during a refresh, the check runs when
the refresh ends.

### 6.4 M3U playlists

**Verified 2026-10-22 (M3U ticket 01)** with `vidicon --probe-m3u
<url-or-path>` (devtools), which prints counts only and fetches a URL
twice, 5 min apart. Two lists: a public one, and one an M3U and EPG
generator builds from the provider's Xtream account (a smaller
selection), as the provider's own `get.php` is refused (below). So the
second list shows the generator's habits, not the provider's. A debug
build.

- **Size:** about 11k entries in 2.6 MB and about 19k in 6.2 MB, each
  read in under 2.5 s. Every entry has an `#EXTINF` and a URL; no blank
  or comment line sits inside an entry, and neither list has a BOM. The
  public list ends every line in CRLF, the generator's in LF.
- **Attributes:** every value is double-quoted in both. `tvg-id`,
  `tvg-logo` and `group-title` are on nearly every entry. The public
  list has no `tvg-name`; the generator's has one on every entry, and
  about 400 entries have it without a `tvg-id`. The generator adds
  attributes of its own (an id on every entry, and a few prefixed
  ones) that the parser ignores. `tvg-chno` is on 41 of its entries,
  none of them a number.
- **Titles:** about 330 generator titles hold a comma. The title starts
  after the first comma outside quotes; the last comma would cut them.
- **Groups:** 30 and about 170. About 600 public entries have
  `;`-separated groups.
- **Duplicates:** none on the public list. On the generator's, about
  1.5k `tvg-id`s are on more than one entry (5k entries), and about 900
  URLs are too: one stream in several groups.
- **Options:** about 590 public entries set `http-user-agent` and 240
  `http-referrer` by `#EXTVLCOPT`, and the same entries carry both as
  `#EXTINF` attributes too. No `#KODIPROP` and no pipe suffix in either.
- **URLs:** public, mostly `https`, about 2.4k `http`, and under ten
  `rtmp`, `mmsh` and `srt`; the longest about 1.2k characters.
  Generator, `http`, about 60 characters, with no token.
- **Header:** no attributes on the public list; one gzip `url-tvg`
  feed on the generator's.
- **HLS tags:** the generator's list has one `#EXT-X-CREDENTIALS` line.
  So a playlist counts as an HLS stream only with
  `#EXT-X-TARGETDURATION` or `#EXT-X-STREAM-INF`; other `#EXT-X-` lines
  are skipped like any `#` line.
- **Catch-up:** none on the public list. About 330 generator entries
  (2%) have `catchup-type` and `catchup-days`, none `catchup-source`,
  and the provider behind it has catch-up through Xtream already. M3U
  catch-up is not worth a milestone yet.
- **Between fetches:** nothing changed on either list: no URL, no
  order, and no `tvg-name` under the generator's 13.7k unique
  `tvg-id`s. Neither list showed a reorder or a rotated token, so this
  is a judgement more than a measurement: identity is `tvg-id`, else
  `tvg-name`, else the title; the first entry with a key has the key's
  hash alone, later duplicates are told apart by a URL hash, which
  survives the reorder that adding channels causes, with an occurrence
  index only among entries that share the URL too. A rotated token
  changes a later duplicate's id; the favourites name fallback covers
  it when the names differ. Duplicates that swap places change ids.
- **`get.php` refused:** the provider's own playlist URL answers HTTP
  884 with an empty body, from a proxy in front of the panel, for any
  user agent, `type` and `output`, while `player_api.php` works. A
  panel's playlist cannot be assumed (M3U ticket 06).

## 7. Import and refresh

### 7.1 Import (ADR 0003)

- **Channels:** deserialise `get_live_streams` as a stream of elements, not a
  `Vec`. Insert into the store inside one transaction, replacing the provider's
  previous channels. Then run the favourites fallback (5).
- **EPG:** stream `xmltv.php` (gunzip if gzipped) with a pull XML parser.
  Only keep programmes that end after `now - 1 h` and start before
  `now + 3 days`; on a guide id with an archive (5), those that end after
  `now - archive_days`, taking the longest when channels with different
  archives share it (catch-up ticket 02). The channel import runs first
  (7.2), so the import reads the guide ids with an archive into a map
  first: about 1k entries, not the channel list. `xmltv.php` reaches only
  about a day back (6.1), so that is what an archive's guide gets from it.
  Replace the provider's programmes in one transaction, except older days
  on a guide id with an archive (catch-up ticket 06): programmes that
  have ended there are set aside first (a temporary table, about 16k rows
  on the user's provider), and those that end before the new guide's
  first programme on that guide id, and still after `now -
  archive_days`, are put back with their `archive` flag. So days fetched
  from a table (6.1), and those an earlier `xmltv.php` reached, stay
  until they leave the archive; a guide that reaches back over them
  replaces them. Measured on a copy of the real database: about 130 ms
  each way.
  In the same transaction, each unstarted schedule (9.5) moves to the
  times of the programme on its `guide_id` with its title starting nearest
  its old start, if one starts within 3 hours of it; its padding moves with
  it. Further off is more likely another showing, so no match leaves it.
- **Guide sources** (EPG sources ticket 02, 4.1): each enabled source is
  streamed, gunzipped or unpacked from xz when its first bytes say so
  (M3U ticket 05; a pure-Rust decoder, so packages gain no library), with
  the same parser,
  into one transaction of its own. Its channels are stored as they come;
  at its first programme (XMLTV lists channels first) the provider's
  channels are matched against them (5), and only matched channels'
  programmes are kept, moved by `shift_minutes`, in the window above, the
  past by the longest archive among the channels matched to each feed
  channel. Ended programmes of archived channels are set aside and put
  back as for `xmltv.php`. A source that fails to fetch or parse rolls
  back to its last good data, stores the reason, and is skipped; the
  provider's refresh goes on. Catch-up on a source's programme goes by
  its channel's archive (`archive` is NULL there), at its shifted times.
  xz's checks fail a cut-off body, and `</tv>` is required whatever the
  packing.
- **An M3U provider's guide feeds** (M3U ticket 05, 4.1): `url-tvg` and
  `x-tvg-url` on the playlist's first line, each a comma-separated list,
  name its feeds, in that order, each once; only `http(s)` URLs count, so
  a playlist cannot point the app at a local file. The import stores
  them with the channels, and a rejected playlist keeps the old ones.
  The order then holds them in the provider's guide's place, each a
  source of its own: fetched with conditional requests and the limits of
  7.2, failing alone, kept, matched and shifted as any other. A feed gone
  from the header is deleted as a source gone from the file. The
  refresh that reads a playlist stores its feeds' order before the EPG
  stage, so new feeds import in that same refresh.
- **M3U playlists** (M3U ticket 02, 6.4): the provider's playlist is
  read a line at a time, as it downloads or from its file, into one
  transaction that replaces its categories and channels, then the
  favourites fallback and the rest of a channel import run. An entry runs
  from `#EXTINF` to the next non-empty line that is not a `#` line, its
  URL, with no scheme or length check. `#EXTVLCOPT` lines between the
  `#EXTINF` and the URL set the stream's headers (M3U ticket 03):
  `http-user-agent`, `http-referrer` and `http-origin`; a Kodi-style
  suffix on the URL (`url|User-Agent=...&Referer=...&Origin=...`, keys
  in any case, values percent-decoded) sets those it leaves unset, and
  is cut off the URL. A value holding a control character is dropped.
  Other `#EXTVLCOPT` options, `#KODIPROP`, other `#` lines and blank
  lines are skipped. A URL with no `#EXTINF` is kept,
  named after its path's last segment; an `#EXTINF` with no URL is
  skipped and counted, shown in the refresh panel (`12 entries
  skipped`). A UTF-8 BOM, CRLF, attribute values double-quoted,
  single-quoted or unquoted, and commas in quoted values and in the
  title (everything after the first comma outside quotes) are taken.
  The title is the name, else `tvg-name`; `tvg-id` the `epg_channel_id`;
  `tvg-logo`, `tvg-icon` or `logo` the logo; `group-title` or `group`,
  its first `;`-separated value, the category, else "Uncategorised";
  `tvg-chno` or `tvg-ch`, when a number, the number, else its place
  among the entries. Ids are given at the end of the import from a
  temporary table of hashes (5), so the list is never in memory. The
  playlist is rejected, keeping the stored channels, when it does not
  start with `#EXTM3U`, when the download is shorter than its
  `Content-Length`, when it has no entries, and when it is an HLS stream
  playlist (`#EXT-X-TARGETDURATION` or `#EXT-X-STREAM-INF`: "this is a
  stream, not a channel list"). Measured: 50k entries from request to
  commit in about 2 to 3 s (`docs/perf/v1.md`).
- **Movies and series** (VOD ticket 02): `get_vod_categories` then
  `get_vod_streams`, and `get_series_categories` then `get_series`, each
  streamed into one transaction as channels are, replacing that kind's
  categories and titles only. Then the kind's overrides are re-pointed
  (5). Measured on the user's provider (2026-10-02, release build, a copy
  of the real database): about 183k movies, 75 MB, about 6 s from request
  to commit; about 49k series, 71 MB, about 3 s; the parse keeps up with
  the download. The database grew from about 60 MB to about 137 MB
  (`docs/perf/v1.md`).
- A failed import rolls back and leaves the previous data untouched.
- Peak memory of an import must not scale with list size beyond the current
  element.

### 7.2 Refresh rules

- On launch: show cached data immediately. If channels or EPG are older than
  `refresh_hours`, refresh in the background.
- While running: check staleness every 15 min.
- **Movies and series** (VOD ticket 02) refresh on their own schedule,
  each when older than 24 h, whatever `refresh_hours` says: the lists are
  about 70 MB each against 21 MB for the channels (6.1). The same 15 min
  check looks at them.
- Manual refresh key forces all four, still subject to 6.2.
- Channels refresh before EPG, and both before movies and series, so live
  data lands first. The EPG stage is the provider's guide, unless it is
  off, then each enabled guide source in the file's order (EPG sources
  ticket 02); a failing source is logged and does not fail the stage. A
  source due makes the EPG stage due, so one added to the file imports on
  the next pass; the provider's guide is then fetched only if it is stale
  itself.
- **Guide sources** (EPG sources ticket 03): a URL is due once
  `every_hours` (default 12) have passed since it was last asked, a
  path when its modification time differs from the one last read; a
  source never imported nor failed, marked for a re-import by a manual
  mapping (5, EPG sources ticket 05), or whose `Retry-After` has passed,
  is due at once, and `r` makes every enabled source due. A URL is asked
  with `If-None-Match` and `If-Modified-Since` from its last good fetch;
  a 304 costs that request and imports nothing. Every request to a URL
  counts, conditional, failed and `r` ones included, over a rolling 24 h
  in `source_request` (5), so the count survives a restart. With
  `max_per_day`, a scheduled fetch is skipped once the count is 2 short
  of it, leaving 2 for `r`, and `r` skips the source at the cap; a
  source skipped or backing off does not make the stage due. A source's
  turn ends unchanged, fetched with its programme count, skipped at the
  cap with the count and when the next request may go (`20/20 in 24 h,
  next 14:05`), backing off until a time, or failed with the reason. Each
  source the stage gives a turn has its own row in the refresh panel (8.2,
  EPG sources ticket 04), after the EPG row, with its own progress: the
  body read of its fetch, which does not go through the provider's
  client. A source not due on a scheduled pass, one whose file has not
  changed included, has no row.
- **M3U providers** (M3U ticket 02): a refresh is the channels and EPG
  stages only, with no account check and no movies or series. A playlist
  URL is one GET through the provider's client, under the limits of 6.2,
  on the same schedule and `r`; a playlist path is read again when its
  modification time differs from the last read's, or on `r`, not by age.
  The EPG stage has no `xmltv.php` to fetch; external guide sources run
  as for any provider. The playlist's feeds (4.1) stand in for
  `xmltv.php`, so they are due when older than `refresh_hours`, not on
  a source's 12 h; a source in the file with a feed's URL keeps its own
  `every_hours`.
- An edit in Settings › EPG (4.1) asks the provider's refresher for a
  pass at once, as `refresh_hours` does; one asked while a refresh runs
  follows it.
- The UI shows refresh state unobtrusively (idle, refreshing with progress,
  last success time, last error). Each stage (channels, EPG, movies,
  series) reports to the refresh panel (8.2, refresh panel ticket 01):
  waiting, running with progress, done with its row count, backing off
  with time left, or failed with the reason. Progress is the body read
  of that stage's list or guide, as a percentage of its `Content-Length`
  when the provider sends one (not for a gzipped body, which reqwest
  unpacks), else in MB; 100 % waits for the import to finish.
- First run, a provider whose channels were never imported, shows the
  first-run screen (8.2) rather than an empty list, until the channels
  stage is done.
- An EPG import tells the recorder (9.5), which re-reads its schedules, as
  the import may have moved them.
- A 429 or 5xx retries by itself when its backoff (6.2) ends, rather than at
  the next 15-minute check. The retry runs the stages that refresh had not
  done, stale or not, so one started with `r` finishes. An `r` pressed
  during a full refresh is absorbed by it.

## 8. UI

### 8.1 Toolkit (ADR 0005)

Slint. Use `ListView` directly for every long list. Font: Inter, headings in
Inter Display. Dark theme.

Every colour comes from the `Palette` global, whose base tokens Rust
sets (themes milestone, `docs/milestones/themes.md`); the translucent
ones are derived from them. `theme = "default"` (4.2) is the built-in
palette. `theme = "noctalia"` reads the palette file (Paths, 3), which
Noctalia renders from Vidicon's template (`contrib/noctalia/vidicon.toml`,
set up as in the README's Themes): TOML, `token = "#rrggbb"` per
base token, named as in `Palette` (`bg`, `surface-2`, `logo-backing`).
A token left out keeps its default; an unknown key is ignored. The app
checks the file's mtime once a second while the theme is `noctalia` and
applies a change live. A missing or unreadable file gives the default
palette; a bad one (`noctalia-palette.toml: line 3: not a colour`)
keeps the last good. Either way the Theme field in Settings says why
(`No Noctalia palette at <path>`).

Any other value names a theme (built-in themes milestone,
`docs/milestones/built-in-themes.md`): a user theme
`themes/<name>.toml` (Paths, 3) first, then a built-in, so a user file
with a built-in's name replaces it. A user theme is the same format,
read and watched as the Noctalia file is (`No theme "<name>"` when it
is missing). Every name's file is watched, whether or not it exists
yet, so one created or removed while the app runs applies live. For a
built-in's name, the built-in stands in for a missing or bad file
instead of the default palette. An unknown name gives the default
palette and the Theme field says `No theme "<name>"`.

Wayland app ids, for compositor window rules: `vidicon` for the app window,
`vidicon-mpv` for the Player (9.1).

### 8.2 Layout

Decided: a combination of the three prototype layouts (`prototype/slint`
branch).

- **Main screen: classic three-pane.** Category sidebar, channel list with
  now-playing and progress, detail panel with the selected channel's now/next
  and the rest of today.
- **Sidebar (the library)** (VOD ticket 03): Favourites, Recently played
  and Continue watching, which span the kinds; then a Live · Movies ·
  Series switch (Tab and Shift-Tab cycle it, a click picks); then the
  chosen kind's All (`All channels`, `All movies`, `All series`), for
  movies and series Recently added, and the provider's categories of that
  kind. It opens on Live, All channels. Switching kind goes to the kind's All,
  from a view above the switch too, and so does picking the kind already
  chosen while one of those is selected; a kept search filter is
  dropped. Recently added lists the kind's latest 200 by `added`
  (movies) or `last_modified` (series), newest first, less hidden
  categories. Favourites and Recently played hold channels, movies and
  series (VOD ticket 11), each row drawn as its kind's is elsewhere, and
  each row's keys are its kind's: Enter on a movie plays it, on a series
  moves into its episodes. Their header counts `3 channels` while every
  row is a channel, else `5 titles`. Continue watching (VOD ticket 07) lists the movies stopped
  part way and the series with an episode to carry on with (5), latest
  first, whatever their categories; a series whose episodes are not read
  yet is listed on what was played, a finished one is not. Its header
  counts `2 titles`, its keys are the movies' and series', and `t` says
  the guide grid is for live channels. A save moves its title to the top
  while the view is open, the selection following it. Its header has a
  Clear button while it has entries, armed and confirmed as Recently
  played's: clearing empties the list but keeps positions and watched
  marks (5), and a title played again comes back. Empty, it reads
  "Nothing part way through yet." Recently played lists up to 50 titles, latest
  first, in the same rows as any other list; a play moves its title to the
  top while the view is open, the selection following it. A movie, or a
  series for any of its episodes, counts as played when it is sent to
  mpv. Its header has a
  Clear button while it has entries. Empty, it reads "Nothing played yet."
  Only shown categories (5) are listed, and All holds only their channels,
  less those the channel rules and kept tags hide; Favourites and Recently
  played list their channels regardless. With every category of the kind
  hidden, the library reads "No categories shown. Press e to choose
  which." under the kind's All (and Recently added), and All "No channels
  shown. Press e to choose which." (`movies`, `series`).
- **Manage mode** (ticket 20): `e`, or the funnel button in the library
  header (accent border while any category or channel is hidden), turns the middle
  column into a centred list of every category, the side panes folded as
  in search mode. Top to bottom: `Categories`, a Live · Movies · Series
  switch (VOD ticket 02; Tab and Shift-Tab, or a click) choosing whose
  categories, rules and overrides the rest shows, `12 of 912 shown · 3480
  channels` (`· 9120 movies`, `· 2210 series`) and a Done button; it
  opens on the library's kind (VOD ticket 03); a filter field (typing filters, a
  case-insensitive substring as a rule matches, so the rows are what the
  filter would show as a rule); the rules as chips, `×` removes one, and a
  button saying what Ctrl+Enter does with the filter (`Show only matching`,
  `Also show matching` once there are rules, `Remove rule` if the filter is
  one); on Live only, `Hide names` with the channel rules (ticket 25) as chips, each with
  how many channels of shown categories it hides and `×` to remove it, and
  a button saying what Ctrl+H does with the filter (`Hide names with “fr:”
  · 3`, counting live as the filter changes, or `Unhide names with “fr:”`
  if the filter is one); on Live only too, `Keep tags` with the kept tags (ticket 26) as
  chips with `×`, then `hides 9800`, how many channels of shown categories
  they hide in all, and a button saying what Ctrl+K does with the filter
  when it is a tag (`Keep tag “uk” · hides 9800` for the first, `Also keep
  “fr” · shows 410` after, both counting live, or `Stop keeping “uk”`); then
  one row per category with a `Shown`/`Hidden` pill, the name, `override`
  when its own override decides it, and its channel count (`5 of 8` once
  channel rules or kept tags hide some), or its movie or series count. The rows of chips wrap onto more
  lines rather than push their button out of view. Each edit is saved at once and the library behind is
  re-read.
  Categories follow the order (below).
- **Order** (ticket 19): provider order or A–Z, one setting for the
  categories and for the channels, movies and series in All and in each
  category (Recently added keeps its own), kept in
  `app_state` for every provider. A pill in the library header reads
  `Provider` or `A–Z` (accent border) and toggles it on click, like `a`. A–Z
  compares `name_norm` (5), ties in provider order, so quality tags do not
  scatter a name; categories sort by their name folded the same way, which
  groups them by region prefix. All, Favourites and Recently played stay on
  top; Favourites keep their manual order, Recently played its recency, and
  search results best match first. The selection stays on the same category
  and channel across a toggle. Both orders are read straight from an index.
- **Detail panel** (ticket 12, 260 to 340 px wide), top to bottom: the logo
  (88 px), number and category, chips for favourite, stream format and
  alternates; the name; the NOW card, or "No guide for this channel" with one line of
  explanation; LATER, taking what space is left and scrolling the rest,
  its heading naming the guide's source on the right (`from World feed`,
  `from epg.example.net`, or the provider's name for its own guide; EPG
  sources ticket 02, 4.1);
  RECENTLY PLAYED (the last 4, click plays); the NOW PLAYING card, only while
  mpv runs. That card: logo, channel, programme; start and end times on the
  left and time left on the right, above the programme's progress bar; without
  a guide, "Watching for N min" and no bar. Its heading reads `NOW PLAYING`,
  `PAUSED`, or, once behind live (ticket 14), `BEHIND LIVE · 2 min` / `PAUSED
  · 2 min behind`, in whole minutes (`<1 min` under a minute; paused, plain
  `PAUSED` until a minute), refreshed at minute boundaries. While catch-up
  plays (catch-up ticket 03) it reads `CATCH-UP` (`PAUSED · CATCH-UP`), in
  the behind-live colour, with the programme replayed, its times (with
  `Yesterday` or `Sat 3 Oct` unless today), time left and progress from
  its guide times and mpv's `time-pos`, asked every second while it plays
  (never `duration`, 6.1); go live is lit. Playing live on a channel with
  an archive and a programme airing in the guide, `START OVER` in accent
  on the heading's right (catch-up ticket 04). While `R`
  records it, a red `● REC 0:12` there instead: minutes and
  seconds recorded (`1:05:30` past an hour), redrawn each second by a timer
  that runs only while recording. Then pause, go
  live, record, stop, mute and fullscreen as borderless icon buttons,
  accent-tinted while on (record in red, as `● REC`), no key labels. A click
  on record does what `R` does. Go live is always there, dimmed while at
  live, so the buttons never shift. The panel's contents are not built in search mode.
- **Alternates** (stream recovery ticket 03): a channel's other listings,
  the other channels All shows with its `name_loose` (5), so the same
  broadcast under another region or quality tag (`4K: Northwind Sport`,
  `US: Northwind Sport HD`). Those with its `name_norm` first, then its
  own category's, then by number; at most 50. With any, the detail panel
  has a chip `+1 listing` / `+3 listings` (`+50 listings or more`),
  accent on hover and while they are open. `o` or a click on it opens
  them in LATER's place, headed `OTHER LISTINGS` with `Enter plays · Esc
  back` on the right: one row each, its number, name and category, the
  selected one with the accent border, kept in view. Enter or a click
  plays one; the list stays open on it, so the next can be tried. Each
  move in the list counts them (one indexed lookup, under 0.2 ms on the
  user's database); their rows are read only when opened. The app never
  switches to one by itself: an alternate can be another region's feed,
  which its name and category show.
- **Guide card** (EPG sources ticket 05): `E` on a live channel, in the
  list or the guide grid, opens a card over the screen, `Guide for
  <channel>`, its rows the provider's guide sources in order, named as
  Settings › EPG names them, each saying what it gives the channel:
  `Matched: <its channel's name>`, `Mapped: <name>`, `Ambiguous:
  several of its channels match · Enter picks one` (in amber), `No
  match · Enter picks a channel`, for the provider's guide `Matched by
  its guide id`, `No programmes for its guide id <id>` or `The provider
  gives it no guide id`; `· no programmes for it` where so, `Off · `
  before a source turned off and `· re-import at the next refresh` after
  one marked. On the right, in accent, `PINNED` (`PINNED +60 min` with a
  shift) and `IN USE` on the one its guide comes from, then a `Pin` /
  `Unpin` button. `j`/`k` move, `p` pins the channel to the selected
  source or unpins it, `h`/`l` step the pin's shift by 5 minutes, Enter
  opens the mapping picker on that source (on the provider's guide the
  status bar says it matches by the guide id only), Esc, `E` or `q`
  closes. Each change is written at once and the panes, the grid and the
  card show it. On something other than an available live channel, `E`
  says so in the status bar.
- **Mapping picker** (EPG sources ticket 05): over the guide card or
  Settings › EPG's ambiguous list, `Map <channel> in <source>`, a search
  box typed into as search mode is, filled with the channel's name in the
  looser form (6.1), and the source's channels whose id or display names
  hold every word typed, ignoring case, by name, at most 200: each its
  first name, then its id and other names. While the channel is mapped,
  a first row `Automatic` forgets the mapping, and its channel reads
  `MAPPED`, selected. The arrows and page keys move, Enter maps (the
  status bar says `Mapped <channel> in <source>: its programmes come
  with the next refresh`) and closes it, Esc closes it. With no rows, it
  says the source has not been imported, or that none holds the words.
- **List rows** have a 32 px logo (40 px in search mode) before the name.
  Movie and series rows (VOD ticket 03) have no number; a 2:3 poster
  42 px high (54 px in search mode), its initials on a tile while it
  loads or if it has none or fails (VOD ticket 06), then the title and, below, a series' year and the rating (`2019 · ★
  8.1`; a movie's list has no year, so `★ 7.4`). A rating of 0 or none
  is left out. In a list holding both channels and titles (Favourites,
  Recently played, a widened search), a title row says `Movie` or
  `Series` in the number column, in the numbers' style, and its poster
  is centred in the logo's width, so stars, tiles and names line up; a
  widened search's line under the title then drops its `Movie ·` or
  `Series ·`, which it keeps when no channel matched. Once a movie's info is in (VOD ticket 04), its row reads
  `2019 · 1 h 33 min · ★ 7.4`; once a series' is (VOD ticket 05), its
  years run to its latest episode's (`2014 – 2016 · ★ 8.0`). A movie
  stopped part way has a bar after it, one watched `✓ Watched` at its
  end (VOD ticket 07); in Continue watching a series' line is its
  episode part way, with a bar, or `Next: S1 E2 · Slack Water`. The header
  counts `183 movies`, `47 series`. Enter or a double click on a movie
  plays it (VOD ticket 04); on a series, keys move into its episodes
  (below). Until VOD ticket 09, `/` in a list of movies or series only
  says so in the status bar (`Searching series is not ready yet`); in
  Favourites and Recently played it searches their channels only. `t`
  with Movies or Series chosen says `The guide grid is for live
  channels`.
- **Detail panel for a movie or series** (VOD ticket 03): a 2:3 poster
  tile 120 px high, its category and, for a movie, its format chip; the
  title; the row's meta line; the plot; then GENRE, CAST, DIRECTOR,
  EPISODES (a series' episode length) and ADDED (a movie) or UPDATED (a
  series), each only if given; a series has its episodes instead (VOD
  ticket 05, below). A movie's come with its info, asked for when it is
  selected (VOD ticket 04, 6.2); until it is in, the panel shows what the
  list gives, and fills in when it comes, as the row does. Below the
  title (VOD ticket 07), a movie stopped part way has the NOW card as
  RESUME: `From 0:42:10`, `51 min left` and a bar, with `START OVER` at
  the heading's right as on the NOW PLAYING card; a series has it for its
  next episode, RESUME with the episode's line and both times
  (`START OVER` only in Continue watching, where `S` plays it from the
  start), or UP NEXT with its length and no bar. One watched has a
  `✓ Watched` chip; a series once its last episode is. A movie
  downloaded, or queued or downloading, has a chip after it (VOD ticket
  10): `↓ Downloaded`, `↓ Queued`, `↓ 41 %` while it runs, refreshed each
  second, or `↓ Failed` in amber; one whose file is gone has none. The length is
  the info's `duration`; mpv's own shows on the NOW PLAYING card. It
  takes the space LATER takes for a channel and scrolls. The panel's
  RECENTLY PLAYED is left out: it is the last 4 channels, for zapping
  back as `-` does, so it stays channels only (settled in VOD ticket
  11).
- **A series' episodes** (VOD ticket 05), in the detail panel below the
  title: a meta line of its years, how many seasons the provider has, the
  genre and the rating (`2014 – 2016 · 3 seasons · Crime, Drama · ★
  8.0`), broken only between parts; the plot, at most four lines; season
  pills, one per `episodes` key (6.1), `Season 1` up to three seasons
  and `S1` past that, scrolling sideways to keep the chosen one in view;
  then the season's episodes, taking the space left: number, name and
  length (`48 min`), 46 px rows. The name is what follows the title's
  `S01E02` mark, or the whole title without one, or `Episode 2` when
  nothing follows. The info is asked for on selection as a movie's is
  (6.2); until it is in, `Loading episodes…`; a series with none says
  `The provider lists no episodes.`, one whose info failed says so and
  that selecting it again retries. Enter or a double click on the
  series moves keys into its episodes (the status bar says why not while
  they load or when there are none): the episode selected has the
  accent border, the list's row loses its border, and the status bar
  hints `j k episode   h l season   Enter play   Esc back`. A new
  season selects its first episode. A series opens on its next episode
  (VOD ticket 07), else its first season's first. The season and episode
  are kept while the series stays selected. Episodes watched read
  `24 min · ✓ Watched`; one part way has a bar. After that, an episode's
  download as a movie's chip says it (`↓ Downloaded`). In Continue watching,
  Enter or a double click on a series plays its next episode, resuming
  one part way; `S` plays it from the start. Until its episodes are
  read, or with none to carry on with, they go into its episodes as
  elsewhere.
- **A movie playing** (VOD ticket 04): the NOW PLAYING card has its
  poster tile, title, and year and rating; its position and length from
  mpv (`0:12:34 / 1:33:14`, `time-pos` asked every second and `duration`
  until known), time left on the right (`1 h 21 min left`) and a bar
  once the length is known. The heading is `NOW PLAYING` or `PAUSED`,
  never behind live. Go live and record are not on it; `L` and `R` say
  so in the status bar (`Go live is for channels`, `Recording is for
  channels`), as they do from mpv (9.2), and the hints leave out
  `R record`. Enter plays a movie from where it stopped (VOD ticket 07,
  5); `S` on a movie or episode plays it from the start, and `S` while
  one plays (in a channel list, or in mpv) seeks it back to 0, so the
  card has `START OVER` at its heading's right, as a channel's does. A movie takes a connection
  as a channel does (9.5). It is added to Recently played (VOD ticket
  11), and an episode adds its series. An episode plays the same way (VOD
  ticket 05): the card's name is the series and its line `S2 E2 · The
  Night Ferry`. When an episode plays to its end (mpv's `end-file`
  `eof`, for the latest `loadfile` only, 9.1), the next one plays after
  a 10 s countdown (VOD ticket 08): the next in the season, then the
  first of the next season; none after the last, where the status bar
  says `That was the last episode of Glass Kings`. The status bar counts
  down (`Next episode in 7 s: S2 E3 · Low Tide. Esc cancels`) and so
  does mpv's OSD, with `PgDn plays it now · Backspace cancels`. Esc in
  the app, Backspace in mpv, playing anything else or closing mpv
  cancels it. The next episode resumes if it was stopped part way. When
  its series' panel is shown, its season and episode are selected, as
  the list follows the channel playing.
- **Search mode: focus-list style.** `/` turns the middle column into a large
  search field with wide result rows; Enter keeps the filter and returns to
  the three-pane view, Esc clears it.
- **Guide grid: guide-strip style.** Not v1: the guide grid milestone
  (`docs/milestones/guide-grid.md`), done. `t` replaces the three
  panes with it and `t` again returns; the status bar stays, and Esc does
  not leave. Rows are the current category's channels with a guide (any
  programme in the kept window, 7.1), in the library's order. Each row is
  a fixed column (number, star, 32 px logo, name) and the programmes
  airing in a 3 h span, the one on now tinted. A programme the next
  overlaps (6.1) is cut where that one starts. Above the rows a time header
  with half-hour ticks, and a line at now across both; neither scrolls
  vertically. The header names the category, with `115 channels with a
  guide` on the right. `[` and `]`, or Shift-Tab and Tab, step through the
  library's categories as the sidebar lists them (hidden ones left out),
  stopping at the ends; the grid starts at its top, at the same time, and
  the main view follows (ticket 03). The selection is a programme on a
  channel (ticket 02), outlined in accent. `t` opens the grid on the main
  view's category and selected channel, or, if that one has no guide, the
  first guide channel after it (the last before it at the end), at now,
  the span from the current half hour. `t` back puts the main view on the
  grid's category and selected channel, dropping a kept search filter
  that would hide it. Plays and favourites from the grid show in the main
  view as they happen. `/` searches the grid's channels (ticket 04, 8.5):
  a field in the header, accent while typing, with "Enter keeps the
  filter · Esc clears it" beside it; the count reads `3 of 115 channels
  with a guide`. The grid's filter is its own, not the list's: time moves
  keep it, and a category switch or leaving the grid drops it. Esc clears
  it, the selection staying on its channel. A gap in a channel's guide is selectable too, as
  "No information", so every channel has a cell at every time. `h`/`l`
  step to the previous or next cell on the channel, and one shared span
  moves every row and the header only as far as needed to show it: back
  to its start, or on until its end shows. `j`/`k` keep to the selected
  time: now while the selection airs, otherwise the start of the cell
  last chosen with `h`/`l` or a jump, so moving down a column does not
  drift. `n`, `<`, `>` and `P` jump (8.3); time stops at the kept window, back as
  far as the longest archive among the category's channels (1 h if none
  has one) to 3 days ahead (7.1, catch-up ticket 03). Days a channel's
  stored guide does not reach are gaps, cut at every 3 h so `h`/`l` step
  through them a span at a time. On a channel with an archive they fill
  in from its guide table, fetched as the rows on screen reach before
  what is stored, the selected row first (6.1, catch-up ticket 06). Above the grid a detail strip shows the
  selection: the channel's number and name, the title, the times (with
  `Tomorrow`, `Yesterday` or `Sat 3 Oct` unless today) and either time
  left (in accent), `starts in 2 h 5 min` or `ended 20 min ago`, then up
  to three lines of description; a gap shows the channel and "No
  information" only. `R` schedules the selected programme for recording
  (9.5), or cancels it if scheduled or recording; one airing now starts
  recording at once, to its padded stop. On a programme that has ended,
  or a gap, the status bar says why and nothing is scheduled (recording
  ticket 03). A scheduled programme's block has a red ring at its top
  right, a recording one a filled red dot, and the detail strip adds
  `○ Scheduled to record 19:58 – 21:05` or `● Recording 19:58 – 21:05`
  in red after the times, the padded times. The minute tick moves the line and the tint, a span
  on the current half hour moves on with it, and a selection on now
  follows the programme airing. **Catch-up** (catch-up ticket 03): a
  programme that has ended, on a channel with an archive, within its
  `archive_days` of now (or that its table marks `has_archive`, 5), has
  an archive mark (a clock with a back arrow,
  in accent) at its top right, and the detail strip adds `CATCH-UP` in
  accent after the times; other past programmes have dimmed titles. Enter on a marked one plays it from the archive
  (6.1); on another past one, or a past gap, nothing plays and the status
  bar says why (`No archive on this channel`, `Older than this channel's
  3-day archive`, `Not in this channel's archive` where the table says
  so, `Nothing to play: the guide has no programme here`).
  Enter on now or later plays the channel live. **Start over** (catch-up
  ticket 04): on the programme airing now, on a channel with an archive,
  the detail strip adds `START OVER` in accent, and `S` plays it from
  the archive from its start, as catch-up (6.1); on a channel without one,
  on a gap airing now, or on a programme not airing, the status bar says
  why (`No archive on this channel`, `Nothing to start over: the guide has
  no programme airing`, `Start over is for the programme airing now`).
  With no guide channels: "No channel in All has a guide."
  Mouse: 8.4. In the grid the status
  bar's hints start `t list` instead of `/ search  t guide`, and while
  something plays it shows the playing channel compactly, as in search
  mode.
- **Account dashboard** (ticket 21): `i`, or a click on the status bar's
  dot or account, opens a 480 px card over everything, closed by Esc, `i`, `q`,
  the × button or a click outside. Top to bottom: the provider's name, with
  `Checked 14:05` (or `Checking…` until its check answers) and a status
  chip, green while the account can refresh, red (`danger`) once expired or
  disabled; the provider's message in an accent box, if any; SUBSCRIPTION
  (expires, with `in 160 days`, created, trial, connections as `1 of 2 in
  use`, stream formats and the one used); SERVER (timezone, server time as
  the server writes it, ports, protocol), only rows the provider sent;
  LOGIN: server URL, username and password from the credentials file,
  masked. A click on one, or `1`, `2`, `3`, shows or masks that one field;
  every field is masked again when the dashboard closes, so it always opens
  masked. Masked values are not in the window at all, so a snapshot shows
  only what was revealed.
- **What an M3U provider lacks** (M3U ticket 04) is hidden, not shown
  disabled: the UI asks the provider shown what it has (`archive`, `vod`,
  `account`, from its source; an Xtream provider has all three, an M3U
  one none). Without `vod` the sidebar has no Continue watching and no
  kind switch (`h`/`l` and `[`/`]` pass over where Continue watching
  sits), and manage mode has no kind switch. Without `account` the
  status bar shows no expiry or its colours, and neither `i` nor a click
  on the dot opens the dashboard. Without `archive` the grid has no
  archive marks and no dimmed past programmes, the detail strip and NOW
  PLAYING have no `CATCH-UP` or `START OVER`, and no guide tables are
  asked for. The keys panel drops the keys that would lead there (8.3).
  Switching providers redraws all of it at once.
- **Recordings view** (recording ticket 04): `v` opens a card over
  everything, up to 680 by 640 px, closed by Esc, `v`, `q`, the × button
  or a click outside; other keys do nothing while it is open. Its header
  is a Recordings · Downloads switch (VOD ticket 10; Tab and Shift-Tab,
  or a click), the hint line below it; it opens on the tab last shown
  while the app runs, Recordings at first. The Recordings tab lists
  every recording (9.4 and 9.5 alike, all providers) in three groups,
  each left out while empty: NOW RECORDING, by start; SCHEDULED, soonest
  first; HISTORY, latest first. A row is the programme (the channel
  without a guide), then the channel and the guide's times (`Today 20:00
  – 21:00`, `Tomorrow`, `Yesterday` or `Sat 3 Oct`; a recording of what
  plays reads `Today from 20:04` until it ends), and the note if any
  (why it failed, `cut short: …`, or for a missed one `the app was not
  running`), amber for failed and missed. On the right its state: `●
  Recording` in red, `Scheduled`, the file's size (`1.2 GB`, or `File
  missing` in amber), `Failed` or `Missed` in amber, `Cancelled` (with
  its size if it kept a file) dimmed. `j`/`k`, arrows, `g`/`G` move over
  the recordings, skipping the headings. `x` on a scheduled or running
  one arms its cancel: the status bar says what a second press does and
  the row reads `Press x again to cancel` in amber; a second `x` within
  4 s cancels (9.5; one of what plays is stopped as `R` stops it, and
  kept as cancelled), any other key or the 4 s passing disarms. Enter (or
  a double click) on a finished one plays its file in mpv: no channel,
  so no NOW PLAYING card and no connection counted. `d` on one in
  HISTORY (recording ticket 05) arms its delete: the row reads `Press d
  again to delete` in amber, the status bar `Press d again to move <file
  name> (1.2 GB) to the trash`; a second `d` within 4 s moves the file to
  the home trash (`$XDG_DATA_HOME/Trash`, freedesktop.org trash spec,
  with its `.trashinfo`, so the desktop can restore it) and the recorder
  marks the row trashed: `Moved <file name> to the trash`. A trashed row
  is hidden while its file is gone, and shows again once the file is
  restored to where it was (on the next re-read: reopening the view, or
  a recorder event). Clear forgets trashed rows too, counting only the
  shown ones, and not their files as kept. Any other key or the
  4 s passing disarms. Nothing is unlinked outright. A row with no file,
  or whose file is gone, is only forgotten, saying so. Refused, with a
  status bar message and nothing changed: scheduled and running ones (`x`
  cancels those), and a file outside `recordings_dir` (the setting
  changed since). A failed move (a file on another filesystem than the
  trash, as per-volume trashes are not supported, or permissions) leaves
  file and row as they were, the error in amber. `o` opens the
  selected recording's folder (`recordings_dir` for one with no file)
  with `xdg-open`. While HISTORY has entries the header has a Clear
  button; `c` (or a click) arms it, the button turning amber and
  the status bar saying what a second press does, and a second within 4 s
  forgets every done, failed, missed and cancelled recording, keeping
  their files (the status bar says how many, and that files are kept
  only if some had one: `Cleared 1 recording from the list`, `…; its
  file is kept`, `…; the 2 files are kept`); any other key or the 4 s passing disarms. Each
  recorder event re-reads it, so it follows starts,
  ends and cancels while open, the selection staying on its recording.
  Empty, it reads "Nothing recorded yet." and, below, "Press R while
  watching to record it, or on a programme in the guide to schedule it."
- **Downloads tab** (VOD ticket 10, 9.6): every download, all providers,
  in three groups, each left out while empty: DOWNLOADING; QUEUED, in
  queue order; FINISHED, done and failed, latest first. A row is the
  movie or series, then `Movie` or the episode (`S2 E4 · The Night
  Ferry`), then a note: how far (`48 MB of 94 MB`) while it runs or for
  a queued one with a part, `waiting for a free connection` on the
  queue's first while none runs (6.3), why a failed one failed. On the
  right `↓ 50 %` in accent while it runs, with a bar under the row,
  `Queued`, the file's size (`File missing` in amber), or `Failed` in
  amber. Each second while one runs the tab is redrawn. Keys as on the
  Recordings tab: `x` twice cancels a queued or running one, which
  forgets it and removes its part; `d` twice on a done one moves its file
  to the trash (refused for a file outside `downloads_dir`), the row
  hidden while the file is gone and shown again once it is restored, as
  a recording's (the tab on its next re-read, the detail panel's mark
  when the title is selected again or on the minute tick); on a failed one, or one whose file is gone, it forgets
  it and removes any part. Without the file the title is not downloaded:
  no chip, and Enter streams it;
  Enter (or a double click) plays a done one's file in mpv, as a
  recording's; `o` opens its folder (`downloads_dir` for one with none);
  `c` twice, or Clear, forgets the done and failed ones, keeping files
  (`Cleared 2 downloads from the list; their files are kept`), and
  trashed ones, counting only the shown ones. Empty, it
  reads "Nothing downloaded yet." and "Press d on a movie or episode to
  download it, or D for a whole season."
- **Settings view** (settings ticket 01): `,` (also in the guide grid),
  or a click on the gear at the status bar's right end, opens a card over
  everything, up to 760 by 560 px, closed by Esc, `,`, `q`, the × button
  or a click outside; other keys do nothing while it is open. Under
  `Settings`, a hint line for the keys. A managed `config.toml` (4.2)
  puts its note in an amber box under the header. Left, the sections:
  Providers, General, Player, Recording, Downloads, EPG (Providers
  first since 2026-10-12: nothing works without one; the view opens
  on it); right, the
  selected one's fields, each its label, then its key in `config.toml`
  and what it does in dim type, the control on the right. Tab moves
  keys between sections and fields (Enter on a section too), `j`/`k`
  and arrows move within them. Fields that outgrow the card scroll,
  by the wheel too, the selected one kept in view (EPG sources ticket
  04). General: Start with (`active_provider`),
  a picker of `Last used` and each provider by name, used from the next
  start; Refresh when older than (`refresh_hours`), a `12 h` / `6 h`
  switch; Theme (`theme`), a picker of `Default`, `Noctalia`, the
  built-ins by name, then the user themes by file stem in name order,
  read when Settings opens, with why the chosen palette is not in use
  under it (8.1). A saved name no longer there stays in the list, last,
  so the field never changes it unasked. Enter on the
  switch flips it; on the picker it opens its list under the field,
  the saved entry ticked, where `j`/`k` move, Enter chooses and Esc
  cancels. Each choice saves at once, no Save button. A
  save that fails says why under its field in amber, nothing written.
  Player (settings ticket 02): mpv (`mpv_path`) and Extra mpv arguments
  (`extra_mpv_args`, shown as its TOML array). Recording: Recordings
  folder (`recordings_dir`), ffmpeg (`ffmpeg_path`), Start early by
  (`record_before`) and Run late by (`record_after`), each a stepper
  with − and + either side of `2 min`. Downloads: Downloads folder
  (`downloads_dir`), shown with an M3U provider active too, as it is one
  setting for every provider's movies and series (M3U ticket 06). A text field shows its value on the right; Enter
  opens a box under it holding the value (the arguments as a shell
  line), typed into as in search, Ctrl v pasting (one line,
  newlines as spaces) and Ctrl u clearing it. Enter checks and
  saves it (4.2), closing the box; a refusal shows under the box, which
  stays open, and for a missing folder a `Create it` button beside it,
  which Enter again also does. Esc cancels. `h`/`l` and the arrows step
  a stepper, each step saved. The hint line says the keys for what is
  selected. Providers (onboarding ticket 01): each provider by name, its
  id and last account check under it (`demo · Expires 21 Mar 2027 · 2
  connections`; an M3U one's `lists · M3U · lists.example.net`,
  M3U ticket 06), then `Add provider…`, whose Enter opens the form in the
  section's place: a `Type` switch, `Xtream` / `M3U`, whose
  Enter flips it and clears the URL; for Xtream Name, Server URL,
  Username and Password, for M3U Name, Playlist and User agent, text
  fields as above whose Enter keeps the value rather than saving it,
  then `Check and save` (`Fetching the playlist…` for M3U). The playlist
  URL's query shows as `?•••` with a `Show` / `Hide` button, as Settings
  › EPG's URLs do; Ctrl r does the same. For a panel's `get.php` link,
  `Add as Xtream instead` sits before `Check and save`, its Enter
  filling the Xtream form and selecting `Check and save` (4.1). The
  password shows as dots, with a `Show` / `Hide` button; Ctrl r does
  the same. Enter on `Check and save` says what is
  missing, if anything, else checks (`Checking the account…`) and saves
  (4.1), closing the form on the new provider with `Added <name>: p
  switches to it` in the status bar; a refusal or warning shows under
  it, a warning with `Save anyway`, which Enter again also does. Esc
  leaves the form. Enter on a provider (onboarding ticket 02) opens the
  same form filled in without the `Type` switch, the Name note saying
  its id never changes,
  `Save changes` in place of `Check and save`, then `Remove provider…`.
  `Save changes` checks only when the URL or login changed (4.1), and
  closes the form on the provider with `Saved <name>`. `Remove
  provider…` replaces the form with its confirm, on its Remove row:
  `Also delete favourites, progress and history`, a `Keep` / `Delete`
  switch with that data's counts under it, then `Remove <name>` with
  the channels, movies and series deleted, the scheduled recordings
  cancelled and the queued downloads forgotten. Enter twice removes,
  keeping the personal data; the status bar says `Removed <name>`. Esc
  goes from the confirm back to the form, and from the form to the
  provider's row. A note too long for one line wraps. A managed
  `credentials.toml` puts its note in the amber box while Providers is
  selected, and nothing opens; while EPG is selected too.
  EPG (EPG sources ticket 04): the shown provider's guide sources in
  their order (4.1), the provider's own guide among them under the
  provider's name. Each row says, under its name, where it comes from (a
  URL with its query string and any password as `•••`, Ctrl r showing
  them whole; a path; `The provider's own guide (xmltv.php)`), then
  `matched 1,200 · ambiguous 40 · none 56,000` (for the provider's guide,
  the channels with programmes under their own guide id, and none), then
  a URL's requests in the last 24 h (`3/20 in 24 h` with
  `max_per_day`) and the last result: `imported 14:05`, `importing…`
  while a refresh does, `busy, asked again after 15:10`, `not fetched
  yet`, `off: skipped, its data kept` with `Off` on the right, or
  `failing` with `Last fetch failed: <reason>` in amber under it, and
  `re-import at the next refresh` after a manual mapping marked it (EPG
  sources ticket 05). `J` and
  `K` move the selected source down and up, saving at once. After the
  sources, while one just added is below the provider's guide, `Move
  <name> above <provider>'s guide`, whose Enter does that; then `Add
  source…`. Enter on a source, or on `Add source…`, opens its form in
  the section's place: for a saved one an `On` / `Off` switch, then URL
  or path, Name, Shift (minutes), Fetch every (hours) and At most per day,
  text fields as in the provider form whose Enter keeps the value once
  checked (a URL's syntax or a path's readability, never fetched; whole
  numbers, at least 1 and 3) and a switch saved with them, then `Add
  and import` or `Save changes`,
  then for a saved one with ambiguous channels `Ambiguous channels…`
  (`40 channels matched several of its channels, so none`), whose Enter
  opens, over Settings, `Ambiguous in <source>`: those channels by name,
  filtered as typed, each Enter opening the mapping picker on it (EPG
  sources ticket 05), Esc going back; then for a saved one `Remove
  source…`. The provider's guide's form has
  its switch and Shift only; switching it off says under the switch
  `Provider guide off: 312 of your favourites have no other source` when
  any live favourite has its guide from it and from no enabled external
  source. Save refuses a source already in the list (`Already a source:
  <name>`) and a limit on a path. Adding saves it last, closes the form on
  its row with the move offer under it, and the status bar says `Added
  <name>: importing it`; a failed fetch leaves it saved, marked failing.
  `Remove source…` replaces the form with its confirm, on its Remove
  row: with manual mappings to it, `Also delete 41 manual mappings`, a
  `Keep` / `Delete` switch (kept, they come back with a source of the
  same address); then `Remove <name>` with what it deletes, how many
  channels go back to the next source and how many pinned channels go
  back to the source order; Enter removes it. Esc goes from the confirm to the form and from the
  form to the list. A shift applies from the source's next fetch.
- **Keys panel** (ticket 24): `?` opens a card over everything listing
  every key, closed by Esc, `?`, `q`, the × button or a click outside;
  other keys do nothing while it is open. Three columns of groups: MOVE,
  CHANNELS and GUIDE; PLAYER, LIBRARY and MOVIES AND SERIES (VOD ticket 07); APP and INSIDE MPV (9.2). Each row is the
  keys as keycaps, then the action. Below, IN OTHER VIEWS has one line each
  for the account dashboard, search, manage mode, the refresh panel
  (refresh panel ticket 01), the provider picker and the Recordings view
  (its tabs, VOD ticket 10); the Settings view has its own hint line.
  F12 shows only with devtools. One list in `src/ui.rs` feeds it, and a test fails if an
  action has no row. In search and manage mode `?` types instead.
- **Status bar** across the bottom of the window, visible in every mode: a
  health dot, the provider and its account (6.3; a click on the provider
  opens the picker, on the dot or account the account dashboard);
  refresh state (7.2); then player and app messages, errors in amber, key
  hints when idle. While something plays the hints are a merged set,
  `/ search  t guide  space pause  R record  s stop  m mute  - last channel  q quit`, with
  `space resume` while paused, `L live` after it while behind live, and
  `R stop rec` while recording; catch-up shows `L live` too;
  otherwise
  `/ search  t guide  f favourite  r refresh  p provider  ? keys  q quit`.
  With Movies or Series chosen, `Tab kind  f favourite  a order  e
  categories  r refresh  p provider  ? keys  q quit`, and while playing `Tab kind` in
  place of `/ search  t guide`. In search mode,
  when the panel is hidden, the right side shows the playing channel
  compactly (`▶ name`, `❚❚` when paused, `· catch-up` while catch-up
  plays, `· muted`). While recording, a red
  `● REC 0:12` at its right end, as on the NOW PLAYING card; starting says
  `Recording to <file>`, stopping `Recorded to <file>`, and a failure
  `Recording stopped: <mpv's reason>` in amber. When a live channel drops
  (9.3), `RECONNECTING` while the app plays it again, cleared once it
  plays or the user plays or controls something else; giving up says
  `Can't play <channel>: <reason>` in amber, as for any stream that fails
  (stream recovery ticket 02). While a refresh runs a thin accent line sweeps along its
  top edge; it exists only then, so an idle window does not redraw. A
  click on the line, or on the refresh state, opens the refresh panel.
  A gear at its right end opens the Settings view.
- **Refresh panel** (refresh panel ticket 01): a 300 px card at the
  bottom right, above the status bar and left of the detail panel (so
  Recently played stays in view), over the screen without blocking
  it: keys still reach the screen, and clicks off the card what is under
  it. `Refresh` and a ×,
  then one row per stage the refresh runs, in run order: `Channels ✓
  58k`, `EPG ▸ 41 %` (or `▸ 34 MB`), `Movies waiting`, and a backoff as
  `provider busy, retry in 0:42`, counting down; under EPG, a smaller,
  indented row per guide source taking a turn (7.2, EPG sources ticket
  04): its progress, `✓ 1.2k`, `✓ unchanged`, `at its cap` with `20/20
  in 24 h, next 14:05` under it, `busy until 15:10`, or failed with the
  reason; while one marked by a manual mapping waits or runs,
  `re-importing for new mappings` under it (EPG sources ticket 05). A failed stage reads
  `failed` with the reason under it in amber (7.2). It opens for `r`, for
  a click on the sweep line or refresh state, and for a refresh that
  fails or backs off; a background one (launch, the 15-minute check)
  stays on the sweep line. It closes by itself 4 s after a success, as
  the `c` arm lapses (8.3); a failure stays until dismissed. Esc, before
  anything else Esc does, or its × dismisses it; the refresh carries
  on. It shows the provider shown, and keeps its rows from the last
  refresh until the next starts.
- **First-run screen** (refresh panel ticket 02): for a provider whose
  channels were never imported (7.2), the refresh panel's rows
  full-window, in large type on a card, in place of the main screen and
  under the app logo (`assets/vidicon.svg`) and `Loading <provider>`;
  before the stages, the account check's state.
  The status bar stays. Once the channels stage is done, the main screen
  shows and the refresh panel opens on the stages left, closing as for
  `r`. A failure before then stays full-window, with the failed row's
  reason and `Press r to retry.`; the refresh panel does not open while
  the screen shows.
- **Setup screen** (onboarding ticket 03): when `credentials.toml` gives
  no provider (4.1), the Settings card over everything on an opaque
  background, under the app logo as on the first-run screen and only as
  tall as what it holds, with no sections, no × and no status bar; `q` quits, and
  Esc, `,`, Tab or a click outside do nothing. Its title says why, and
  for a refusal its text too, a path under the home folder written
  `~/…`. No file or no provider (`Add a provider`, no text): the
  add-provider form of
  Settings › Providers (8.2), either type (M3U ticket 06), whose
  `Check and save` saves as there,
  then shows the main screen with `Added <name>`, the first-run screen
  over it until the channels land. Modes too open (`Credentials open to
  others`): the refusal with its `chmod`, `Nothing is read from it until
  this is fixed.`, then `Fix permissions` when offered (4.1) and `Check
  again`. Not valid (`Credentials not valid`): the parse error with its
  line and the expected format, then `Check again`. `Check again` reads
  the file afresh; a file still failing the same way says `Still the
  same: change the file, then check again` under it. A file that now
  names providers starts them all, as at launch, on the provider 4.2
  picks. A managed file shows its note and `Check again` in place of
  the form. Removing the last provider (4.1) stops anything playing and
  shows this screen.

### 8.3 Keys

Keyboard first; every action has a key. Bindings are fixed in v1 and
configurable later.

| Key | Action |
|---|---|
| `j` / `k`, Down / Up | Move selection |
| `Ctrl-d` / `Ctrl-u` | Half page |
| PageDown / PageUp | Full page |
| `g` / `G`, Home / End | Top / bottom |
| `h` / `l`, Left / Right | Previous / next category; in the guide grid, previous / next programme (8.2) |
| Tab / Shift-Tab | Next / previous kind in the library: Live, Movies, Series, wrapping (VOD ticket 03). In the guide grid, next / previous category |
| `/` | Search; typing filters live, Tab widens to all kinds and back (8.5), Enter keeps the filter, Esc clears. In the guide grid, its own filter (8.2) |
| Enter | Play selected channel or movie (VOD ticket 04), a movie or episode from where it stopped (VOD ticket 07); on a series, into its episodes, and there play the selected one (VOD ticket 05, 8.2), or in Continue watching play its next episode (VOD ticket 07); in the guide grid, a past programme the archive keeps from its start (8.2) |
| `f` | Toggle favourite: the channel, movie or series selected (in a series' episodes, the series) |
| `J` / `K` | Move favourite down / up (in Favourites only, across kinds) |
| `r` | Refresh now, and open the refresh panel (8.2); while one runs, the panel alone, as the refresh absorbs it (7.2) |
| `p` | Switch provider: a picker of the providers, with one provider too, each with its type (`Xtream` or `M3U`) on the right (M3U ticket 06), its last row `Add provider…`, which opens Settings › Providers on the form (onboarding ticket 01) |
| Space | Pause / resume the Player (not while searching: it types) |
| `s` | Stop: closes the Player's window |
| `m` | Mute / unmute |
| `F` | Fullscreen on / off |
| `L` | Go live: reload the channel playing at the live edge, when behind live (ticket 14); while catch-up plays, the same channel live, and in the guide grid a selection on that channel goes back to now, as `n` does. Either way the status bar says `LIVE` |
| `S` | Start over: the programme airing now on the channel playing, from its start in the archive, as catch-up (catch-up ticket 04); a movie or episode playing, from its start (VOD ticket 07). Nothing while mpv is closed; without an archive or a programme airing in the guide, the status bar says why. On a selected movie or episode, play it from the start; on a series in Continue watching, its next episode from the start (elsewhere the status bar says to choose an episode). In the guide grid: the selected programme, airing now (8.2) |
| `R` | Record what is playing, or stop recording it (9.4). Nothing while mpv is closed; while catch-up plays, the status bar says recording it is not supported yet. In the guide grid: record the selected programme, or cancel it (8.2, 9.5) |
| `-` | Last channel: play the one before the current (with the Player closed, the one played last; after a clear there is none, and the status bar says so). Channels only: movies and series in Recently played are passed over |
| `o` | The selected channel's alternates, opened in the detail panel (8.2, stream recovery ticket 03); without any, the status bar says so, and on a movie or series that it is for channels. In the guide grid it does nothing |
| `a` | Order: provider or A–Z (8.2) |
| `e` | Manage mode: which categories and channels show (8.2) |
| `E` | Guide card: where the selected live channel's guide comes from, its pin and mappings (8.2, EPG sources ticket 05); in the list or the guide grid |
| `?` | Keys panel (8.2) |
| `t` | Guide grid, or back to the list (8.2); live only, so with Movies or Series chosen the status bar says so. In the grid `j`/`k`, arrows, page keys and `g`/`G` move between channels at the selected time; Enter, `/`, Esc, `f`, `r`, `p`, `i`, `?`, `q`, `-` and the Player keys work as in the list; `a`, `e`, `c` and `J`/`K` do nothing there |
| `n` | In the guide grid: back to now |
| `<` / `>` | In the guide grid: a day back / ahead, the span moving by the day |
| `P` | In the guide grid: prime time, 20:00 today, or tomorrow once past |
| `[` / `]`, Shift-Tab / Tab | In the guide grid: previous / next category (8.2) |
| `v` | Recordings view (8.2); inside it Tab and Shift-Tab switch between Recordings and Downloads, `x` twice cancels, Enter plays a finished one, `d` twice moves a finished one's file to the trash, `o` opens its folder, `c` twice clears the finished ones from the list (files kept), Esc, `v` or `q` closes |
| `d` | Download the selected movie (VOD ticket 10, 9.6); in a series' episodes, the selected episode. On a series in the list the status bar says to choose an episode, or `D`; on a channel, that it is for movies and episodes. In the guide grid it does nothing |
| `D` | Download every episode of the season a series' panel shows; elsewhere the status bar says it is for a series' season. In the guide grid it does nothing |
| `i` | Account dashboard (8.2); inside it `1`–`3` show or mask the server, username and password, Esc, `i` or `q` closes |
| `,` | Settings view (8.2, settings ticket 01); inside it Tab moves between sections and fields, `j`/`k` and arrows within them, Enter edits a field (a switch flips and saves, a picker opens, a text field takes typing and Enter again saves it), `h`/`l` and arrows step a stepper, Esc cancels an edit or closes, as `,` and `q` do; in EPG `J`/`K` move a guide source and Ctrl r shows URLs whole (EPG sources ticket 04) |
| `w` | Watched or not (VOD ticket 07): the selected movie; on a series in the list, every episode of the season its panel shows; in its episodes, the selected one. A season partly watched becomes all watched, only one all watched goes back to unwatched. Unwatching drops the resume point too. The status bar says what changed (`Glass Kings season 1: watched`). In the guide grid it does nothing; on a channel the status bar says it is for movies and episodes |
| `c` | Clear recently played (in Recently played only), or Continue watching (in it, VOD ticket 07). The first press arms it: the status bar says what a second press does and the Clear button turns amber. A second press (or click) within 4 s clears; any other key, or the 4 s passing, disarms |
| `q` | Quit (closes mpv) |

In a series' episodes (VOD ticket 05, 8.2) `j`/`k`, arrows, page keys
and `g`/`G` move between the season's episodes, `h`/`l` and Left/Right
change season (stopping at the ends), Enter plays (resuming, VOD ticket
07), `S` plays from the start, `w` marks the selected one watched or
not, `d` downloads it and `D` the season (VOD ticket 10), and Esc goes
back to the list. The Player's keys, `-`, `r`, `i`, `v`, `?` and `q` work as
anywhere; any other key goes back to the list and acts there.

On a provider without movies and series, catch-up or an account (8.2,
M3U ticket 04) the keys for them do nothing, without a message: Tab and
Shift-Tab outside the grid and in manage mode, Tab in search, `w`, `d`,
`D`, `S` on a channel, Enter on a past programme in the grid, and `i`.
The keys panel leaves them out, and its grid Enter reads `Play live`.

In a channel's alternates (stream recovery ticket 03, 8.2) `j`/`k`,
arrows and `g`/`G` move, Enter plays the selected one, and Esc or `o`
closes them. The Player's keys, `-`, `r`, `i`, `v`, `?` and `q` work as
anywhere; any other key closes them and acts in the list.

In manage mode every printable key types into the filter and Backspace
deletes, as in search mode. Arrows, Page keys, Home/End and Ctrl-d/u move.
Tab and Shift-Tab switch kind (Live, Movies, Series), keeping the filter.
Enter shows or hides the selected category with an override; toggled back
to what the rules say, the override is dropped. Ctrl+Enter adds the filter
as a rule, or removes it if that rule exists already. Ctrl+H does the
same with a channel rule (ticket 25). Ctrl+K keeps the filter as a region
tag, or stops keeping it (ticket 26); it does nothing unless the filter is
2 or 3 letters, with or without a trailing `:` or `|`. Ctrl+H and Ctrl+K
do nothing on Movies or Series. Esc leaves. Mouse: a
click selects, a click on the pill or a double click toggles.

### 8.4 Mouse

Click selects, double-click plays, wheel scrolls, lists also pan with
click-drag, categories are clickable, and so is each kind on the
library's switch. A click on the status bar's sweep line or refresh
state opens the refresh panel, and its × closes it (8.2). The status
bar's gear opens the Settings view; there a click on a section selects
it, a click on a field selects it (and opens a picker), a click on a
switch's side, a picker's entry or a stepper's − or + saves it, a click
on `Create it` makes the folder and saves it, one on `Show` or `Hide`
shows or masks the password (or a guide source's URL), one on `Save anyway` saves the provider
despite the check's warning, a guide source in Settings › EPG dragged
8 px or more moves to the row it is dropped on (EPG sources ticket 04),
and a click outside closes
the view. In the guide card, the picker and the ambiguous list (EPG
sources ticket 05) a click selects a row, a double click does what
Enter does, `Pin` / `Unpin` pins or unpins, and a click outside does
what Esc does. Mouse and keyboard share one selection. In a series'
panel (VOD ticket 05) a click on a season pill shows that season, a
click on an episode selects it and moves keys into the episodes, a
double click plays it, and a click in the list moves keys back there.

In the guide grid (ticket 05): a click selects the programme under it (on
the channel column, the channel at the selected time); a double click
does what Enter does: its channel live, or a past programme from the
archive (8.2). The wheel scrolls the channels; Shift+wheel and a
touchpad's horizontal swipe move the time. A drag locks to the direction
it starts in once it has moved 8 px, vertical on a tie: vertical pans the
channels, without the lists' fling (the grid handles the drag itself, so
the `ListView` cannot take it over), horizontal moves the time, the
header with it. Time moves by
the second, not by half hours, and stops at the kept window's ends; the
selection stays where it was until a key moves it. The ‹ and › beside
the category name in the header switch category, as `[` and `]` do.

### 8.5 Search

Fuzzy match (nucleo) over names within the current category, best match
first; in the guide grid, over the category's guide channels only (8.2).
`/` in a Live, Movies or Series category searches that kind only; in
Favourites, Recently played and Continue watching it searches what they
hold, each title against its own kind's names. The search bar names the
scope (`Series`, `Favourites`). Tab in the search bar widens it to the
All of every kind and back: results are grouped by kind, live first, then
movies, then series, each group its best 50, and the line under the bar
counts each (`12 channels · 50 of 312 movies · 3 series`). A title row
then says its kind (8.2, list rows). Enter keeps a widened filter as it keeps any other
(VOD ticket 09). On a provider without movies and series (M3U ticket 04)
Tab widens to All channels instead, every match best first, and the
bar names that scope. Search filters the view's own keys, so in All it only finds
channels of shown categories, and nowhere channels a channel rule or kept
tags hide (5); the index keeps every name. At 50k channels an in-memory match over names takes 4 to 5 ms
(prototype benchmark), so v1 keeps names in memory for matching (about a few
MB) and reads everything else from SQLite by page. If memory becomes a
concern, use `channel_fts` as a prefilter.

**Measured, 2026-09-29 (ticket 06), release build, about 58k real names:**

- Names are NFKC-folded before matching (so `uhd` finds `ᵁᴴᴰ`); ties keep
  list order. Matching is split across up to 8 threads for views of 4,096
  channels or more.
- 3 to 6 ms per keystroke, occasional outliers to 14 ms. Single-threaded it
  was 5 to 13 ms.
- The index takes about 110 ms to build, so it is built off the UI thread at
  launch and after each channel import. It reads on a read-only connection
  that skips migrations, so the EPG import that follows a channel import
  does not hold it up (ticket 23). A query typed before it is ready
  shows "Indexing channel names…".
- In search mode the sidebar and detail panel fold away and the list is a
  centred column (prototype layout B). Arrows, Page keys, Home/End and
  Ctrl-d/u navigate; Left/Right change category; every printable key types.
- In the guide grid the same keys navigate, Left/Right step programmes, and
  every printable key types, so `t`, `n`, `[` and the other grid keys do
  not act. 2 to 5 ms per keystroke over 30k guide channels (ticket 04,
  `docs/perf/guide-grid.md`).

**Measured, 2026-10-03 (VOD ticket 09), release build, the user's
provider: about 58k channel, 183k movie and 49k series names.**

- One index per kind, built off the UI thread at launch and after an
  import of that kind (live about 150 ms, series 160 ms, movies 430 ms).
  Each holds its names end to end, by row key, with a 64-bit set of the
  ASCII letters and digits each holds. A name lacking a letter the query
  needs is ruled out by that set before nucleo sees it, which changes no
  result. Memory: about 12 MB more resident than with channel names only.
- Movies' All (183k): 5 to 16 ms per keystroke, rare outliers to 18 ms
  on one- to five-letter queries made of common words that match 20k to
  120k titles; 1 to 7 ms once a query is down to a few thousand. Widened
  (about 250k names shown): 6 to 15 ms. Live All: under 3 ms.
- Without the letter sets and the layout (a map of separately allocated
  names), Movies' All took 11 to 28 ms and a widened search up to 30 ms:
  most of a keystroke went on names that do not match, read cold between
  keystrokes. No FTS prefilter was needed.
- Tab reads each kind's All keys, 90 to 120 ms at these sizes, once per
  widening and again only after an import or an order change.

## 9. Player (ADR 0002)

### 9.1 Process

Spawn on first play:

```
mpv --idle=yes --force-window=yes \
    --input-ipc-server=$XDG_RUNTIME_DIR/vidicon/mpv.sock \
    --wayland-app-id=vidicon-mpv \
    '--title=${?media-title:${media-title}}${!media-title:No file} - mpv' \
    --stream-lavf-o-append=reconnect=1 \
    --stream-lavf-o-append=reconnect_streamed=1 \
    --stream-lavf-o-append=reconnect_delay_max=10 \
    <extra_mpv_args>
```

- Play: set `force-media-title` to `<num> <channel name>` (plus ` ·
  <programme>` for catch-up, so the window says what is replayed), for
  a movie its name (VOD ticket 04), for an episode the series, `S2 E4`
  and its name (`Silver Signal · S2 E2 · The Night Ferry`, VOD ticket
  05), then
  `{"command": ["loadfile", "<url>", "replace"]}`, for a movie or episode
  resuming `["loadfile", "<url>", "replace", -1, "start=<secs>"]` (VOD
  ticket 07). The title goes first so the
  URL is never used as the title, not even briefly.
- M3U channels (M3U ticket 03) play their stored `url` (5) with their
  headers as `loadfile` options, so mpv sets them for that file alone
  and they never reach the next one: `user-agent` (the stream's, else
  the provider's `user_agent`, else `vidicon/<version>`), `referrer`,
  and `http-header-fields` with an `Origin:` line. Each value is
  written `%<bytes>%<value>`, which mpv takes as is, and the `Origin`
  line's commas are escaped (`\,`) as it is a list item:
  `["loadfile", "<url>", "replace", -1, "user-agent=%11%Example/1.0,referrer=%18%https://r.example/"]`.
  An Xtream channel's `loadfile` has no header options, so mpv sends
  its own user agent as before. Checked with mpv 0.41 against a local
  server: the headers arrive, commas in the user agent and the origin
  survive, and the next
  plain `loadfile` sends mpv's own agent and no referrer.
- mpv's stdout and stderr are discarded: mpv prints the stream URL there.
- `--title` pins mpv's default title template, so a user `mpv.conf` title
  built from `${path}` or `${filename}` cannot expose the URL.
- The `stream-lavf-o` options are mpv's own reconnect (stream recovery
  ticket 02, 9.3), appended one each so a `stream-lavf-o` in the user's
  `mpv.conf` is kept.
- Closing the app kills mpv. mpv exiting (window closed) means stopped; the next
  play spawns a new one.
- Controls (ticket 12): pause, mute and fullscreen are `cycle` commands; stop
  is `quit`, so the window closes as if the user closed it. The app shows
  mpv's real state by observing `pause`, `mute` and `fullscreen` over IPC
  (`observe_property`), so keys pressed inside mpv are reflected too.
- The user's own `mpv.conf` and `input.conf` apply.

### 9.2 Keys inside mpv

Added over IPC with the `keybind` command at spawn, so the user's `input.conf`
is otherwise untouched. Each sends `script-message vidicon <action>`, which the
app receives as a `client-message` event.

| mpv key | Action |
|---|---|
| PageUp / PageDown | Previous / next channel in the current list; while an episode plays or its countdown runs, the previous / next episode (VOD ticket 08, 8.2) |
| `0`–`9` | Type a channel number (`num`); plays after 1.5 s idle |
| Backspace | Back to the list: the app raises its own window; cancels the countdown to the next episode (8.2) |
| `L` | Go live, as `L` in the app (ticket 14) |
| `R` | Record what is playing, or stop, as `R` in the app (9.4) |
| `S` | Start over what airs now, as `S` in the list (8.3, catch-up ticket 04); a movie or episode from its start (VOD ticket 07) |

Raising the app window: winit's `focus_window` is a no-op on Wayland, but
`request_user_attention` sends an xdg-activation request for the app's own
surface. Umbriel grants it and focuses the window (spike, 2026-09-29). Reached
through Slint's `unstable-winit-030` feature (`WinitWindowAccessor`). Other
compositors may only mark the window urgent; that is acceptable.

Behaviour (ticket 09):

- PageUp/PageDown step through the list as the app shows it (category and
  search filter), from the channel playing, skipping unavailable favourites
  and, in Favourites and Recently played, movies and series.
  They stop at either end rather than wrapping. While an episode plays,
  or has just ended, they step through its series' episodes instead,
  across seasons, resuming one stopped part way; at either end the status
  bar says so (VOD ticket 08).
- A typed number is looked up across the whole provider. If it is not in the
  current list, the app switches to All and drops the filter so its
  selection can follow. A channel in a hidden category (5) plays without
  being selected. An unknown number shows "No channel N".
- The selection follows whatever mpv plays.
- mpv's own bindings for PageUp/PageDown (chapters), Backspace (speed reset),
  `0`–`9` (volume, contrast and so on), `L` (infinite loop, meaningless
  for live), `R` (subtitles down) and `S` (a screenshot without
  subtitles; `s` and Ctrl+s still take one) are replaced; every other key
  keeps the user's binding. Checked against mpv 0.41's `input-bindings`.
- `o` (alternates, 8.3) is not bound inside mpv: mpv's own `o` shows the
  progress bar, and choosing an alternate needs the list in the app.

### 9.3 Errors

Surface mpv `end-file` errors (unreachable stream, auth failure, too many
connections) in the app's status area with the channel name.

Drops (stream recovery ticket 01). `RUST_LOG=vidicon::player=debug`
logs each `end-file` `eof` or `error` of the latest `loadfile`: its kind
(live, catch-up, movie, episode, read from the URL's path; an M3U
channel's is always live, M3U ticket 03), mpv's reason
and `file_error`, the seconds since the `loadfile`, how many of them were
paused, and whether it is paused now or how long since it resumed. No
title or URL.

- **Alternates** (verified against the user's database, 2026-10-06,
  `--probe-alternates`): of about 17.7k channels All shows, about 8 % have
  another shown channel with the same `name_norm`: about 1000 one, 250
  two, 140 three or more. None of the 27 in Recently played has one;
  there are no favourites yet. The query takes about 0.6 s for all of All,
  so one channel's alternates are a single indexed lookup.
  By the looser name form (2026-10-18, stream recovery ticket 03, the
  probe since): about 20 %, about 1900 one, 600 two, 1000 three or more,
  and 8 of the 15 in Recently played. `name_norm` keeps the tag before
  the name, so `4K: Northwind Sport` and `US: Northwind Sport HD` never
  matched; the user found such a pair by hand after a blackout. About
  260 channels are dividers (5), 14 of them sharing a real channel's
  name, so they are left out.
- **mpv on a closed connection** (mpv 0.41, the demo provider with
  `DEMO_DROP`): the providers' streams have no length, so a close is a
  clean end: `end-file` `eof`, not `error`. With
  `--stream-lavf-o=reconnect=1,reconnect_streamed=1,reconnect_delay_max=10`
  mpv reconnects by itself and the app sees no `end-file`, only a
  `playback-restart` (with the demo's timestamps then starting again at
  0; see Reconnecting below); `reconnect_at_eof` is not needed. With nothing to
  reconnect to, the picture freezes (`paused-for-cache`) for about 10 s,
  then mpv reports `eof` as before. A reconnect lands at the live edge, so
  the app's time behind live would be wrong after one.
- **Pause** (verified against the user's provider, 2026-10-06,
  `--probe-pause`, one channel, headless mpv 0.41 without the user's
  `mpv.conf`): paused 1, 5 and 15 min, with and without the reconnect
  options, there was no `end-file` while paused or in the 60 s after
  resuming, and `time-pos` went on from where it paused. mpv keeps
  reading while paused: about 80 s cached after 1 min, 320 s after 5,
  but only about 480 s after 15, so its cache (150 MiB by default) fills
  after about 8 min and mpv stops reading. Whether the provider then
  drops the stalled connection would only show once those 480 s have
  played, which the 60 s watch did not reach. No play was paused in the
  week of normal use below, so a drop after a long pause is still
  unobserved.
- **Drops in normal use** (verified against the user's provider,
  2026-10-06 to 10-11, the debug log above, mpv as the app runs it,
  without the reconnect options): about 170 `loadfile`s. Two live
  channels dropped while playing, unpaused, 13 and 44 min in: each an
  `eof` with no error, and each time the user closed mpv and played the
  channel again by hand within a minute. Two `error` (`loading failed`)
  came 2 s after their `loadfile` while flicking through channels: a
  channel that would not start, which 9.3 already reports, not a drop.
- **Movies, episodes, catch-up:** one movie and one episode played to
  their end (`eof` within 15 s of their length, marked watched); none
  dropped. No catch-up was played, so whether it drops is unknown.

Reconnecting (stream recovery ticket 02). mpv's own reconnect (9.1) comes
first; the app plays a channel again only once mpv gives up.

- **A drop** is `end-file` `eof` for the latest `loadfile` of a live
  channel, or `error` once it has played (a `playback-restart` since the
  `loadfile`) or while the app retries. An `error` before it played is a
  channel that would not start, reported as above. Catch-up's end
  (catch-up ticket 04), movies and episodes are left as they were (the
  milestone's Open). Stop and closing mpv end with `quit`, not a drop.
- **Retries:** two, 3 s after the drop and 10 s after the first fails,
  each a `loadfile` of the same channel at the live edge, unpaused, as
  `L` does, with the same headers for an M3U channel (9.1). Once one plays (its `playback-restart`) the status bar's
  `RECONNECTING` clears (8.2); after the second fails the app gives up
  with the last reason (`the stream ended` for an `eof`). A later drop
  starts again from the first. They are mpv's loads, which 6.2's limits
  do not count, on mpv's one connection: at most two a drop.
- **The user** playing anything, any player control (pause, mute, stop,
  fullscreen) and `R` cancel the retries; `L` retries at once. Moving
  through the lists does not.
- **mpv reconnecting by itself:** a `playback-restart` after the file
  played with no `seek` event before it is mpv reconnecting at the live
  edge, so behind live goes back to zero. Whether mpv sends one depends
  on the stream's timestamps (mpv 0.41, the demo provider, 2026-10-12):
  starting again at 0 on each connection drew one; running on from the
  last connection, as a broadcast's do and the demo's do since this
  ticket, drew none, and then the app cannot tell. The debug log says
  `live channel reconnected by mpv` when it sees one, so the user's run
  shows which the provider does.
- **Recording what plays** (9.4) goes on through mpv's reconnect, in one
  file. A provider's burst on connect repeats the last seconds already
  read; mpv plays them twice and its muxer drops the repeats, logging
  `Failed writing packet.` (the demo with `DEMO_DROP=15 DEMO_BURST=5`: 37
  s recorded held 27 s of stream, no gap, ending at the picture). That
  error used to end the recording in the app while mpv wrote on; now
  only the dump stopping does. A drop the app retries ends it, as before.
- **Verified** with mpv 0.41 against the demo provider, 2026-10-12:
  `DEMO_DROP=15` closed the stream every 15 s for a minute and the app
  saw no drop. With the server stopped mid-play mpv gave up with `eof`
  about 11 s later; both retries failed with `loading failed` 1 to 2 s
  after their `loadfile`, and the status bar said `Can't play`. With the
  server back 15 s after it stopped, the second retry played. Against
  the user's provider, 2026-10-12 to 18 of normal use with the debug
  log: mpv reconnected by itself 12 times, unseen; the app retried 2
  drops, each back after one retry in about 4 s, saying `RECONNECTING`
  and coming back at the live edge. Both were a clean `eof` about 600 s
  after a `loadfile`, on a channel showing a blackout picture: the
  provider closes such a stream every 10 min, and until then the app
  cannot tell it from a programme (milestone Open).

Blackouts (stream recovery ticket 05). `--probe-blackout <stream_id>`
plays a channel in an mpv window as the app runs it, with the user's
`mpv.conf`, until mpv is closed, and prints the spans ffmpeg's filters
find: `blackdetect` (from 1 s), `freezedetect` (a still picture, from
5 s) and `silencedetect` (from 2 s). Each is an mpv `--vf`/`--af`
`lavfi` filter with a label, and mpv reports its spans' starts and ends
as the `vf-metadata/<label>` (`af-metadata`) property changing, so the
app could observe them like any property, no log parsing. Each span's
line has how much of it was silent and the video bitrate in it and in
the minute before. The first minute plays without the filters and the
second with them, to compare mpv's CPU. A stream that ends is played
again with the app's retries.

- **Verified against the demo provider** (mpv 0.41, ffmpeg 9.0,
  2026-10-18, `DEMO_BLACKOUT`): a black picture with no sound was found
  as black, frozen and silent, to the second; a slate as frozen and
  silent, not black, its start reported 5 s late (`freezedetect`'s
  `d`). On the demo's MPEG-2 the video bitrate fell from about 1.5 to
  0.1 to 0.2 Mbit/s; on an H.264 copy of it (x264, `zerolatency`) only
  from about 0.76 to 0.58, so the bitrate depends on the encoder.
- **Hardware decoding:** with `hwdec=auto` (the user's `mpv.conf`) mpv
  decodes H.264 to frames on the GPU (`vulkan`), which a `lavfi` filter
  cannot take, and mpv disables the filter with only a log warning
  (`Disabling filter black because it has failed.`), so it finds
  nothing. With `hwdec=auto-copy` (`vulkan-copy`) the filters work. The
  probe switches to it when it adds them, which restarts the decoder (a
  `playback-restart`), and prints any filter mpv disables. On the
  demo's 640x360 at 25 fps mpv used about 5 % of a core before and 7 %
  with copy-back and the filters; the user's channels are the real
  measure.
- **A blackout** (verified against the user's provider, 2026-10-18,
  stream recovery ticket 05, one channel for about 10 min, H.264 with
  `hwdec=auto`): the picture is black, still and silent. All three
  filters found it at once when added (black at once, silent 2 s and
  frozen 5 s later, their `d`), and it lasted to the stream's end: the
  provider closed it with a clean `eof` 601 s after the `loadfile`, as
  in 9.3's drops above. Played again it started in 1.1 s, black again.
  mpv used about 5 % of a core decoding on the GPU and 10 % with
  copy-back and the filters. The channel has no alternate. Its bitrate
  was not recorded (the probe printed none for a span the stream's end
  cut off, fixed since).
- **False alarms** (verified against the user's provider, 2026-10-18,
  one normal channel for 44 min in the evening, H.264 at about 7
  Mbit/s): no span frozen for 5 s or silent for 2 s. `blackdetect` gave
  46 black spans, each 0.1 s or less: single dark frames at cuts.
  Its metadata marks every black run, whatever its `d`; only its log
  line keeps to `d`, so a reader of `vf-metadata` must apply the
  length itself. mpv used about 6 % of a core decoding on the GPU and
  13 % with copy-back and the filters, against 5 % and 10 % on the
  blacked-out channel.
- **So a blackout can be told:** black, still and silent together,
  for longer than a cut or a fade (10 s would do), which nothing on the
  normal channel came near. A slate would show as still and silent
  without black (the demo; none seen on the provider). The cost is
  copy-back decoding while watching, about 7 % of a core more on the
  user's channels, and a decoder restart if `hwdec` is switched mid-play.
  The blackout's bitrate was not recorded; with black and silence
  found directly it is not needed.

### 9.4 Recording what plays (recording ticket 01)

- `R` records on the connection mpv already has, from what is on screen
  to what is on screen at the stop. The provider's `.ts` is written as is;
  a recording cut off part way still plays.
- mpv reads well ahead of the picture: providers send a burst on connect,
  about 20 s with the user's. `stream-record` set while playing writes only
  what is read from then on, so a recording ran 20 s ahead of the screen at
  both ends (measured 2026-10-01). Instead, `R` asks for `time-pos` and runs
  `dump-cache <time-pos> no <file>` (async): it writes from the keyframe at
  or before the picture, then keeps appending what is read.
- A stop (`R`, the card's button, `s`, a channel switch) asks for
  `time-pos` and `demuxer-cache-state`'s `cache-end`, the difference being
  what mpv has written past the picture, ends the dump (`dump-cache 0 0
  ""`), waits for mpv to close the file, then cuts that much off its end
  (`src/ts.rs`: read back from the end, cut at the first packet starting a
  frame timed after it). A switch does this before its `loadfile`, which
  would end the dump anyway. mpv closing or the stream ending (`end-file`
  with `eof` or `error`) ends the recording uncut.
- The file: `<channel> - <programme> - 2026-10-01 20.00.ts`, the programme
  airing when `R` is pressed, left out without a guide, the time local.
  `/ \ : * ? " < > |` and control characters become `-`; a taken name gets
  ` (2)`, ` (3)` and on. The app creates the file (and `recordings_dir`)
  first, which reserves the name and shows the directory can be written;
  if not, the status bar says why and nothing is sent to mpv.
- A failure shows only in mpv's log (0.41): `Failed opening output file.`
  from the `recorder` module, then `Cache dumping stopped due to error.`;
  the command's own reply says just `error running command`. The app asks
  for `request_log_messages error` at spawn and ends the recording when
  `Cache dumping stopped due to error.` follows, with the `recorder`'s
  last error as the reason. A `recorder` error alone does not end it:
  mpv writes on after `Failed writing packet.` (stream recovery ticket
  02, 9.3). Other modules' messages are ignored:
  they can quote the stream URL. An empty file left by a failure is removed.
- Only the tracks mpv has selected are written: with no audio output (or
  `--aid=no`) a recording has no sound.
- Stored in `recording` (5) by the recorder, for the Recordings view
  (recording ticket 04): `recording` from `R`, with the channel, the
  programme airing and the file, its `stop` its `start` until it ends;
  then `done`, `failed` with mpv's reason (an empty file removed and
  forgotten), or `cancelled` if the view's `x` stopped it. Quitting
  keeps it as `done`, noted `cut short: the app quit`. It takes no
  connection of its own, so the recorder's limit (9.5) does not count it
  beyond mpv's.

### 9.5 Scheduled recordings (recording ticket 02)

The recorder (`src/recorder.rs`) runs on the worker runtime. Schedules live
in `recording` (5); the app must be running for them to record. A systemd
user timer, so they run with the app closed, may come later.

- **Padding:** a schedule records from `record_before` minutes before the
  guide's start to `record_after` after its stop (4.2), fixed when
  scheduled; a re-import moves both with the programme (7.1).
- **Start:** at the padded start, `ffmpeg -nostdin -hide_banner -loglevel
  error -i <stream url> -c copy -f mpegts -y <file>`, on a connection of
  its own: the provider's `.ts` as is. An M3U channel's stream (M3U
  ticket 03) is its stored `url`, with its headers (9.1) before the
  `-i`: `-user_agent <agent> -referer <referrer> -headers "Origin:
  <origin>\r\n"`. The file is named as in 9.4, with
  the guide's start time, and reserved the same way. `ffmpeg_path` sets
  the program. Like mpv it gets `PR_SET_PDEATHSIG`, so it dies with the
  app.
- **Stop:** at the padded stop, SIGTERM, which ffmpeg answers by finishing
  the file (killed after 2 s): `done`.
- **Timing:** the recorder sleeps until the next padded start or stop, and
  while anything is scheduled or recording also wakes each minute, which
  catches up after a suspend. Nothing is polled; with nothing scheduled it
  never wakes.
- **Connections (6.3):** a start that would make mpv's stream (if it plays
  from that provider) plus the running recordings exceed
  `max_connections` is refused: `failed`, with the counts in its note
  (`no free connection (the account's limit is 2; in use: watching, 1
  recording)`). Not retried. An M3U provider has no account, so no
  limit (M3U ticket 03).
- **Failures:** ffmpeg exiting before the padded stop, for any reason, or
  not starting at all, is `failed` with ffmpeg's last error line (or the
  spawn error) as its note, redacted (4.1) as ffmpeg quotes the URL. An
  empty file it leaves is removed and forgotten. A channel gone from the
  provider's list, or a provider gone from the credentials file, fails it
  too. Nothing is retried.
- **Cancel:** an unstarted schedule becomes `cancelled`; a running one is
  stopped as at its stop, `cancelled`, its file kept.
- **Launch:** recordings left `recording` by a crash become `done`, noted
  `cut short: the app closed`. Schedules whose padded stop passed while
  the app was closed (or the machine slept) are `missed`; the status bar
  says `Missed 2 scheduled recordings: the app was not running`, after any
  late start's message. One still inside its padded times starts late and
  stops at its padded stop.
- **Quit:** asks nothing. Every ffmpeg is stopped, and each recording kept
  as `done`, noted `cut short: the app quit`. The app waits for this, up
  to 5 s, before it exits.
- **Status bar:** `Scheduled to record <programme> on <channel>, 19:58 –
  21:05` (the padded times) when one is scheduled,
  `Recording <programme> on <channel>` when one starts,
  `Recorded to <file>` when done, `Recording <programme> on <channel>
  failed: <note>` in amber, `Cancelled recording <programme> on
  <channel>` when cancelled. Marks in the guide grid: 8.2. The Recordings
  view: 8.2.
- **Scheduling:** a programme scheduled or recording already is not
  scheduled again; the grid's `R` cancels it instead. A cancelled, missed
  or failed one can be scheduled again.

### 9.6 Downloads (VOD ticket 10)

The recorder (`src/recorder.rs`) queues downloads beside its recordings,
as both take the provider's connections; `src/download.rs` fetches them.

- **Queue:** `d` and `D` queue movies and episodes (8.3) at the end, in
  order, one at a time per provider. A title queued, downloading, or
  done with its file still there is not queued again; a failed one is,
  resuming its part; a done one whose file is gone is forgotten and
  queued anew. The status bar says what was queued: `Queued Red Summer
  to download`, `Queued 9 episodes of Broken Tide season 1 to download;
  1 already were`, `Red Summer is downloaded or queued already`.
- **Connections (6.3):** a download starts only on a connection mpv's
  stream (if it plays from that provider) and the running recordings
  leave free. Playing from the provider, or a recording starting, that
  needs its connection stops it and queues it again, keeping its part
  (`Paused downloading Broken Tide S1 E3: its connection is needed. It
  carries on once one is free`); it starts again once one is free. A
  movie or episode played from its downloaded file takes none.
- **Fetching:** a plain GET of the stream URL (6.1), redirects followed,
  written to `<file>.part`. A part from before resumes with `Range:
  bytes=<size>-` (VOD ticket 01: ranges are honoured); a 206 appends, a
  200 (the range ignored) starts the part again, a 416 on a part counts
  it whole. Connecting gives up after 15 s, and a read with nothing for
  60 s fails; there is no overall timeout. Once the part is whole
  (`Content-Range` or `Content-Length` say how big), it is renamed to the
  file: `done`. The status bar says `Downloading <what>` (`Resumed
  downloading` from a part), then `Downloaded to <file>`.
- **The file:** in `downloads_dir`, the movie's name, or `<series> -
  S02E04 - <episode name>` (`S02E04` alone without a name, `S02` without
  a number), and the stream's extension; characters a file name cannot
  hold become `-`, as for recordings (9.4). A name taken by a file or
  another's part gets ` (2)`, ` (3)` and on. The part is created at the
  first start, which reserves the name and shows the folder can be
  written.
- **Failures:** any other status (`the provider answered HTTP 404`), a
  connection error or a body that ends short (`the provider stopped
  sending at <n> of <total> bytes`) is `failed`, with why as its note,
  without the URL, and the part kept; nothing is retried. The status bar
  says `Downloading <what> failed: <note>. d tries again` in amber. With
  the provider down, each queued one fails in turn.
- **Quit:** each download stops at once and is queued again with its
  part; the next launch resumes the queue. One left `downloading` by a
  crash is queued again at launch.
- **Playing:** Enter (or `S`) on a downloaded movie or episode plays its
  file, from where it stopped as from the provider, saving its position
  and watched mark (VOD ticket 07) the same way.

## 10. Performance budgets

Measured on the user's machine with a 50k-channel provider (real or synthetic):

| Metric | Budget |
|---|---|
| Launch to first frame, cached data | < 300 ms |
| Search, per keystroke | < 16 ms |
| List scrolling | holds refresh rate at 60 Hz and 144 Hz |
| Idle CPU | ~0 |
| Idle memory | < 200 MB |
| Channel switch (keypress to `loadfile` sent) | < 50 ms |

The prototype's `PROTO_FPS=1` approach (auto-scroll, frame-interval stats)
carries over as a debug flag, `VIDICON_FPS=1`. It moves the selection on
every frame, which refills the detail panel on every frame; under that the
144 Hz output holds 141 to 142 fps, not 144. Accepted: real input never
does that, and without the refill the list holds 144. Measurements and how
to repeat them: `docs/perf/v1.md`.

Search over movies (VOD ticket 09): Movies' All, about 183k names, takes
5 to 16 ms per keystroke, with rare outliers to 18 ms on queries of one
to five letters made of common words, which match 20k to 120k titles.
Accepted: a query that short matches far more than anyone reads, and
once it narrows it takes 1 to 7 ms. Cutting the outliers would mean a
coarser ranking or a prefilter that drops scattered matches (8.5).

The guide grid (milestone ticket 06), under the same test on the 144 Hz
output: moving the time a minute per frame holds 144 (median 143.9 fps);
one row per frame holds a median of 140 to 142 fps, about one frame in
fifty missed while a row's blocks are built as it scrolls in. Accepted on
the same grounds as the list. 60 Hz holds 60 in both. The Skia renderer
held about 143 but used 272 MB resident, over the memory budget, so the
app stays on femtovg. `docs/perf/guide-grid.md`.

## 11. Testing

- **No real credentials or provider responses in the repo.** Fixtures are
  synthetic or scrubbed, and test data uses invented channel names, never
  ones copied from a provider or real broadcasters.
- `xtream` and `import`: tests against recorded, scrubbed JSON and XMLTV
  fixtures, including a generated 50k-channel list and a large XMLTV file.
- A local fake Xtream server (test-only) for refresh, backoff and error paths.
- Redactor: tests that no credential survives in logged URLs of both shapes.
- Favourites fallback: ID change with one, zero and several name matches.
- `player`: tests against a fake IPC socket; one manual smoke test with real mpv
  per release.

## 12. Build

- Toolchain from the flake devShell. Add runtime deps (SQLite if not bundled)
  there as they arrive.
- `nix build` (`packages.default` in `flake.nix`, added 2026-10-04 for
  daily use): a release build without the `devtools` feature, its tests
  run in the build. A wrapper puts the libraries winit loads at runtime
  (as in the devShell) first on `LD_LIBRARY_PATH`, and mpv and ffmpeg
  last on `PATH`, so ones the user has installed, with their config and
  scripts, are used in their place.
- It installs a launcher entry and icon from `assets/`
  (`vidicon.desktop`, `vidicon.svg`): the icon is a vidicon tube's
  faceplate with the app's play mark. Both match the window's Wayland
  app id, `vidicon`, so the compositor shows the icon on the window.

## 13. Open questions

1. ~~Layout~~: decided, see 8.2.
2. ~~Back to list from mpv~~: works on Umbriel, see 9.2.
3. ~~Name~~: Vidicon.
4. ~~Channel logos~~: first planned after v1, moved into v1 on 2026-09-29
   (ticket 13), `docs/milestones/channel-logos.md`.
5. ~~Rate-limit numbers~~: kept as specified after testing against the
   user's provider, see 6.2.
