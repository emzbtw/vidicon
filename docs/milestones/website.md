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

## Proposed (settle in the tickets)

- How README sections become pages: a script run before `astro build`
  that splits `README.md` on `## ` headings into a git-ignored folder of
  Starlight pages, or a custom Astro content loader. The script is
  simpler; a loader avoids generated files. Settle in ticket 02.
- Which README sections become pages and in what sidebar order. A first
  guess: Install, First run, Keys, Reporting a bug, Disclaimer.
  "What it does" and Credits feed the landing page instead.
- README anchors (`#keys`, `#disclaimer`) rewritten to page links, and
  `docs/screenshots/` paths to the site's image paths.
- Inter through Fontsource (OFL-1.1), matching the app's font, with
  Inter Display for headings.
- Colours taken from the app's palette in `ui/app.slint`, so the site
  looks like the app.
- Link checking with `starlight-links-validator` in the build.

## Later

- Feature guides beyond the README (the settings file, EPG sources,
  recording, catch-up), as Markdown in `docs/user/` that both GitHub and
  the site render. Not in this milestone.
- A custom domain.
