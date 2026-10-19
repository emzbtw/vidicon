# 02: Docs from the README

Status: open. Blocked by: 01.

## Goal

The site's docs pages are built from `README.md`'s sections, so the
README stays the only copy of that text.

## Context

`docs/milestones/website.md` (one source of truth, Proposed). `README.md`.
Starlight's content collections docs.

## Scope

- Settle the mechanism: a pre-build script that splits `README.md` on
  `## ` headings into git-ignored Starlight pages, or a custom content
  loader. Record the choice and the reason in the milestone doc.
- Settle which sections become pages and their sidebar order (first
  guess: Install, First run, Keys, Reporting a bug, Disclaimer).
- Rewrite README anchors that point to another page into links to that
  page, and `docs/screenshots/` image paths into ones the site serves,
  without copying the images into `site/`.
- The README's HTML header (icon, badges, link row) does not appear on
  docs pages.
- Link checking in the build (`starlight-links-validator` or equal), so a
  broken link fails the build.

## Acceptance

- [ ] No README text exists in a committed file under `site/`.
- [ ] Editing a sentence in `README.md` and rebuilding changes the page.
- [ ] Every link and image on the docs pages works on the deployed site;
  the build fails on a broken one (tried once with a deliberately broken
  link).
- [ ] The README still renders as before on GitHub.

## Out of scope

The landing page (03). New docs that are not in the README (milestone
doc, Later).
