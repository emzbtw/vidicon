# EPG sources tickets

The milestone in `docs/milestones/epg-sources.md`. One file per ticket
in `issues/`. Work any ticket whose blockers are closed; set `Status: done`
in the file when its acceptance boxes are all ticked. Ticket 04 also
needs settings 01 and refresh panel 01.

| # | Ticket | Blocked by |
|---|---|---|
| 01 | Measure an external source | none |
| 02 | Import sources in order | 01 |
| 03 | Conditional fetches and fetch limits | 02 |
| 04 | EPG section in Settings | 02, 03, settings 01, refresh panel 01 |
| 05 | Pins, mappings and channel shifts | 04 |
