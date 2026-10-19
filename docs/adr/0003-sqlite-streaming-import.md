# 0003: SQLite with streaming import and paged queries

Date: 2026-09-29. Status: accepted.

## Decision

All provider data lives in a local SQLite database.

- Channel lists and XMLTV are imported as streams into normalized tables, in a
  background task, one transaction per import. The full list or guide is never
  held in memory.
- The UI only ever queries a page of rows at a time.
- **Exception, amended 2026-09-29:** channel names (with their row keys) are
  held in memory for fuzzy search. Everything else about a channel, and all
  EPG data, is still read from SQLite by page.
- Old EPG entries are pruned on import.
- On launch the app serves the local copy immediately and refreshes in the
  background when data is older than 12 h (6 h selectable).

## Why

Poor performance with large lists is one of the two reasons for this project.
Of the reference players, open-tv's streaming import plus paged queries is the
approach that holds up; IPTVnator loads the whole playlist as one JSON blob, and
ynoTV's database grows to around 5 GB and hangs startup. See
`research/reference-players.md`.

The search exception: fuzzy matching 50k names in memory takes 4 to 5 ms per
keystroke (prototype benchmark) and costs a few MB. Matching through SQLite
FTS5 is slower and not truly fuzzy. Names are a small fraction of a channel
row, so the memory cost stays small even for very large lists.

## Consequences

- Full-text search (FTS5) over channels and EPG is available when needed, for
  example as a prefilter if in-memory names ever become a problem.
- The EPG timeline grid (a later milestone) needs no new data layer.
