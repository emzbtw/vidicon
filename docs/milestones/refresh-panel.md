# Milestone: refresh panel

Planned 2026-10-12 with settings, onboarding and EPG sources; first of
the four, as it depends on none of them. Tickets in
`.scratch/refresh-panel/`.

## Why now

A refresh shows only a sweeping line on the status bar and its state in
words (SPEC 7.2, 8.2). A refresh runs four stages (channels, EPG, movies,
series) and can take about a minute; when one is slow, waiting on a
backoff (6.2) or failing, the status bar cannot say which. External EPG
sources (`epg-sources.md`) add a row per source.

## Requirements

- **Refresh panel:** a small card anchored bottom right, above the status
  bar, over the main screen and not blocking it. One row per stage, in
  run order: `Channels ✓ 58k`, `EPG ▸ 41 %`, `Movies waiting`,
  `Series waiting`; a stage in backoff shows its countdown
  (`Movies: provider busy, retry in 0:42`).
- **When it shows:** for a refresh started with `r`, and for any refresh
  that fails. Background refreshes (launch, 15 min check) stay quiet:
  the sweep line only. A click on the sweep line, or `r` while a refresh
  runs (absorbed by it, 7.2), opens the panel.
- **Closing:** by itself a few seconds after a refresh succeeds; a
  failed one stays, showing the error, until dismissed. Esc or a click
  on its × dismisses it; the refresh carries on.
- **First run:** with an empty database the same rows show full-window,
  in large type, in place of the "loading channels" state (7.2). Once
  channels land, the main screen takes over and the remaining stages
  carry on in the panel.
- Keys panel (`?`) and SPEC 7.2, 8.2, 8.3, 8.4 updated.

## Settled (ticket 01)

- Progress: every stage by the body read of its list or guide, as a
  percentage of `Content-Length` when the provider sends one, else in
  MB. Not rows imported: counting them means threading a counter
  through each import's parser, and MB is what the status bar already
  showed. The done row gives the row count.
- Auto-close after 4 s, as the `c` arm (8.3).
- Esc order: the panel closes before anything else Esc does, after the
  modal cards (picker, dashboard, keys, Recordings), which sit over it.
- A retry after a backoff runs the stages that refresh had not done,
  forced or not; before, a forced refresh's retry skipped the stages
  still fresh, so an `r` that hit a 503 never finished.
- An account check that fails marks the first stage not done, as it
  never started; a launch check alone (data fresh) has no stages and
  opens nothing.

## Settled (ticket 02)

- First run means the provider's channels were never imported, not an
  empty channel list: a provider with every category hidden is not on
  its first run.
- The status bar stays under the first-run screen, as in every mode.
