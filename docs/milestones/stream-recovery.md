# Milestone: stream recovery

After movies and series (`vod.md`, done 2026-10-04), once the app was in
daily use. Planned 2026-10-06; tickets in `.scratch/stream-recovery/`.

## Why now

A live channel that drops stays dropped. mpv reports `end-file` (`eof`
or `error`, `src/player.rs`); for a live channel the app shows the error
or nothing, and the user starts it again by hand. v1 ticket 14 left
"reconnecting after the provider drops a stream that was paused too
long" for a later ticket that was never written. The same channel is
often listed more than once (a quality tag, another category), which
gives a second way back when the stream itself is gone.

## Requirements

- **Notice a drop:** a live channel never ends on its own, so `end-file`
  `eof` or `error` for the latest `loadfile` of a live channel is a drop.
  Catch-up's own end (catch-up ticket 04) is not.
- **Reconnect:** retry the same channel, a few times with a backoff, and
  say so in the status bar. After a pause, come back at the live edge, as
  `L` does.
- **Alternates:** the other shown channels with the same looser name
  form (SPEC 6.1: a leading region or quality tag such as `US:` or `4K:`
  dropped, bracketed text and punctuation too; `+1` kept). Changed from
  `name_norm` 2026-10-18 (ticket 03): `name_norm` keeps the tag, so it
  missed most. Hidden channels (rules, kept tags) and dividers
  (`##### SPORT #####`) are not alternates.
- **Switch by hand:** the user sees a channel's alternates and picks one,
  also when it plays but badly. Not a key that cycles through them
  unseen: with tags dropped, an alternate can be another region's feed,
  so the choice is laid out first (ticket 03).
- **Offer on failure:** when reconnecting gives up, the status bar says
  how many alternates there are and which key tries one. The app does
  not switch by itself: an alternate can differ in quality or language.
- **Politeness (SPEC 6.2)** and the one Player (ADR 0002): retries go
  through the same rate limiting, on mpv's one connection.

## Proposed (settle in the tickets)

- Retries: mpv's own `stream-lavf-o` reconnect options first. Ticket 01
  (SPEC 9.3) found they hide a closed connection from the app and, with
  nothing to reconnect to, give up with `eof` after about 10 s. The app's
  retries then start: 2, after 3 and 10 s, unless 02 finds a reason for
  more. 02 kept them (SPEC 9.3).
- Key: `o` opens the alternates in the detail panel (SPEC 8.3); not
  inside mpv, where `o` is mpv's own (9.2). Settled in ticket 03.
- Alternates in order: those with the same `name_norm` first, then the
  channel's own category, then provider order. Settled in ticket 03.

## Parked

Alternates (tickets 03 and 04), 2026-10-12. Ticket 01 (SPEC 9.3): about
8 % of shown channels have an alternate and none of the 27 recently
played do, and both drops in a week came back by playing the same
channel again, which ticket 02 does by itself. An alternate helps only
when a channel stays down, which was not seen.

Ticket 03 picked up again 2026-10-18: in ticket 02's run a channel
stayed down (a blackout picture) and the user found its alternate by
hand, which `name_norm` had missed. By the looser name form about 20 %
of shown channels and 8 of 15 recently played have one (SPEC 9.3).
Ticket 04 stays parked: a blackout is no drop (Open), so there is
nothing yet to offer an alternate on.

## Open

- **Blackouts:** a channel showing a blackout or holding picture plays
  as far as mpv and the app can tell. The user's provider closes such a
  stream after about 600 s, the app retries it and gets the same picture
  (SPEC 9.3). Ticket 05 (SPEC 9.3) found it can be told: the user's
  blackout is black, still and silent, which ffmpeg's `blackdetect`,
  `freezedetect` and `silencedetect` in mpv report as property changes,
  and 44 min of a normal channel gave nothing like it for even 2 s. The
  price is copy-back decoding (`hwdec=auto-copy`; with `auto` mpv
  disables the filters), about 7 % of a core more. Acting on it is
  ticket 04's, if it is picked up.
- Movies, episodes and catch-up dropping: none dropped in a week of use
  (ticket 01; no catch-up was played). Left as is until one does.
