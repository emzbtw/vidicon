# Milestone: named favourite groups

Parked 2026-10-02 and again 2026-10-06 (SPEC 2). Revisit once the
favourites list is long enough that one flat list gets unwieldy. Below is
a sketch from 2026-10-06, not settled.

## Starting point

`favourite.group_id` exists and is always NULL (SPEC 3). The key is
`(provider_id, kind, stream_id)`, so a favourite can be in one group at
most as the schema stands.

## Proposed

- **Data:** `favourite_group (provider_id, id, name, position)`. NULL
  `group_id` is Ungrouped. Groups are per provider, like favourites.
- **Library:** Favourites stays and lists everything; each group sits
  indented under it, in group order. A group lists its favourites in
  their manual order, across kinds, as Favourites does.
- **Guide grid:** each group is a category after Favourites, so `[` and
  `]` reach it.
- **Adding:** `f` as now; inside a group it adds to that group. `b`
  (free) opens a picker on the selected item: the groups plus "New
  group…", `j`/`k` and Enter. It also moves a favourite between groups.
- **Order:** `J`/`K` inside a group reorder its favourites.
- **Managing:** rename, delete and reorder groups in manage mode (`e`).
  Deleting a group moves its items to Ungrouped, it does not unfavourite
  them.
- **ID changes:** the name fallback (SPEC 3) keeps `group_id`.

## Open questions

- One group per favourite or several? One fits the schema and makes `b`
  a move. Several needs a join table, and `b` becomes a checklist.
- Top-level Favourites: one flat manual list as now, or sections with a
  header per group? This decides what `J`/`K` do there.
