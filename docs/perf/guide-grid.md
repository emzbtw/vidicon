# Guide grid performance

The guide grid milestone (`docs/milestones/guide-grid.md`). Same machine,
build and method as `docs/perf/v1.md`: release build with `devtools`, the
fake provider in `tools/demo/` padded to 50,000 channels, the window tiled
full-screen (1900x1060) on the 144 Hz output.

## Ticket 01: first measurement

Measured 2026-10-01, with the grid as ticket 01 left it: every row's
programmes read and its block model built when Slint asks for the row.

### Scrolling at 144 Hz

`VIDICON_FPS=1 vidicon --keys t`: the grid on All, the selection moved one
row per frame. About 27 one-second samples per run.

| View | Logo cache | fps per second, min / median / max | render p50 | render max |
|---|---|---|---|---|
| Grid | cold | 113.6 / 116.6 / 123.5 | 4.25 ms | 10.2 ms |
| Grid | warm | 105.5 / 116.2 / 122.7 | 4.24 ms | 48.4 ms |
| List (same session) | warm | 126.1 / 140.8 / 143.6 | 2.47 ms | 10.7 ms |

- The grid renders in about 4.2 ms per frame against 2.5 ms for the list,
  past the roughly 4 ms a frame has at 144 Hz, so it holds about 116 fps.
  The prototype's grid held 134 to 140 under the same test (ADR 0005).
  Ticket 06 owns the 144 fps target.
- The 48 ms render max was a single frame in one run.
- Not measured yet: 60 Hz, and moving through time (ticket 02 adds it).

### Finding the guide channels

The grid's rows are the view's channels with any programme, by an `EXISTS`
on the programme key (`store::guide_keys`). The ticket asked to measure
that first and keep a flag updated by the EPG import only if it is slow.
From `guide "…"` in the debug log, `--keys t`:

| View | Guide channels | Time |
|---|---|---|
| All, demo (60% of channels linked to the guide) | 30,136 of 50,000 | 39 to 71 ms |
| All, demo thinned to the real provider's coverage | 4,341 of 50,000 | 18 to 34 ms |
| A category of 10 | 7 | 0.1 to 0.2 ms |

All's plain key read takes 10 to 16 ms on the same data. On the real
provider only about 14% of channels link to the guide (SPEC 6.1), so the
second row is the realistic one: 8 to 18 ms more than the list, once per
`t`. No flag for now. If ticket 03's category switching in the grid makes
All feel slow, the flag (or `epg_channel_id` in All's indexes, so no
channel row is read) is the next step.

The thinned copy: the 50k database with `epg_channel_id` set to NULL on
six of every seven channels, run against a closed port so no refresh
replaces it.

## Ticket 04: search in the grid

Measured 2026-10-01, release build, the 50k demo with its guide now
running 3 days ahead (about 1.85M programmes): `/` in the grid on All,
30,049 guide channels. From `guide search "…"` in the debug log, which
times the whole keystroke on the UI thread: the match and the new
selection. The rows are built in the next frame, about 20 µs each.

```sh
RUST_LOG=vidicon=debug vidicon --screenshot /dev/null --keys 't /summit<Space>news' 2>&1 | grep 'guide search "'
```

| Query | Matches | Per keystroke, three runs |
|---|---|---|
| `s` | 20,518 | 3.4, 4.9, 3.7 ms |
| `su` to `summit news` | 3,956 to 377 | 2.0 to 4.7 ms |

Within the 16 ms budget (SPEC 10). The match alone is 1.4 to 5.3 ms.

A first version replaced the grid's model with a reset, and a keystroke
took 12 to 21 ms: Slint tears down and rebuilds every visible row on a
reset, about 13 ms for the grid's rows with their programme blocks
against 3 to 4 ms for the list's. The grid now tells Slint which rows
were removed or added and re-reads the rest in place (`Guide::replace_keys`).
The list still resets; at 3 to 4 ms plus its match it stays within budget.

## Ticket 06: performance pass

Measured 2026-10-01, release build, femtovg, the 50k demo with its 3-day
guide (about 1.85M programmes), the grid on All (30,049 guide channels).
`VIDICON_FPS=1` moves the selection down a row every frame;
`VIDICON_FPS=time` (new) moves the time a minute every frame, turning at
the kept window's ends. 25 one-second samples per run.

### Before

With the grid as tickets 02 to 05 left it, on the 144 Hz output:

| Test | fps per second | render p50 |
|---|---|---|
| One row per frame | 78 to 91 | 6.0 to 6.8 ms |
| A minute per frame | 50 to 60, frames up to 69 ms | 12.7 to 14.8 ms |

Ticket 01's build (`9d961b1`) on the same data held 111 to 120 fps at
4.3 ms, so tickets 02 to 05 had added about 2 ms a frame. Where it went,
found by removing one thing at a time:

