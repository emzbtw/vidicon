# 25: Channel name rules

Status: done. Blocked by: none.

## Goal

Hide channels by name, for providers that file channels by format rather
than region: an `8K|` category kept for its UK channels also holds `FR:`,
`DE:` and `PL:` ones. Category rules (ticket 20) cannot reach those; a
channel rule such as "names containing `FR:`" can.

## Context

SPEC 5 (category allowlist), 8.2 (manage mode), 8.5 (search), 10
(budgets). Ticket 20 left hiding single channels out of scope. Found with
the guide grid (guide-grid ticket 01): on the real provider about a third
of All's guide channels are named for regions whose categories are hidden.

Every rule is the user's, per provider, and there are none by default, so
nothing provider-specific is built in. A provider without region prefixes
simply never needs one.

## Behaviour

- No channel rules: nothing changes.
- A channel rule hides every channel whose name contains it at the start
  of a word: at the start of the name, or after anything but a letter or
  digit. Case-insensitive and NFKC-folded, like category rules.
- Why not a plain substring: on the real provider `FR:` would also hide
  every `AFR:` channel (about 200), and `ES:` names with `bikes:` or
  `series:`. Why not prefix only: about 480 channels carry their region tag
  mid-name (`… | FR:`), which a prefix rule would miss. `BE/FR:` matches
  `FR:`, since `/` is a word break.
- Rules, not a snapshot: channels the provider adds later follow them, and
  they survive restart and refresh.
- Hidden channels drop out of All, their category, search, and the guide
  grid. Favourites and Recently played show them regardless, as with
  hidden categories.
- A category whose channels are all hidden by rules still shows, with none.

## Scope

- Schema: channel rules per provider. `shown` derived per channel, like
  `category.shown`, recomputed by each channel import and each edit, so the
  list queries read it directly.
- Manage mode: a way to add and remove channel rules, with how many
  channels each hides. How it sits beside the category list is part of the
  ticket; propose it before building.
- All, the categories and the search index restricted without breaking the
  budgets; the guide grid's query (`store::guide_keys`) included.
- Update SPEC 5, 8.2, 8.3 and 8.5.

## Acceptance

- [x] Tests: matching (case, folding, word start: `FR:` hides `FR: x`,
      `x | FR: y` and `BE/FR: z` but not `AFR: x`); a new channel matching a rule is
      hidden after an import; Favourites and Recently played unaffected;
      the guide grid's channels follow the rules.
- [x] Rules survive restart and refresh.
- [x] With rules: All's first frame, scroll and search per keystroke within
      SPEC 10, numbers in `docs/perf/v1.md`.
- [x] Screenshot of manage mode with a channel rule, demo data.

## Out of scope

Hiding a single channel by hand (an override per channel). Regex rules.
Changing how category rules match. Sharing rules between providers.
