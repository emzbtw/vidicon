# 0001: Xtream Codes API is the only source in v1

Date: 2026-09-29. Status: accepted.

## Decision

v1 reads channels and EPG only from providers speaking the Xtream Codes API.
No M3U files or URLs, no Stalker portals. EPG comes from the provider's
`xmltv.php`. The data model still scopes every channel, favourite and EPG row to
a provider, so several providers can coexist; the v1 UI shows one active
provider with a switcher.

- **Amended 2026-10-12:** a provider can also take its guide from external
  XMLTV sources (URLs or files), ordered with its own `xmltv.php` and
  matched to its channels (`docs/milestones/epg-sources.md`). Channels and
  streams still come only from Xtream.
- **Amended 2026-10-22:** a provider can also be an M3U playlist, by URL
  or path (`type = "m3u"`), its entries all live channels, its guide
  from the playlist's `url-tvg` feeds and external sources
  (`docs/milestones/m3u.md`). Catch-up, VOD and the account stay
  Xtream-only. Stalker portals are still out.

## Why

It is what the user's providers speak. Stalker is niche and awkward
(ynoTV is the only reference player that supports it, open-tv's `stalker.rs` is
empty). Multi-provider is cheap if designed in from the start and expensive to
retrofit.

## Consequences

- M3U support can be added later as another source type, behind the same
  provider abstraction.
- Xtream puts credentials in every stream URL; see ADR 0004 for handling.
- All three reference players have reports of provider IP bans, so provider
  requests are rate-limited from day one.