- Ticket 05's padding, `max(8px, 8px - parent.x)`, kept a block's text in
  view when it began before the span. Any binding on the offset inside a
  block's layout makes Slint lay out every block's text again on every
  time step: 13.5 ms against 2.2 ms with a fixed padding. Limiting it to
  blocks left of the span did not help, as the condition still depends on
  the offset.
- Ticket 02 marked the selected block by re-reading the old and new rows
  on every move, so each move rebuilt two rows' blocks: about 1.7 ms.
- `clip: true` on each rounded block: about 0.9 ms when scrolling.
- Each time the span left the rows' page, every visible row was read
  again and its block model replaced, rebuilding all its blocks: frames of
  20 to 50 ms once a second in the time test.

Not costs: the detail strip (0.1 ms), rounded corners, and how many blocks
lie outside the span.

### What changed

- A block's two texts are placed by hand, not in a layout: only their `x`
  and width follow the offset, which re-lays no text. A title that would
  pass the block's right edge is narrowed rather than clipped, so blocks
  have no clip.
- The selection is a window property (`GuideSel`) that each block
  compares with its programme's start; a selected gap is drawn from it. A
  selection move reads no rows.
- Blocks are placed from an origin fixed when the grid opens, not from the
  page, and each row keeps its block model by channel. A re-read row
  updates the model in place (`sync_blocks`): Slint keeps the blocks it
  already has and builds only those that come or go. The page is now just
  the half hours the span touches.

### After

| Output | Test | Logo cache | fps per second, min / median / max | render p50 | render max |
|---|---|---|---|---|---|
| 144 Hz | one row per frame | warm | 135.7 / 140.2 / 143.3 | 3.41 ms | 8.2 ms |
| 144 Hz | one row per frame | cold | 133.0 / 141.8 / 144.0 | 3.40 ms | 9.1 ms |
| 144 Hz | a minute per frame | warm | 142.0 / 143.9 / 144.3 | 2.38 ms | 10.1 ms |
| 144 Hz | a minute per frame | cold | 141.5 / 143.9 / 144.4 | 2.39 ms | 11.6 ms |
| 60 Hz | one row per frame | warm | 59.9 / 60.1 / 61.1 | 3.6 to 4.8 ms | 10.4 ms |
| 60 Hz | a minute per frame | warm | 60.0 / 60.1 / 60.1 | 4.1 to 4.3 ms | 8.7 ms |

The 60 Hz rows are the five and three seconds the window was visible on
that output; while it was not, the compositor held it to 10 fps.

One row per frame still misses about one frame in fifty: the frame that
builds the new row's blocks. With no blocks at all, rows hold 144.0 at
1.9 ms. Accepted 2026-10-01 (SPEC 10), as for the list: real input does
not build a row on every frame.

### The Skia renderer

Built in alongside femtovg (`renderer-skia`) and picked with
`SLINT_BACKEND=winit-skia-opengl`. `winit-skia` showed a window but never
drew a frame.

| | femtovg | Skia (OpenGL) |
|---|---|---|
| One row per frame, 144 Hz | median 140 to 142 fps, 3.4 ms | 140 to 144 fps, 2.6 ms |
| A minute per frame, 144 Hz | 143.9 fps, 2.4 ms | 144.0 fps, 2.5 ms |
| Launch to first frame | median 208 ms | median 204 ms |
| Resident memory, grid open, idle | 150 MB | 272 MB |
| Binary | 45 MB | 57 MB |

Skia's memory is over SPEC 10's 200 MB, so the app stays on femtovg.

### Launch, idle and memory with the grid open

- Launch to first frame: median 208 ms, 183 to 267 ms over 8 launches.
- Idle with the grid open: 0 CPU ticks in 30 s, 150 MB resident.

A first set of launches came out near 300 ms for every build, ticket 01's
too: the runs above end by `timeout`, so SQLite never closed cleanly and
left a 285 MB write-ahead log, which each launch had to recover. After a
`PRAGMA wal_checkpoint(TRUNCATE)` they were back to the numbers above.
When repeating these, checkpoint between killed runs.

## Recording ticket 03: record marks

Measured 2026-10-01, same data and method as ticket 06, 144 Hz output,
warm logo cache, against the build before the ticket in alternating runs
of 25 one-second samples.

| Test | Before, fps median | After, fps median | render p50 |
|---|---|---|---|
| One row per frame | 138.4, 140.2, 140.9, 139.2 | 139.9, 138.6 | 3.44 to 3.47 against 3.49 to 3.51 ms |
| A minute per frame | 143.6, 143.4 | 144.0, 143.9, 143.7 | 2.40 to 2.41 against 2.40 to 2.41 ms |

No worse in fps, within the spread between runs; the row test renders
about 0.04 ms slower at the median, the marked blocks' extra element and
the title's width binding. A block's mark is a field set
when its row is read, as `airing` is, and an `if` element only on marked
blocks, so nothing in it follows the offset. A first version read each
row's recordings with a query as the row was built: about 1 fps lower in
the row test, every run. The grid now reads the provider's scheduled and
recording ones once per refresh (a handful of rows) and each row looks its
channel up.
