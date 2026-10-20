# 04: Point to the site

Status: done. Blocked by: 02, 03.

## Goal

The README, the repo page and the release docs lead to the site.

## Context

`docs/milestones/website.md`. `README.md` (badges, link row),
`docs/releasing.md`.

## Scope

- README: a "Website" link in the centred link row, and the link row or
  badges otherwise unchanged.
- Repo homepage set to the site: `gh repo edit emzbtw/vidicon --homepage
  https://emzbtw.github.io/vidicon/`. This changes the public repo page,
  so ask the user first.
- `docs/releasing.md`: one line saying the site redeploys when the
  README changes and needs nothing at release time.
- `docs/milestones/website.md` updated to what was built.

## Acceptance

- [x] The repo page's About box links to the site.
- [x] The README link opens the deployed site.

## Out of scope

A custom domain.
