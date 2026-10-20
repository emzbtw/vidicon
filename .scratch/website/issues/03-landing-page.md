# 03: Landing page

Status: done. Blocked by: 01.

## Goal

The site's front page shows what Vidicon is and how it looks, and sends
readers to install it or read the docs.

## Context

`docs/milestones/website.md`. `README.md` (intro, What it does,
Disclaimer, Credits), `docs/screenshots/`, `assets/vidicon.svg`,
`ui/app.slint` (palette), AGENTS.md (Font, no real provider data).

## Scope

- Hero: icon, name, the tagline "A fast, keyboard-driven IPTV player for
  Linux", buttons to Install and to the docs, and the GitHub link.
- The demo GIF, then a short feature tour with screenshots from
  `docs/screenshots/` (main screen, search, guide grid, movies, player).
  Feature wording from the README's "What it does" where it fits; any
  new text stays short.
- The disclaimer visible on the page: no channels, playlists or streams
  shipped or pointed to; bring your own provider.
- Footer: licence (GPL-3.0-or-later), credits link, "written with the
  help of AI coding assistants" as in the README.
- Settle Inter via Fontsource and colours from the app's palette;
  self-hosted, no third-party requests.
- Works at phone width and in light and dark.

## Acceptance

- [x] Checked by screenshots of the deployed page at 375 px and desktop
  width, light and dark.
- [x] Every image has alt text, and every image shows invented data only.
- [x] The browser's network panel shows no request outside
  `emzbtw.github.io`.

## Out of scope

Docs pages (02). Pointing the README and repo at the site (04).
