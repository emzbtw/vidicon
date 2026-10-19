# 08: Refresh scheduling and status

Status: done. Blocked by: 02, 05.

## Goal

Data stays fresh without the user thinking about it, and the UI says what's
happening.

## Context

SPEC 6.2 (politeness, backoff), 6.3 (account status), 7.2 (refresh rules),
4.2 (`refresh_hours`).

## Scope

- On launch: serve cached data; refresh channels then EPG if older than
  `refresh_hours` (12 or 6).
- Check staleness every 15 min while running.
- `r` forces a refresh, still rate-limited.
- Backoff on 429/5xx per SPEC 6.2.
- Account check each refresh: expired or disabled blocks refresh with a clear
  message; cached data stays browsable. Show expiry and max connections
  somewhere unobtrusive.
- Status area: idle / refreshing (with progress) / last success / last error.
- First run with an empty database: a "loading channels" state.

## Acceptance

- [x] Tests against a local fake Xtream server: stale triggers, fresh doesn't,
      backoff timings, expired account.
- [x] UI never blocks during refresh (scroll while importing).
- [x] `refresh_hours = 6` honoured.

## Out of scope

Logo fetching (milestone).
