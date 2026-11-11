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

The font is recognised by its own family name (English, else first `name` record), read every time
the metrics are computed, with no memo. These metrics are computed once per font, size and
variation location: `RunMetrics` is kept with the run's glyphs in the glyph cache, and
`FontMetrics::from_font_instance` only runs, once per style and layout, when no shaped run used the style's first available font at its size and variations.
An earlier version kept a thread-local list keyed by the font data's address and ttc index; once a
font's data was freed and another font was allocated at the same address, the new font got the old
font's answer (an extra ascent of about 15% given or withheld), so line heights, document heights
and paint hashes depended on what the thread had laid out before. Do not reintroduce a per-font
memo keyed by an address; if one is ever needed, key it by the blob id, which is never reused.

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

## Run metrics without a font parse per run

`shape/shaped_text.rs`, `RunMetrics` and `ShapedText::push_run`: a run's font metrics (the font's
units per em and its `FontMetrics` at the run's size and variation location) were computed from
scratch for every run, parsing the font (`FontRef::from_index`) twice and reading its metric tables
(`skrifa::Metrics::new`) once, although `ShapedText::fonts` already deduplicates the font instances.
Now a `ShapedText` keeps one `RunMetricsEntry` per font, size and variation location it has runs
for, and a run of a combination seen before takes that entry (one parse for the rest). The glyph
cache (`shape/cache.rs`) stores the metrics of a run beside its glyphs — its key holds everything
they depend on — so a run it serves costs no font parse at all. `shape/shaper.rs` builds the
harfrust shaper (and parses the font) only for a run the glyph cache does not hold; a cached run's
variation coords are the shaper instance's, which are what the shaper would report. Metric values
are unchanged: the same `skrifa` computation runs, once per combination instead of once per run.

## Chromium's hyphen-before-digit break on its own

`break_overrides.rs`, `HYPHEN_DIGIT_LINE_BREAK_OVERRIDE`: the hyphen-digit rule of
`CHROMIUM_LINE_BREAK_OVERRIDE` without the rest of Chromium's ASCII table. A break between `-` and
an ASCII digit is allowed when the character before the hyphen is an ASCII letter or digit
(`4b87-|916e`, `10-|20`); every other pair, including a minus sign after a space or at the start of
the text, is left to ICU (UAX #14 LB25 keeps `HY × NU`). It only adds opportunities, so
`line-break: anywhere` is unaffected. lp-render uses it for `serve --hyphen-digit-breaks on`.
