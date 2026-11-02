# parley_engine 0.12.0, patched

The published `parley_engine` 0.12.0 crate (crates.io; part of
[linebender/parley](https://github.com/linebender/parley)), used through `[patch.crates-io]` in the
workspace `Cargo.toml`. License: Apache-2.0 OR MIT, as upstream. `.cargo_vcs_info.json` and
`Cargo.toml.orig` were left out of the copy.

## The change

`UPSTREAM.patch` is the whole diff.

`shape/shaped_text.rs`, `FontMetrics::from_skrifa_metrics`: Times, Helvetica and Courier get
WebKit's and Chromium's (macOS) extra ascent, 15% of their rounded ascent plus descent, rounded.
These are macOS's default `serif`, `sans-serif` and `monospace` fonts, so every page without an
explicit font family had shorter lines than in Chrome (`line-height: normal` at 16px: 16px instead
of 18px), and taller documents paginated differently.

`shape/shaper.rs` and `shape/cache.rs`: a glyph cache. Each shaped run is kept with its shaping
inputs (font instance, size, variations, features, direction, script, language, the run's text and
up to five characters of context on each side, harfrust's context length), and a run with the same
inputs reuses those glyphs instead of shaping again. The `Shaper` lives in parley's `LayoutContext`,
which blitz-dom keeps per document, so content cloned or laid out again (pagination scripts clone
rows and paragraphs into pages) is shaped once. Bounded at 4096 runs, emptied when full. On the
`cmr-overflow` PDF case 79% of the text shaped was identical to text shaped before; with the cache
pagination went from about 925 to 687 ms, and the PDF is byte-identical.

With `std` the glyph cache is one per thread rather than per `Shaper`, shared by every document laid
out on the thread (as Chromium's shape caches are): its keys hold the font's blob id and the run's exact
inputs, so it serves a document only the glyphs of the very same font data. blitz-dom interns web font
files per thread so documents loading the same font share its blob. Bounded at 16384 runs. On the
`cmr-overflow` PDF case, laid out once per document after a reload, pagination went from about 460 to
388 ms per run (interleaved A/B, load average 27-75); the PDFs of the long cases are byte-identical.
