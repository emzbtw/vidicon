# 15: Launch time: glyph hinting in femtovg

Status: done. Blocked by: 11.

## Goal

Launch to first frame is reliably under SPEC 10's 300 ms. Today it is
239 to 371 ms (median 290 ms) on the user's machine.

## Context

`docs/perf/v1.md` (Launch). About 100 ms of the first frame is glyph
hinting. femtovg 0.27.0's `Font::swash_font_ref` calls
`swash::FontRef::from_index` for every glyph it rasterises, and that makes a
new `CacheKey` each time. swash's hinting cache is keyed by it, so every
glyph misses and the font's hinting state is rebuilt (skrifa's
`HintingInstance`: the TrueType programs for Inter, the autohinter for an
unhinted font). Slint 1.18.1 and femtovg 0.27.0 are the latest releases.

Keeping one key per `Font` fixes it. Measured with this change in a local
copy of femtovg: 169 to 225 ms over 20 launches, median 198 ms.

```diff
--- femtovg-0.27.0/src/text/font.rs
+++ femtovg-0.27.0/src/text/font.rs
@@ -367,6 +367,8 @@
     glyphs: RefCell<FnvHashMap<GlyphCacheKey, Glyph>>,
     #[cfg(all(feature = "swash", not(feature = "textlayout")))]
     swash_scale_context: Rc<RefCell<swash::scale::ScaleContext>>,
+    #[cfg(all(feature = "swash", not(feature = "textlayout")))]
+    swash_key: swash::CacheKey,
 }
@@ -573,6 +575,7 @@
             metrics,
             glyphs: RefCell::default(),
             swash_scale_context: text_context.swash_scale_context(),
+            swash_key: swash::CacheKey::new(),
         })
@@ -607,7 +610,12 @@
     #[cfg(feature = "swash")]
     pub(crate) fn swash_font_ref(&self) -> Option<swash::FontRef<'_>> {
-        swash::FontRef::from_index(self.data.as_ref().as_ref(), self.face_index as usize)
+        let mut font = swash::FontRef::from_index(self.data.as_ref().as_ref(), self.face_index as usize)?;
+        #[cfg(not(feature = "textlayout"))]
+        {
+            font.key = self.swash_key;
+        }
+        Some(font)
     }
```

## Scope

Decide how to carry the fix, then do it:

- Report it upstream (femtovg) and wait for a release; or
- `[patch.crates-io]` to a fork or a vendored copy until upstream has it; or
- the Skia renderer (ADR 0005 names it as something to try), which replaces
  femtovg's text path.

Switching font does not help: an unhinted Inter runs the autohinter instead,
at the same cost.

## Acceptance

- [x] 20 launches against the 50k provider all under 300 ms, recorded in
      `docs/perf/v1.md` (167 to 247 ms, median 188 ms).
- [x] Text looks the same: screenshots in `docs/screenshots/` unchanged or
      replaced. Same view taken with and without the fix: the only pixels
      that differ are progress bars, by as much as two runs of one build.

## Outcome

Vendored copy with the fix, through `[patch.crates-io]`: `vendor/README.md`.
The repo has no remote, so a fork would have been its first dependency on
one. Not reported upstream yet.
