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
  part covers both types. The README's "source: Xtream Codes" badge
  names M3U too.
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

- [x] Site builds with link checking; no real provider data in any
  screenshot. Retaken against the demo provider: `settings-add-provider`,
  `settings-providers` (now with an M3U provider), `setup-add-provider`;
  new: `settings-add-m3u`, `provider-picker`, `main-m3u`. The edit and
  remove views did not change for an Xtream provider and stay.
- [x] SPEC 10 budgets met with an M3U provider active
  (`docs/perf/v1.md`, With an M3U provider active).
- [x] Milestone review's findings fixed: an M3U stream's token in a
  failed recording's note and log (ffmpeg's last line, now naming the
  host only, and the log line redacted for Xtream too); `--probe-epg`
  reading xz feeds; the README's first-run wording, `x-tvg-url`, the
  section named "M3U providers"; AGENTS.md's Settled heading. Kept:
  config's absolute-path-only playlists (SPEC 4.1); the picker's type
  and the playlist URL's hidden query (ticket 06, checked by the user);
  the recorder's no-limit for M3U (SPEC 9); the duplicated playlist
  open-and-read shape in `import::playlist` and `check_playlist`, the
  probe's own M3U scan, and the type labels in three places in
  `ui.rs`, left as they are this close to the release.
- [ ] v0.2.0 released with notes.

## Out of scope

Nothing new in the app.
