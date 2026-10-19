# Stream recovery tickets

The milestone in `docs/milestones/stream-recovery.md`. One file per ticket
in `issues/`. Work any ticket whose blockers are closed; set `Status: done`
in the file when its acceptance boxes are all ticked.

| # | Ticket | Blocked by |
|---|---|---|
| 01 | Measure drops and alternates | none |
| 02 | Reconnect a dropped channel | 01 |
| 03 | Switch to an alternate | 01 |
| 04 | Offer an alternate when a channel stays down (parked) | 02, 03, 05 |
| 05 | Measure blackouts | none |
