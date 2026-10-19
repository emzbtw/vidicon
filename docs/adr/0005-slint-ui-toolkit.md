# 0005: Slint is the UI toolkit

Date: 2026-09-29. Status: accepted.

## Decision

The UI is built with Slint (1.18 at the time of writing): `.slint` markup for
layout and styling, Rust for state and logic.

Visual direction stays open: all three prototype layouts (classic three-pane,
focus list, guide strip) are candidates and may be combined.

## Evidence

Same screen built in iced 0.14 and Slint 1.18 (50k fake channels, now/next,
fuzzy search, Inter). Branches `prototype/iced` and `prototype/slint`.

- **Virtualised list.** Slint's `ListView` virtualises natively with smooth
  pixel scrolling, wheel, scrollbar and click-drag panning. iced 0.14 has no
  virtual list; the prototype had to hand-roll one that scrolls a row at a time.
- **Stable API.** Slint is 1.x; iced is pre-1.0 and changed its builder API in
  0.14.
- **Memory.** About 125 MB idle vs 165 MB for iced.
- **Frame pacing.** Both lock to monitor refresh (60 Hz and 144 Hz, following
  the window between outputs). On the list layouts Slint renders in about 2 ms at
  144 Hz. On the guide grid under a stress test (one row scrolled per frame)
  Slint dropped to 134 to 140 fps at 144 Hz while iced held 144.
- **Design iteration.** `.slint` markup with live preview suits the part of the
  project that is least settled: the look.

## Consequences

- **Guide grid performance target:** 144 fps at 144 Hz on the guide grid is an
  acceptance criterion for that milestone (`docs/milestones/guide-grid.md`).
  First things to try: reuse per-row
  block models instead of rebuilding them, and the Skia renderer.
- **Never wrap `ListView` in a custom component that inherits it.** In the
  prototype this silently disabled virtualisation: all 50k rows were
  instantiated (534 MB, a full CPU core). Use `ListView` directly.
- **Licence:** Slint is `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR
  LicenseRef-Slint-Software-3.0`. Publishing means either GPL for the app or
  meeting the royalty-free licence's terms (attribution). Revisit when the
  project's own licence is chosen.
