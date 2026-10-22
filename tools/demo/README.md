# Demo provider

For screenshots in `docs/screenshots/`, which must show invented data only.

- `demo_xtream.py <port>`: a fake Xtream provider on `127.0.0.1` with
  made-up categories, channels and a guide built around the current time,
  running the 3 days ahead the app keeps, each programme with an invented
  description.
  It also serves invented logos (drawn shapes): most from `localhost`, which
  the app treats as a third-party image host, every fifth from `127.0.0.1`,
  the provider's own host, and a few missing or not images, which keep
  their initials tile.
  Movies and series have invented posters (drawn gradients, VOD ticket
  06), all from `localhost`; a few have none, are missing or are not
  images, and keep their initials tile.
  Every channel streams a test pattern from `/live/` (`ffmpeg`, from the
  devShell), for checking playback and recording with the real mpv. The
  pattern shows the stream's own time, large, so a recording's first and
  last frames say where it starts and ends. `DEMO_BURST=20` sends the first
  20 s at once on connect, as providers do, so mpv reads that far ahead.
  `/timeshift/` serves the same pattern for catch-up, `minutes` long and
  labelled `ARCHIVE` with the start asked for; the server's clock is UTC.
  `xmltv.php` reaches a day back on a channel with an archive, as real
  providers' do; `get_simple_data_table` has the rest, to a day past the
  archive, base64 as on the real provider, with `has_archive` on those
  the archive keeps (catch-up ticket 06).
  `cargo test real_ffmpeg -- --ignored --nocapture` records 8 s of
  channel 1 with the real `ffmpeg`, as a scheduled recording would, and
  prints the file.
  It also lists invented movies and series with their categories
  (`get_vod_streams`, `get_series`, VOD ticket 02), in the field types the
  real provider sends; movie categories mostly begin `EN - `, series ones
  mostly have no prefix, so live category rules match none of them.
  Series ids collide with live stream ids, as on real providers.
  `get_vod_info` answers in the real provider's shape (VOD ticket 04),
  `duration_secs` in minutes as there. Every `/movie/` streams one test
  pattern, 5 minutes long and labelled `MOVIE`, made with `ffmpeg` on
  first use into the temp directory and served with ranges, so the real
  mpv knows its length and seeks. `get_series_info` answers in the real
  provider's shape too (VOD ticket 05): `seasons` lists one more season
  than `episodes` holds on every third series, one in 11 has no episodes,
  and titles are whole (`<series> - S01E02 - <name>`). Every `/series/`
  episode streams the same file as a movie.
- `fake-mpv`: stands in for mpv. Serves the IPC socket, answers
  `observe_property`, `cycle`, and `time-pos` and `duration` (12 min in
  at the load, or from the `start=` asked, of 1 h 33 min), and `seek`
  (VOD ticket 07), plays nothing. With `FAKE_MPV_LOG=<file>` it appends
  each `loadfile` to that file, to check its options (M3U ticket 03).

The API accepts one invented account, username `demo` and password
`demo`; any other gets `{"user_info":{"auth":0}}`, as wrong credentials do
on a real provider (onboarding ticket 01).

`DEMO_CONNECTIONS` sets the account's `max_connections` (default 2), and
`DEMO_RATE` the megabytes a second a movie or episode is sent at (default
as fast as it goes): `DEMO_CONNECTIONS=1 DEMO_RATE=8` shows a download
running, and pausing while a channel plays (VOD ticket 10).

`DEMO_EXPIRY_DAYS` moves the account's expiry (default 160 days, fractions
allowed): `DEMO_EXPIRY_DAYS=0.25` expires in six hours, a negative value has
expired.

`DEMO_DROP=30` closes each live stream 30 s after it starts, as a provider
dropping it (stream recovery ticket 01); the archive is not dropped. A live
stream's timestamps run on from the server's start, as a broadcast's do, so
the next connection's carry on rather than starting again at 0 (stream
recovery ticket 02).

`DEMO_BLACKOUT=black` turns each live stream's picture black and its sound
off from 20 s to 40 s of every minute after it connects, as a blackout
would; `slate` shows a still slate instead (stream recovery ticket 05).

