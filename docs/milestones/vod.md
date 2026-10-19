# Milestone: movies and series

After catch-up (`catch-up.md`, done 2026-10-02). SPEC 1 and 2 list VOD
and series as not v1. Planned 2026-10-02; tickets in `.scratch/vod/`.
Done 2026-10-04 (tickets 01 to 11).

## Why now

Live, the guide grid, recording and catch-up are done. The user's provider
also offers movies (Xtream VOD) and series, which the app cannot reach
today. Nothing about them has been measured yet: how many there are, how
big their lists are, and what their info calls return are ticket 01's
first job.

## Requirements

- **Browse movies and series** in the main screen's three panes, beside
  live channels, under category rules (8.2). The user's provider does not
  file VOD by region as it files channels (01, SPEC 6.1), so the live
  allowlist matches no movie or series category: see Schema below.
- **The library shows one kind at a time.** A Live · Movies · Series switch
  in the library pane picks whose categories list below it, so the pane
  stays as long as it is today. Favourites, Recently played and Continue
  watching sit above the switch and span all three kinds.
- **Details in the right pane:** a movie's poster, year, length, genre,
  rating, plot and cast; a series' poster and plot, then season pills and
  the season's episodes, each with its length, watched mark and resume
  progress.
- **Play** a movie or an episode in the one Player (ADR 0002), as a
  channel plays.
- **Resume where you stopped:** the position per movie and per episode,
  kept as it plays; Enter resumes, another key starts from the beginning.
  Near the end counts as watched. Continue watching lists what is part
  way through, latest first, and for a series its next episode.
- **Next episode** when one ends.
- **Posters**, fetched and cached as channel logos are (SPEC 6.2 limits,
  ticket 13 of v1).
- **Download** a movie, an episode, or a whole season to disk.
- **Search scoped to the kind on screen:** `/` in a Live, Movies or Series
  category searches that kind only, and says so in the search bar; Tab
  widens it to all three, results grouped by kind. In Favourites and
  Recently played it searches what is in them.
- **Same politeness (6.2)**, and never the whole list in memory (ADR 0003).

Mocks from planning, on a canvas: A (library sections), B (mode switch,
series shown), C (poster screen), and the chosen mix, A's layout with B's
episode panel, a kind switch in the library and scoped search
(<https://claude.ai/artifact/Tc5iUneeDJRL6NeKCVpXYQ>, private to the user).
C, a poster grid screen like the guide grid, may come later as an
optional view.

## Proposed (settle in the tickets)

- **Endpoints** (SPEC 6.1, verified in 01): `get_vod_categories`,
  `get_vod_streams`, `get_vod_info&vod_id=`, `get_series_categories`,
  `get_series`, `get_series_info&series_id=`. Streams at
  `/movie/{user}/{pass}/{id}.{container_extension}` and
  `/series/{user}/{pass}/{episode_id}.{container_extension}`, both
  redirected to a streaming host as live is.
- **Schema:** categories gain a kind (live, movie, series), so overrides
  and manage mode cover all three. **Rules per kind** (from 01): live,
  movie and series category names begin differently (`UK| `, `EN - `, no
  prefix), and none of the user's live rules matches a VOD category, so
  shared rules would hide every movie and series. Each kind gets its own
  rule list, and a kind with none shows everything, as live does today
  (settled in 02; channel rules and kept tags stay live only). `movie` and `series` tables from the lists, imported
  streaming like channels (7.1); seasons and episodes stored per series
  as its info is fetched, not imported whole. The movie list has no year,
  length, genre or plot (those come with its info); the series list has
  year, genre, plot and cast.
- **Info on demand:** a movie's or series' info is fetched when it is
  selected, through the client's gate behind refreshes (as guide tables
  are, catch-up ticket 06), and kept until the next refresh of its list.
  1 to 25 KB and under 0.4 s each (01). A movie's `duration_secs` is
  unreliable (01); its length shows from mpv once it plays. Season pills
  come from `episodes`' keys: `seasons` lists the show's seasons, not
  the ones the provider has.
- **Refresh:** the two lists are about 70 MB each (01), against 21 MB
  for the channels, so they refresh on their own longer schedule (once a
  day) and by the manual refresh key, not with every channel refresh
  (settled in 02; SPEC 7.2).
- **Resume:** mpv's `time-pos`, saved every few seconds and on stop, per
  movie or episode; watched past about 90 % or the last 5 minutes.
  `time-pos` starts at 0 and `loadfile ... start=<secs>` resumes (01).
  Settled in 07: saved every 5 s, whichever of 90 % and the last 5
  minutes comes first (at most a quarter), `S` plays from the start, and
  Enter on a series plays its next episode in Continue watching only.
- **Favourites and history** gain a kind, so a movie or series can be a
  favourite and Recently played shows what was watched. Required, not
  just tidy: series ids collide with live and movie ids (01). Settled
  in 11: an episode's play records its series, and the detail panel's
  RECENTLY PLAYED stays channels only.
- **Downloads:** a plain HTTP download (01): ranges are honoured, so a
  cut download resumes where it stopped, and it ran at about 18 MB/s.
  The account check showed no connection for it, but the account has
  `max_connections` 1; until shown otherwise a download takes a
  connection like a recording (6.3), so it cannot run beside playback
  from the provider. Settled in 10: it takes a connection, untested
  against the provider, by the user's choice; playback or a recording
  that needs it pauses the download, which resumes with `Range` once one
  is free. Downloads have their own tab in the Recordings view (Tab
  switches) and their own folder, `downloads_dir`, default
  `Videos/Vidicon/Downloads`, with `recordings_dir`'s default moved to
  `Videos/Vidicon/Recordings` beside it (SPEC 4.2, 9.6).
- **Posters** are all on third party hosts (01), about 160k distinct
  URLs, nearly all 600×900 JPEG of about 100 KB: the cache keeps them
  scaled down to the sizes drawn. Settled in 06: their own 256 MB cache
  of 240 px JPEGs, the hosts' limits shared with logos (SPEC 6.2).
- **Search** keeps the names in memory as live search does (01): about
  232k movies and series, 7 MB. Movies' All, about 183k names, may come
  near the 16 ms keystroke budget; 09 measures. Settled in 09: one index
  per kind, each name with a set of its letters that rules out most
  non-matches before nucleo, no FTS prefilter; Tab widens to all kinds,
  each group its best 50 (SPEC 8.5).
- **Keys:** `d` downloads the selected movie or episode, `D` the season
  (both free in the main view; `d` is only taken inside the Recordings
  view).

## Open

- Next episode: play on by itself, with a countdown to cancel, or only
  offer it. Settle in ticket 08.
