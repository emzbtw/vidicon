# CONTEXT

Glossary for Vidicon, an IPTV player. Terms only; decisions go in `docs/adr/`.

## Terms

- **Provider**: an IPTV service the user has an account with, or a playlist they read channels from. A provider speaks the Xtream Codes API or is an M3U provider.
- **Xtream Codes API**: the HTTP API (`player_api.php`, credentials as query params) a provider exposes for listing and streaming content. One of two source types, and the only one with an account, catch-up and VOD.
- **M3U provider**: a provider read from an M3U playlist, by URL or local path, instead of the Xtream Codes API. Its entries are all live channels; it has no account, catch-up or VOD.
- **Channel**: a live stream offered by a provider. v1 content is live channels only (no VOD, series or catch-up).
- **EPG**: the electronic programme guide: what is airing on each channel and when.
- **Favourite**: a channel the user has marked for quick access.
- **Player**: the single long-running external mpv process the app drives over mpv's JSON IPC socket. The app never renders video itself.
- **Behind live**: how far the Player's picture trails the live broadcast, which is the time spent paused since the stream last loaded. **Going live** reloads the stream to end it.
- **Category rule**: a pattern that picks which of a provider's categories the library shows. A category shows if its name contains any rule's pattern; with no rules, every category shows. Rules, not a list of categories, so ones the provider adds later follow them.
- **Override**: one category's own shown or hidden setting, which wins over the category rules either way.
- **Channel rule**: a pattern that hides channels by name, for providers that file channels by format rather than region. It hides a channel whose name contains the pattern at the start of a word, wherever its category shows. None by default.
- **Kept tag**: a region code, such as `uk`, whose channels stay when the provider tags channel names by region (`UK: x`, `FR| y`). With any kept, a channel whose name starts with a region tag shows only if one of the tag's codes is kept; channels without a tag always show. None by default.
- **Alternate**: another of the provider's channels carrying the same broadcast under a different region or quality tag (`4K: x`, `US: x HD`), to play when one listing stops working. The app shows them as "other listings" and never switches by itself.
- **Archive**: a provider's store of a channel's recent broadcasts, kept a number of days (often 3). Only some channels have one.
- **Movie**: a single film a provider offers to play on demand (Xtream calls it VOD). Not live: it plays from its start, or from where it was stopped.
- **Series**: a show a provider offers on demand, in seasons of episodes. An **episode** plays like a movie.
- **Kind**: whether a category or title is live, a movie or a series. The library shows one kind's categories at a time.
- **Resume position**: how far into a movie or episode playback got when it stopped, kept so it can continue there. Near the end it counts as watched.
- **Continue watching**: the movies part way through and each series' next episode, latest first.
- **Download**: a movie or episode saved to disk to play later, unlike a recording, which captures live TV as it airs.
- **Catch-up**: playing a programme that has already aired, or the one airing now from its start, from the channel's archive. Not live, so going live returns to the live stream.
