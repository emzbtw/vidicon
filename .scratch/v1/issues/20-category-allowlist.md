# 20: Category allowlist

Status: done. Blocked by: 19.

## Goal

Show only the categories you want. Out of about 900, a few rules such as
"categories containing `UK|`" pick the ones shown, and new categories the
provider adds later follow the same rules.

## Context

SPEC 5, SPEC 8.5 (search), SPEC 10 (budgets). Rules, not a snapshot: a
snapshot of today's matches would let next month's new `UK|` category slip
through. open-tv has hidden categories that do not survive a refresh
(`research/reference-players.md`); these must.

## Behaviour

- No rules: every category shows, as now.
- With rules, a category shows if its name matches any rule (case-insensitive
  substring), unless it has its own override.
- Per-category overrides win over rules, both ways: hide `UK| ADULT` inside
  an allowed `UK|`, or show one category no rule covers. Keyed by
  `category_id`, with a name fallback when the ID changes (like favourites,
  SPEC 5).
- Hidden categories drop out of the library, All and search. Favourites and
  Recently played show their channels regardless.

## Scope

- Schema: rules and overrides per provider.
- A manage mode for the library pane (key to be picked; clickable too): a
  filter field over every category, each row marked shown or hidden with a
  one-key/one-click toggle (an override), "show only matching" to add the
  filter text as a rule, the list of rules with remove, and a count of
  categories and channels shown.
- If the rules show nothing, the library says so and names the key for
  manage mode, so the way back is always visible.
- All and the search index restricted to shown categories without breaking
  the budgets.
- Update SPEC 5, 8.2, 8.3 and 8.5.

## Acceptance

- [x] Tests: rule matching; overrides both ways; a new category matching a
      rule shows after an import; an override survives a category ID change.
- [x] Rules and overrides survive restart and refresh.
- [x] With rules: All's first frame, scroll and search per keystroke within
      SPEC 10, numbers in `docs/perf/v1.md`.

## Out of scope

Hiding single channels. Regex or prefix-only rules. Sharing rules between
providers.
