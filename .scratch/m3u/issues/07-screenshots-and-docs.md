# 07: Screenshots and docs

Status: open. Blocked by: 03, 04, 05, 06.

## Goal

The docs and the site say Vidicon reads M3U playlists, and v0.2.0 ships
it.

## Context

`docs/milestones/m3u.md`, `docs/releasing.md`. `README.md`, `site/`,
`docs/screenshots/`, `CONTEXT.md`, `SPEC.md` 2, `tools/demo/`.

## Scope

- README and site: M3U out of "What it does not do", a short M3U
  section with the `credentials.toml` example; the install guide's setup
  part covers both types.
- `CONTEXT.md`: "Provider" and "Xtream Codes API" no longer say Xtream
  is the only source ("M3U provider" is already there).
- SPEC 2: the milestone marked done. AGENTS.md Settled: the ADR 0001
  line names the M3U amendment.
- Screenshots retaken where a view changed for M3U (the providers form
  at least), against the demo provider.
- The SPEC 10 table (launch, search, scrolling, idle, switch) re-run
  with an M3U provider active, in `docs/perf/v1.md`.
- A `code-review` of the whole milestone, since the commit that planned
  it, findings fixed before the release.
- Version 0.2.0, released by `docs/releasing.md`.

## Acceptance

- [ ] Site builds with link checking; no real provider data in any
  screenshot.
- [ ] SPEC 10 budgets met with an M3U provider active.
- [ ] Milestone review's findings fixed.
- [ ] v0.2.0 released with notes.

## Out of scope

Nothing new in the app.
