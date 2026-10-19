# Vendored crates

## femtovg 0.27.0

A copy of the crates.io release (upstream commit in
`femtovg/.cargo_vcs_info.json`), used through `[patch.crates-io]` in
`Cargo.toml`, with two changes:

- `src/text/font.rs`: a `Font` keeps one swash cache key (`swash_key`)
  instead of making a new one for every glyph it rasterises. Without it
  swash rebuilds the font's hinting state per glyph, which cost about 100 ms
  of the first frame (`docs/perf/v1.md`, ticket 15).
- `Cargo.toml`: a `[lints.rust]` section at the end that silences two
  warnings, since path dependencies are not lint-capped.

Everything else is as published, `book/` and `tests/` included because the
manifest names targets in them. Don't format or otherwise edit it.

Drop it when a femtovg release has the fix and Slint uses it: delete
`vendor/femtovg` and the `[patch.crates-io]` section, run `cargo update -p
femtovg`, then check launch time as in `docs/perf/v1.md`. When Slint moves
to a newer femtovg without the fix, this copy stops applying (cargo warns
that the patch is unused) and has to be redone on that version.
