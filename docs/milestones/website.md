# Milestone: website

Planned 2026-10-20. Tickets in `.scratch/website/`. A landing page and
user docs for Vidicon, served by GitHub Pages at
`https://emzbtw.github.io/vidicon/`.

## Why now

v0.1.0 is released with .deb, .rpm and Nix installs, and the README has
grown to hold the install, first-run, keys and disclaimer text. A site
shows the app better than a README, gives the docs room as milestones add
features, and is easier to find than a repo page.

## Requirements

- **Astro with Starlight**, in `site/` in this repo. Starlight's `splash`
  template is the landing page; its docs pages are the docs. A separate
  repo was rejected: docs change in the same commit as the code, the site
  imports repo files directly, and a project site in `emzbtw/vidicon`
  gets the `/vidicon` path for free.
- **One source of truth.** User-facing text that the README already has
  (Install, First run, Keys, Reporting a bug, Disclaimer) is imported
  from `README.md` at build time, never copied into `site/`. Editing the
  README changes the site on the next deploy.
- **Screenshots come from `docs/screenshots/`** (invented data from
  `tools/demo/`, per AGENTS.md), used in place, not copied into `site/`.
  A site-only image follows the same rule: taken against the demo
  provider, never the real one.
- **The disclaimer is on the landing page**: Vidicon ships no channels,
  playlists or streams and points to none.
- **No third-party requests at view time**: no analytics, trackers or
  font CDNs. Fonts are self-hosted.
- **Toolchain from the flake devShell** (`pkgs.nodejs`), with
  `site/package-lock.json` committed and `npm ci` in CI. The Nix package
  and the AUR builds stay untouched: the flake's `fileset` lists its
  sources, and the PKGBUILDs build with cargo only.
- **Deploy by GitHub Actions** (`.github/workflows/site.yml`): on pushes
  to `main` that touch `site/`, `README.md` or `docs/screenshots/`, and by
  hand. Pages' source is set to GitHub Actions.
- **Works at phone width and in light and dark**, with alt text on every
  image.
- **No real provider data**, as everywhere in the repo (AGENTS.md).

## Settled

- **README sections become pages through an Astro integration**
  (`site/readme-pages.mjs`, ticket 02). In `astro:config:setup` it
  splits `README.md` on `## ` headings and writes the pages to the
  git-ignored `site/src/content/docs/docs/`, before Starlight's content
  loader reads them; in dev an edit to the README restarts the server.
  Chosen over a custom content loader: plain Markdown files go through
  Starlight's normal pipeline, so `starlight-links-validator` checks them
  and Astro's image pipeline serves the screenshots. Over a separate
  pre-build script: it runs on every `astro build` and `astro dev`, with
  no npm hook to forget.
- **Pages, in sidebar order:** Install, First run, Keys, Reporting a bug,
  Disclaimer. "What it does", "What it does not do", Credits and Licence
  stay README-only or feed the landing page. The HTML header and intro
  come before the first `## `, so no page has them.
- **Rewrites:** an anchor to a README section becomes a link to its page,
  an anchor to a subheading a link to that page's anchor; `###` headings
  move up to `##` under the page title; `docs/screenshots/` image paths
  become paths relative to the generated file, so Astro serves the
  images from `_astro/` without a copy in `site/`.
- **Link checking:** `starlight-links-validator` in the build. A broken
  internal link or hash fails `npm run build`.

- **Font:** Inter through `@fontsource-variable/inter` (OFL-1.1),
  self-hosted. Its `opsz` axis set to 32 on headings gives Inter
  Display, so one package covers both (ticket 03).
- **Colours:** dark mode is the app's palette from `ui/app.slint` mapped
  onto Starlight's colour tokens in `site/src/styles/theme.css`; light
  mode inverts the same greys and darkens the accent for contrast. The
  app has no light theme to copy (ticket 03).
- **Landing page:** the disclaimer on it is the README's Disclaimer
  section, imported from the generated docs page, not copied. The
  licence, credits and AI note sit in a footer override on every page.

## Later

- Feature guides beyond the README (the settings file, EPG sources,
  recording, catch-up), as Markdown in `docs/user/` that both GitHub and
  the site render. Not in this milestone.
- A custom domain.
