# Milestone: channel logos

Planned as the first milestone after v1; moved into v1 on 2026-09-29 as
ticket 13, once the detail panel redesign (ticket 12) made the room for them.
`channel.icon_url` is already stored, so this needs no import changes.

## Requirements

- **Lazy:** fetch logos only for rows on screen (and a small margin), never the
  whole list.
- **Disk cache** under `$XDG_CACHE_HOME/vidicon/logos/`, keyed by URL hash,
  with a size cap and pruning.
- **Politeness (SPEC 6.2):** a logo on the provider's own host goes through
  the provider's rate limiting, at a lower priority than API calls so it
  never delays a refresh. Measured 2026-09-29, 91% of the user's logos are on
  one image host and only 26 hosts serve about 14.5k distinct URLs; those third
  party hosts get their own per-host limit (2 in flight, same backoff) rather
  than the provider's 2 s gap, which would take half a minute per screen.
- **Off the UI thread:** download, decode and resize on the worker runtime;
  the UI only receives ready-sized images. Scrolling must still hold 144 fps
  at 144 Hz.
- **Fallback:** missing, broken or failed logos show the channel's initials on
  a colour tile derived from the name, so rows never jump in layout.
- **Normalise size:** logos are fitted into a fixed box regardless of their
  source dimensions.
