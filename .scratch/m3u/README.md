# M3U tickets

The milestone in `docs/milestones/m3u.md`. One file per ticket in
`issues/`. Work any ticket whose blockers are closed; set `Status: done`
in the file when its acceptance boxes are all ticked.

Each ticket ends with a `code-review` of its diff (since the commit the
ticket started from), standards and the ticket as its spec; findings are
fixed in the same ticket before `Status: done`. Before v0.2.0, ticket 07
reviews the whole milestone since the commit that planned it.

Performance is measured in the tickets that add the work (02's import,
05's xz), recorded in `docs/perf/v1.md`; 07 re-runs the SPEC 10 table
with an M3U provider active. A miss over budget becomes its own ticket.

| # | Ticket | Blocked by |
|---|---|---|
| 01 | Measure a playlist | none |
| 02 | Import an M3U provider | 01 |
| 03 | Play and record M3U channels | 02 |
| 04 | Hide what an M3U provider lacks | 02 |
| 05 | The playlist's guide | 02 |
| 06 | M3U in the providers form | 02, 04 |
| 07 | Screenshots and docs | 03, 04, 05, 06 |
