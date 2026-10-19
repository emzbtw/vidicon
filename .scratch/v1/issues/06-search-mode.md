# 06: Search mode

Status: done. Blocked by: 03.

## Goal

`/` opens a focus-list style search in the middle column with live fuzzy
filtering over channel names.

## Context

SPEC 8.2 (search mode), 8.5. ADR 0003 amendment (names in memory). Visual
reference: layout B on the `prototype/slint` branch.

## Scope

- In-memory index of `(stream_id, name)` for the active provider, rebuilt after
  each channel import.
- nucleo matcher, best match first, scoped to the current category.
- `/` enters search mode: the middle column becomes a large search field with
  wide result rows. Typing filters live. Enter keeps the filter and returns to
  the three-pane view; Esc clears it. Navigation keys work while searching.
- Typed text is handled by the app (no text-input widget stealing `j`/`k`),
  as in the prototype.

## Acceptance

- [x] 50k channels: under 16 ms per keystroke (log timing in debug).
- [x] Enter/Esc behave as specified; the kept filter is visible in the
      three-pane header.
- [x] Unit tests for ranking and category scoping.

## Out of scope

EPG title search.
