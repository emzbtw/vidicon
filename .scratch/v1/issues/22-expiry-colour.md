# 22: Expiry warning colour

Status: done. Blocked by: none.

## Goal

The account's expiry in the status bar warns before it happens instead of
staying neutral until the account is blocked.

## Context

SPEC 6.3. `account_text` in `src/ui.rs` builds the text; the palette in
`ui/app.slint` has `warn` (amber) but no red.

## Scope

- 4 to 7 days left: `warn`. 1 to 3 days, and expired or disabled: a new
  `danger` red in the palette.
- Within 7 days the text turns relative ("Expires in 3 days", "Expires
  today 18:00"), which is what matters that close; further out it stays a
  date.
- Updated by the existing minute tick; no new timers.
- Update SPEC 6.3.

## Acceptance

- [x] Tests: thresholds at 8, 7, 4, 3, 1 and 0 days, and expired.
- [x] Checked against the demo provider with its expiry moved close.

## Out of scope

Desktop notifications. Configurable thresholds.
