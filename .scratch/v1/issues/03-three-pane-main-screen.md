# 03: Three-pane main screen

Status: done. Blocked by: 02.

## Goal

The v1 main screen: category sidebar, channel list, detail panel, driven by
real data from the store with full keyboard and mouse navigation.

## Context

SPEC 8.1 to 8.4. ADR 0005. Visual reference: layout A on the `prototype/slint`
branch (`prototypes/slint-ui/ui/app.slint`, `VariantA`); rewrite properly
rather than copying prototype code.

## Scope

- Sidebar: "All", "Favourites", then provider categories. Clickable,
  drag-pannable, `h`/`l` and Left/Right move between them.
- Channel list: Slint `ListView` used directly (never a component inheriting
  it, see ADR 0005), backed by a Rust `Model` that pages rows from the store.
  Rows show `num`, name, and a placeholder where now-playing goes (ticket 05).
- Detail panel: selected channel's name and category (now/next arrives in 05).
- All keys in SPEC 8.3 that apply here: `j`/`k`, arrows, `Ctrl-d`/`Ctrl-u`,
  PageUp/PageDown, `g`/`G`, Home/End, Enter plays, `q` quits. Selection kept
  in view.
- Mouse per SPEC 8.4: click, double-click plays, wheel, click-drag pan.
- Palette and type from the prototype: dark, Inter, Inter Display headings.
- **Dev tooling: `--keys "<sequence>"`** (`devtools` feature, see ticket 01):
  replay keys through the same key handler as real input before
  `--screenshot` fires, e.g. `--keys "l l j j j"`. Plain characters map to
  themselves; named keys use a small syntax such as `<Enter>`, `<Esc>`,
  `<Down>`, `<C-d>`. Keys must not reach mpv (no playback in replay).

## Acceptance

- [x] Browsing a real provider: categories switch instantly, selection stays
      visible, Enter plays.
- [x] With a 50k-channel synthetic provider, row model only builds visible rows
      (instrument `row_data` calls in a debug build).
- [x] Mouse and keyboard share one selection.
- [x] `--keys "l l j j" --screenshot x.png` captures the first provider
      category with the third row selected. (Was "l j j", but the second
      sidebar entry is Favourites, which is empty until ticket 07.)

## Out of scope

Now/next data (05), search mode (06), favourites actions (07).
