# 13: Channel logos

Status: done. Blocked by: 12.

## Goal

Real logos in list rows, the detail panel, recently played and the NOW
PLAYING card, without costing scroll smoothness, idle CPU or politeness.

## Context

`docs/milestones/channel-logos.md` (requirements), SPEC 6.2 (logo
politeness), SPEC 10 (budgets). 99% of the user's channels have an
`icon_url`; about 14.5k distinct URLs across 26 hosts.

## Scope

- Lazy fetching for rows on screen plus a small margin, the detail panel and
  the cards; never the whole list.
- Disk cache `$XDG_CACHE_HOME/vidicon/logos/`, keyed by URL hash, size cap
  with pruning; failures remembered so a broken URL is not retried every
  scroll.
- Per-host limits as SPEC 6.2; provider-host logos through the provider's
  limiter at low priority.
- Download, decode and resize on the worker runtime to the sizes drawn (32,
  40, 44, 88 px at the window's scale factor); the UI only receives ready
  images. The initials tile from 12 stays as the fallback and while loading,
  so rows never jump.
- Logos fitted into the box, centred, on a neutral backing so transparent
  and white logos stay visible.

## Acceptance

- [x] Tests: cache hit, miss, prune; per-host concurrency and backoff against
      the fake server; a broken image falls back to the tile.
- [x] Scrolling 50k channels holds 144 fps on the 144 Hz output with logos
      loading (checked again in 11).
- [x] Idle memory stays under SPEC 10's 200 MB with a warm cache (about
      157 MB, release build, 2026-09-29).

## Out of scope

Logos for categories. Fetching logos anywhere but the provider's `icon_url`.
