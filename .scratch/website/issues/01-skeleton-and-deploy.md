# 01: Skeleton and deploy

Status: open. Blocked by: none.

## Goal

An Astro and Starlight site in `site/` builds in the devShell and is
deployed by GitHub Actions to `https://emzbtw.github.io/vidicon/`.

## Context

`docs/milestones/website.md`. `flake.nix` (devShell), AGENTS.md
(Development environment), `.github/workflows/release.yml` for the
workflow style.

## Scope

- `pkgs.nodejs` in the devShell; check the package name and version with
  the `mcp-nixos` tool.
- `site/` from the Starlight starter, with `site: 'https://emzbtw.github.io'`
  and `base: '/vidicon'`, current Astro and Starlight versions pinned in
  `site/package-lock.json`. `site/node_modules` and `site/dist` ignored.
- A placeholder landing page (`splash` template) and one placeholder docs
  page, both plainly marked as placeholders.
- `.github/workflows/site.yml`: `npm ci` and `npm run build` in `site/`,
  then `actions/upload-pages-artifact` and `actions/deploy-pages`. Runs on
  pushes to `main` touching `site/`, `README.md` or `docs/screenshots/`,
  and by `workflow_dispatch`. Action versions current and pinned.
- Pages' source set to GitHub Actions. This changes the public repo's
  settings, so ask the user before running
  `gh api -X POST repos/emzbtw/vidicon/pages -f build_type=workflow`.
- AGENTS.md: `site/` named in "What this is"; Development environment
  says `npm` comes from the devShell and the site builds with
  `cd site && npm ci && npm run build`.

## Acceptance

- [ ] `npm run build` passes in the devShell, from a clean `site/`.
- [ ] The workflow run is green and the URL serves the placeholder pages,
  with working links and assets under `/vidicon/`.
- [ ] `nix build` still works, and its source does not include `site/`.

## Out of scope

Real content: docs (02) and the landing page (03).