`DEMO_SLOW=xmltv:20` sends one request's body over 20 s, and
`DEMO_FAIL=get_vod_streams:503` answers one with that HTTP status, keyed
by its `action` (`xmltv` for `xmltv.php`), to show a refresh's stages in
the refresh panel (refresh panel ticket 01): a 429 or 5xx backs off and
counts down, any other status fails.

Two invented external guides (EPG sources ticket 02), for the
screenshot set's channels, any query string accepted as a token:
`/epg/match.xml` has the provider's own guide ids with titles of its own,
so placing it above `provider_guide = true` shows the order at work;
`/epg/other.xml.gz` is gzipped and covers the channels the provider has
no guide for, by ids and names of its own (`Harbour Two (UK)`), matched
by the looser name form, with times an hour late, put right by
`shift_minutes = -60`. Regional variants there share a name, so about
40 channels are ambiguous. Both send an `ETag` and answer a request
that sends it back with a 304 (EPG sources ticket 03);
`DEMO_FAIL=epg:429` answers them with a 429 and `Retry-After: 60`.
`/epg/other.xml.xz` is the second feed again, xz-compressed and on time,
for the playlist below (M3U ticket 05).

```toml
[[provider.epg]]
url = "http://127.0.0.1:47811/epg/match.xml?token=demo"

[[provider.epg]]
provider_guide = true

[[provider.epg]]
url = "http://127.0.0.1:47811/epg/other.xml.gz?token=demo"
shift_minutes = -60
```

`/playlist.m3u` (M3U ticket 02, any query string accepted as a token)
is an invented M3U playlist of the same channels, each streaming from
`/live/` as the Xtream ones do, in CRLF, with the forms real lists use: a
backup stream under the same key, one stream in two groups (`Highlights`
too), a blank line and a comment inside an entry, unquoted attributes, a
title with a comma, a URL with a 1.5k-character token, a URL with no
`#EXTINF`, an `#EXTINF` with no URL (`1 entry skipped`) and an
`#EXT-X-CREDENTIALS` line. Channels 7 and 8 need headers (M3U ticket
03): 7 sets a user agent with a comma and a referrer by `#EXTVLCOPT`
lines, 8 a user agent and an origin by a `|` suffix. `/live/` refuses
them (403) without that user agent. `DEMO_SLOW=playlist:20` and
`DEMO_FAIL=playlist:503` work on it. Its `#EXTM3U` line names two guide
feeds (M3U ticket 05): `match.xml` by `url-tvg` and `other.xml.xz` by
`x-tvg-url`, so its channels get a guide with no `[[provider.epg]]`.

```toml
[[provider]]
id = "lists"
name = "Demo Lists"
type = "m3u"
url = "http://127.0.0.1:47811/playlist.m3u?token=demo"
```

`demo_xtream.py <port> <channels>` adds generated categories after that set
until there are `<channels>` channels, each logo shared by a few of them:
`demo_xtream.py 47811 50000` is the provider `docs/perf/v1.md` was measured
against. The generated channels are tagged `GEN:`, untagged or `ALT|` by
category, a third each. Without the count the data is the screenshot set, unchanged.

## Taking screenshots

Use a throwaway `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `XDG_CACHE_HOME` so
neither your real config, database nor logo cache is touched. From the repo root, with `$D` a scratch
directory:

```sh
mkdir -p -m 700 $D/config/vidicon $D/data $D/run $D/cache
printf '[[provider]]\nid = "demo"\nname = "Demo TV"\nurl = "http://127.0.0.1:47811"\nusername = "demo"\npassword = "demo"\n' \
  > $D/config/vidicon/credentials.toml
chmod 600 $D/config/vidicon/credentials.toml
printf 'mpv_path = "%s/tools/demo/fake-mpv"\n' "$PWD" > $D/config/vidicon/config.toml

python3 tools/demo/demo_xtream.py 47811 &

vidicon() {
  env WAYLAND_DISPLAY=$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY \
      XDG_CONFIG_HOME=$D/config XDG_DATA_HOME=$D/data XDG_CACHE_HOME=$D/cache \
      XDG_RUNTIME_DIR=$D/run \
      ./target/debug/vidicon --credentials $D/config/vidicon/credentials.toml "$@"
}
vidicon            # first run imports the demo data and logos; quit with q once loaded
vidicon --screenshot /dev/null --keys "f j f jj f"   # favourites 1, 2 and 4
vidicon --play --screenshot docs/screenshots/main-guide.png \
  --keys "jj<Enter> j<Enter> jj<Enter> jkjkjkjkjkjk"
