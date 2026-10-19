# 02: Edit and remove a provider

Status: done. Blocked by: 01.

## Goal

A provider's details can be changed without losing its data, and a
provider can be removed, keeping personal data unless asked.

## Context

`docs/milestones/onboarding.md`, ticket 01. SPEC 5 (what is scoped to a
provider), 9.5 (scheduled recordings). `src/store.rs`, `src/recorder.rs`.

## Scope

- Edit: name, URL, username, password; the id never changes. Changed
  credentials are used from the next request; a changed URL or username
  keeps all data.
- Remove: confirm with counts. In one transaction, delete the block and
  the bulk data (channels, EPG, movie and series lists, guide tables) and
  cancel scheduled recordings. Favourites, progress and recording
  history stay unless the checkbox is ticked. Files on disk stay.
- Removing the active provider switches to the next, or to the first-run
  screen (03) when none is left; until 03 is done, an empty state.
  Settled 2026-10-12: until 03, removing the only provider is refused
  (`It is the only provider: add another first`); 03 lifts it.
  Personal data covers favourites, progress, recently played,
  recordings and downloads past scheduled or queued, category and
  channel rules and kept tags.
- SPEC 4.1, 5, 8.2 updated.

## Acceptance

- [x] Tests: edit keeps rows; remove keeps and with the box deletes
  personal data; a re-added id gets them back; scheduled recordings
  cancelled.
- [x] Checked against the demo provider.

## Out of scope

EPG sources on a provider (EPG sources milestone).
