# 05: EPG import and now/next

Status: done. Blocked by: 02, 03.

## Goal

The provider's XMLTV is imported as a stream and every channel shows what's on
now and next.

## Context

SPEC 5 (`programme`), 6.1 (`xmltv.php`, `epg_channel_id` mapping), 7.1.
ADR 0003. Prototype layout A shows the target rows and detail panel.

## Scope

- Fetch `xmltv.php`, gunzip if needed, parse with a pull XML parser, insert as
  it goes in one transaction replacing the provider's programmes.
- Keep only programmes ending after `now - 1 h` and starting before
  `now + 3 days`.
- Record `provider.last_epg_refresh`.
- Rows: now-playing title and a progress bar. Detail panel: now (title, times,
  minutes left, progress) and the rest of today.
- Local time display.
- Channels without EPG show a neutral placeholder, not an error.

## Acceptance

- [x] Test: large generated XMLTV fixture imports with flat memory; pruning
      window respected.
- [x] Test: failed import rolls back, previous programmes kept.
- [x] Real provider: now/next correct for a sample of channels (spot-check
      against the provider's own guide).
- [x] Progress bars advance without user input (redraw at minute boundaries,
      not every frame).

## Out of scope

Guide grid (milestone), refresh scheduling (08).