```

- `WAYLAND_DISPLAY` is made absolute because `XDG_RUNTIME_DIR` is replaced.
  With the real mpv, also pass `PIPEWIRE_RUNTIME_DIR=$XDG_RUNTIME_DIR` and
  `PULSE_RUNTIME_PATH=$XDG_RUNTIME_DIR/pulse`: without an audio output mpv
  selects no audio track, and recordings have no sound.
- Keep `$D` short: the mpv socket lives under `$D/run`, and Unix socket
  paths are limited to 108 bytes.
- `--play` lets `--keys` start playback; the `jk` pairs give the fake mpv
  time to start before later keys (Space, `m`) are sent to it.
- The keys that produced each file in `docs/screenshots/`, in All:
  `main-guide`: `jj<Enter> j<Enter> jj<Enter>`;
  `main-no-guide`: `<Enter>`, then `j`;
  `player-paused-muted`: `jjjjj<Enter>`, then `<Space> m`;
  `player-behind-live`: `jjj<Enter>`, then `<Space>` and about 130 s of
  `jk` pairs (2250 pairs, 30 ms each), so the card reads `2 min behind`;
  `search`: `jjjj<Enter>`, then `/northwind<Space>sport`;
  `search-all-kinds`: a few `jk` pairs, then `/harbour<Tab>` and about
  30 `<Down><Up>` pairs so the posters have come (VOD ticket 09);
  `recently-played`: `h h`, after the plays above;
  `other-listings`: 14 `j`, then `o` (channel 15's 8K copy, stream
  recovery ticket 03);
  `movies`: `<Tab> jj`, then about 4 s of `jk` pairs so the info has
  come (the requests are 2 s apart); `series`: `<Tab><Tab> jj`, about
  4 s of `jk` pairs so its episodes have come, `<Enter> jj l j <Enter>`,
  then about 9 s of `jk` pairs (VOD ticket 05); `player-movie`:
  `<Tab> jj<Enter>`, then about 9 s of `jk` pairs, so mpv's position and
  length have been asked (VOD ticket 04);
  `a-z`: `a`, as the order is kept;
  `guide-grid`: `t jjj`, in provider order, after the favourites above;
  `guide-grid-tomorrow`: `t jjj <gt>` (`<lt>` and `<gt>` type `<` and `>`);
  `guide-search`: `t /northwind<Enter> j`;
  `guide-catch-up`: `jjjjjt h<Enter>`, padded with `jk` pairs, so the
  status bar shows it playing (channel 6 has a 7-day archive);
  `guide-recording`: `t jjj R j l R`, padded with `jk` pairs, with the
  real `ffmpeg` (the airing one records from the demo provider) and
  `recordings_dir` as for `player-recording`;
  `player-recording`: `jjjjjjj<Enter>`, then `R` and about 5 s of `jk`
  pairs, with `recordings_dir = "/tmp/Videos/Vidicon"` in `config.toml`
  (the status bar shows the path);
  `recordings`: `t jjj R`, 40 `jk` pairs, then `v jk`, with the real
  `ffmpeg` and `recordings_dir` as for `guide-recording`, after writing
  one recording in each finished state and two schedules into the
  database's `recording` table (demo channels, times moved into the past
  for the finished ones; sparse files of a few hundred MB for sizes);
  `downloads`: with `DEMO_CONNECTIONS=1 DEMO_RATE=8` and
  `downloads_dir` set in `config.toml`: `<Tab><Tab> jjj`, about 200
  `jk` pairs so its episodes have come, `D` (a season queued), then
  quit part way; the next run resumes it: about 25 `jk` pairs, then
  `v <Tab>`, so the first is half way;
  `keys`: `?`, after a few `jk` pairs;
  `settings`: with a second `[[provider]]` (`id = "spare"`, `name =
  "Spare TV"`) in the credentials file, a few `jk` pairs, then
  `,j<Tab>j<Enter>k<Enter>jj` (6 h saved, the picker open on Spare TV);
  `settings-refused`: with `recordings_dir = "/tmp/Videos/Vidicon"` in
  `config.toml`, a few `jk` pairs, then
  `,jjj<Tab>jjllkk<Enter><C-u>/srv/tv<Enter>` (4 min saved, the folder
  refused);
  `settings-add-provider`: a few `jk` pairs, then `p j<Enter>`,
  `j<Enter>Spare<Space>TV<Enter>`,
  `j<Enter>127.0.0.1:47811/get.php?username=x<Enter>`,
  `j<Enter>demo<Enter> j<Enter>nope<Enter> j<Enter>` and 80 `h`
  (which move nothing, so the refusal stays; the form opens on its
  type, M3U ticket 07);
  `settings-add-m3u`: a few `jk` pairs, then `p j<Enter> <Enter>`
  (M3U), `j<Enter>Demo<Space>Lists<Enter>`, `j<Enter>` and the
  playlist URL below, `<Enter> j` and 80 `h`;
  `settings-providers`: the same, then `j<Enter>` (saved) and 120 `h`
  (M3U ticket 07);
  `provider-picker`: with Demo Lists saved as above and imported, a
  few `jk` pairs, then `p`;
  `main-m3u`: Demo Lists switched to and imported, then
  `jjjjjj<Enter>` and 9 `jk` pairs;
  `settings-edit-provider`: with the second `[[provider]]` as for
  `settings`, imported, a few `jk` pairs, then `,<Tab>j<Enter>`;
  `settings-remove-provider`: the same, then `jjjjj<Enter>`
  (onboarding ticket 02);
  `settings-epg`: with `match.xml` (`max_per_day = 20`) above
  `provider_guide = true` and `other.xml.gz` (`shift_minutes = -60`)
  after it, both imported, 60 `jk` pairs, then `,jjjjj<Tab>` (EPG
  sources ticket 04);
  `guide-mapping`: with `provider_guide = true` above `other.xml.gz`
  (`shift_minutes = -60`), imported, a few `jk` pairs, then
  `Ej<Enter>` on channel 1 (EPG sources ticket 05);
  `guide-sources`: the same, then `<Enter>` (mapped), quit; the next run
  re-imports the source, about 300 `jk` pairs, `Ejp`, then `l` twice,
  each after a few `kj` pairs so the pin is written first;
  `setup-add-provider`: with no `credentials.toml`, no keys;
  `setup-permissions`: with the file at mode `0644`, no keys;
  `setup-not-valid`: with `username = demo` (unquoted) in the file,
  then `<Enter>` (onboarding ticket 03);
  `refresh-panel`: with `DEMO_SLOW=xmltv:20`, `r`, then 190 `jk` pairs,
  so the guide is part way;
  `first-run`: with `DEMO_SLOW=get_live_streams:20` and an empty
  `XDG_DATA_HOME`, 120 `jk` pairs, so the channels are part way;
  `account`: `i`, padded with `jk` pairs on both sides (the keys go to the
  dashboard and do nothing) so its fresh check has answered;
  `categories`: last, as rules and overrides are kept:
  `a e uk|<C-Enter>`, three `<Backspace>`, `<Down>` four times and
  `<Enter>` (hides UK| KIDS), `de|<Space>sport<Enter>` (shows DE| SPORT),
  nine `<Backspace>`, `8k|<C-Enter>` and three `<Backspace>`, `fr:<C-h>`
  (hides the `FR:` channels in 8K| UHD) and three `<Backspace>`, `uk<C-k>`
  (keeps the `UK:` channels, hiding other tagged ones) and two
  `<Backspace>`, then `<End>`. The same `uk|<C-Enter>`, `fr:<C-h>` or
  `uk<C-k>` removes a rule or kept tag again. This order of categories
  assumes provider order: `a` first only if the order is A–Z.
  Pad with `jkjkjkjkjkjk` after the last `<Enter>`, as above.

`demo.gif`, at the top of the main README, is 22 screenshots, each a fresh
launch with `--play` on a copy of the same data (favourites set, logos and
posters cached), joined by `ffmpeg` with one 192-colour palette at 1200 px
wide. Each frame's keys extend the last: 8 `jk` pairs; `j`, `jj`, `jjj`;
`jjj<Enter>` and 6 `jk` pairs; `/` and `harbour` a letter at a time;
`<Tab>` and 12 `<Down><Up>` pairs; `<Esc> t`; `l` three times; `<gt>`;
`n`; `t`; `<Tab> jj` and 40 `jk` pairs. Holds run from 160 ms a letter to
2.6 s.
