# Milestone: EPG sources

Planned 2026-10-12, last of four: after settings (`settings.md`), whose
EPG section and file-writing it uses, and the refresh panel
(`refresh-panel.md`), which gains a row per source. Tickets in
`.scratch/epg-sources/`. Amends ADR 0001. Done 2026-10-13 (tickets 01
to 05).

## Why now

The guide comes only from the provider's `xmltv.php`, which covers about
3.4k of about 58k channels (SPEC 6.1). A user whose provider's guide
falls short has no way to bring their own. How the reference players do
it, and where they fall short: `research/reference-players.md` section 6.

## Requirements

- **Sources per provider, in `credentials.toml`:** a URL or a local
  path, plain or gzipped XMLTV, any number. URLs often carry a token, so
  they are under the permission check (ADR 0004) and the redactor, and
  Settings masks a URL's query string until a key shows it.
  ```toml
  [[provider.epg]]
  url = "https://epg.example.net/guide.xml.gz"  # or path = "/srv/epg/guide.xml"
  every_hours = 2      # optional, default 12, at least 1
  max_per_day = 20     # optional, default none
  shift_minutes = 0    # optional
  enabled = true       # optional
  ```
- **Order:** each provider has an ordered list of sources, its own
  `xmltv.php` one row among them, first by default. A channel's guide
  comes whole from the first enabled source with programmes for it; no
  merging, so no duplicate programmes. Reordered in Settings › EPG with
  `J`/`K`, as favourites. "Prefer external" is moving the provider's row
  down.
- **Pins:** a channel can be pinned to one source, overriding the order,
  with its own `shift_minutes`. Set from the channel's detail panel. A
  pin to a disabled source is kept and skipped; the channel follows the
  order meanwhile.
- **Matching, per source:** first a manual mapping, then an exact id
  (`epg_channel_id` = `<channel id>`), then the display name as
  `name_norm` (SPEC 5), then only if that finds nothing a looser form
  (region prefixes, brackets and punctuation also dropped; ticket 01,
  SPEC 6.1). Each step counts only when it gives exactly one match; two
  or more is no match, never a guess. Settings › EPG shows per source
  `matched 1,200 · ambiguous 40 · none 56,000`; the ambiguous list
  opens the mapping picker. Matches are recomputed after each import of
  the source and stored; manual mappings survive re-imports and use the
  favourites name fallback when provider ids change.
- **Mapping picker:** from the channel's detail panel, a search over the
  source's channels by id and names.
- **Shift:** `shift_minutes` per source (the provider's guide included)
  and per pin, applied at import, so the grid, catch-up, scheduled
  recordings and search all see corrected times.
- **Refresh:** sources join the EPG stage (SPEC 7.2), after the
  provider's guide. Each is fetched with `If-Modified-Since` / `ETag`
  and not more often than its `every_hours`, unless `r`; a local path is
  re-read when its mtime changes. Unchanged costs a request and no
  import.
- **Fetch limits:** every request to a source counts, conditional, failed
  and `r` ones included, over a rolling 24 h, recorded in SQLite so it
  survives restarts. With `max_per_day`, scheduled fetches stop 2 short
  of it, kept for `r`; at the cap `r` skips the source and the refresh
  panel says so (`guide.xml.gz: 20/20 in 24 h, next 14:05`). A `429` or
  `503` with `Retry-After` backs that source off until then, as 6.2 does
  for the provider. Nothing is tuned to any one EPG service: limits are
  whatever the user sets.
- **Isolation:** a failing source shows its error in the refresh panel
  and keeps its last good data; it never fails the provider's refresh.
- **Storage:** a source's whole channel list (id, names, icon) is kept,
  for the picker. Programmes only for matched channels, from the longest
  archive among the provider's channels (about 7 days, or 1 day without
  archive) to 3 days ahead, as the provider's guide (SPEC 7.1); the
  feeds measured reach 1 to 2 days ahead. A new manual mapping marks the source
  for re-import on the next refresh, one fetch, and the panel says so.
- **Catch-up:** a past programme from any source is playable when its
  channel has an archive (`tv_archive`) and it falls within
  `tv_archive_duration`, using that programme's shifted times. A
  provider refusal goes through the existing catch-up errors (9.3).
- **Adding a source:** Settings › EPG checks the URL's syntax or the
  path's readability without fetching, saves it last in the order, then
  imports it at once (one fetch) and shows the match counts, offering to
  move it above the provider's guide. A failed fetch (404, not XMLTV,
  401 or 403 from a bad token) still saves the source, marked failing
  with the reason.
- **Disable:** `enabled = false`. Not fetched, its rows kept but skipped,
  so turning it back on is instant.
- **Remove:** confirm dialog. Deletes the source's block and, in one
  transaction, its channels, programmes, matches and pins (`3 pinned
  channels go back to the source order`). Manual mappings are kept,
  coming back if a source with the same URL is added, unless "Also
  delete 41 manual mappings" is ticked.
- **The provider's guide can be disabled too.** Then `xmltv.php` and
  the older-day guide tables (catch-up ticket 06) are not fetched, and
  its programmes are deleted. The toggle warns first:
  `Provider guide off: 312 of your favourites have no other source`.
- ADR 0001 amended. SPEC 4.1, 5, 6.1, 6.2, 7.1, 7.2, 8.2, 8.3, 8.4
  updated. The demo provider (`tools/demo/`) serves an invented external
  feed with ids that differ from its channels, and one that matches.

## Proposed (settle in the tickets)

- Key for pin and mapping from the detail panel: `E` (EPG) is free
  (SPEC 8.3); one key opens a small menu with both. Carried into 05,
  which settled on a card listing the sources instead (its ticket).
- A source's identity is its URL or path with the query string dropped,
  so a renewed token keeps its mappings. Carried into 02.

Settled in ticket 01 (SPEC 6.1):

- Name matching: `name_norm` first, then the looser form with region
  prefixes, brackets and punctuation dropped, only when the first finds
  nothing. The looser form alone loses about 2k matches to ambiguity.
- Forward reach: 3 days ahead, as the provider's guide and the grid
  (SPEC 7.1, 8.2), not 7: the feeds measured reach 1 to 2 days ahead.
