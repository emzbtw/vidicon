# 24: Keys panel

Status: done. Blocked by: none.

## Goal

`?` opens a popup listing every key, grouped so the one you want is quick
to find. Same card style and closing rules as the account dashboard.

## Context

SPEC 8.3 (the key table), 8.2 (account dashboard, status bar hints). Keys
are mapped in `action()` in `src/ui.rs`; the dashboard (ticket 21) is the
model for the popup: overlay, card, modal key handling, close on Esc, a
close button or a click outside. `?` is unbound today.

## Scope

- `?` opens it, from the main view. In search and manage mode `?` types,
  as every printable key does, so it does not open there.
- Closes with Esc, `?`, `q`, the × button or a click outside. Modal while
  open, like the picker and the dashboard.
- Groups, in this order, each key with a short action name:
  1. **Move:** `j`/`k` and arrows, `Ctrl-d`/`Ctrl-u`, PageDown/PageUp,
     `g`/`G` and Home/End, `h`/`l` and Left/Right.
  2. **Channels:** Enter play, `f` favourite, `J`/`K` move favourite,
     `-` last channel.
  3. **Player:** Space pause/resume, `s` stop, `m` mute, `F` fullscreen,
     `L` go live.
  4. **Library:** `/` search, `a` order, `e` manage categories, `c` clear
     recently played.
  5. **App:** `r` refresh, `p` provider (only with more than one), `i`
     account, `?` keys, `q` quit.
  6. **In other views**, shorter: search (type to filter, Enter keeps, Esc
     clears), manage mode (type to filter, Enter shows/hides, Ctrl+Enter
     adds/removes a rule, Esc leaves), account dashboard (`1`–`3` show a
     login field), keys inside mpv (SPEC 9.2).
- Keys drawn as small keycaps, action text dim beside them. Two or three
  columns so it fits a 1280×800 window without scrolling.
- One list in Rust feeds the panel. A test fails if an `Action` has no
  entry, so a new key cannot be left off.
- Design: one screenshot against the demo provider for the user to approve
  before polishing; alternatives only if it is rejected.
- `? keys` added to the idle status bar hints.
- Update SPEC 8.2 and 8.3.

## Acceptance

- [x] Every key in SPEC 8.3 and 9.2 is on the panel; test for `Action`
  coverage.
- [x] Opens with `?`, closes with each of Esc, `?`, `q`, × and a click
  outside; does not open while searching or managing.
- [x] Screenshot in `docs/screenshots/keys.png`, demo data.

## Out of scope

Configurable bindings. Mouse gestures. A searchable list.
