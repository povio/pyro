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

## Tracking at the unscaled size

`shape/shaper.rs` (`ShapeOptions::point_size`), `shape/cache.rs` (`GlyphKey::point_size`): harfrust
applies a font's AAT `trak` table (size-specific tracking; San Francisco, macOS's `system-ui`, has
one) at the shaper's point size, which was the item's font size. parley shapes a scaled layout at
the font size times its scale, so at a device scale factor of 3 a 14px label was tracked as 42pt
text: +17 font units per glyph instead of -22, 2.6% wider than in Chromium (Core Text tracks at
the CSS size), and every label, button and tab grew with it. The caller now passes the unscaled
size as `point_size` (`vendor/parley`, `shape/mod.rs`); the glyph cache keys runs by it too, since
two layouts can shape the same scaled size at different point sizes. Measured with
`bench/fidelity/repros/system-ui-tracking.html`: 10-24px widths equal Chromium's to 0.02px at scale 2
(before: -6% to +7%). The tests in `atom.rs` and `shaped_text.rs` and the crate doc example set the
new field (they need assets the vendored copy does not have, as before).

## Chromium's hyphen-before-digit break on its own

`break_overrides.rs`, `HYPHEN_DIGIT_LINE_BREAK_OVERRIDE`: the hyphen-digit rule of
`CHROMIUM_LINE_BREAK_OVERRIDE` without the rest of Chromium's ASCII table. A break between `-` and
an ASCII digit is allowed when the character before the hyphen is an ASCII letter or digit
(`4b87-|916e`, `10-|20`); every other pair, including a minus sign after a space or at the start of
the text, is left to ICU (UAX #14 LB25 keeps `HY × NU`). It only adds opportunities, so
`line-break: anywhere` is unaffected. pyro-render uses it for `serve --hyphen-digit-breaks on`.

## Inline boxes and paragraphs in bidi resolution

`analyzer.rs` (`AnalysisOptions::objects`), `analysis.rs` (`object_levels`): the caller (parley's `analysis.rs`)
names the indices of its objects (atomic inline boxes), which are resolved as U+FFFC OBJECT REPLACEMENT CHARACTER
(class ON) at those indices, as in browsers, and whose levels it reads back; they were left out, so the neutrals
around a box resolved as if it were not there. Each paragraph, up to and including a paragraph separator (class B:
a preserved line feed, U+2029), is resolved on its own (UAX #9 P1); the whole text was one paragraph. The layout's
paragraph level is the first paragraph's. Covered by `crates/pyro-render/tests/unicode_bidi.rs`.

## Ideographic baselines (WPT round 3, lane I)

`src/shape/shaped_text.rs`: `IdeographicBaselines::from_font_instance` reads `ideo`, `idtp`, `icfb` and `icft` of
a font's `BASE` table (horizontal axis, the default script's values), for `text-box-edge: ideographic` and
`ideographic-ink` (WPT `css-inline/text-box-trim`, the BaselineDiagnostic fonts).
## Tabs

`shape/shaper.rs`: a tab is shaped as a space (browsers do the same): a font without a glyph for U+0009 (Ahem) drew
its missing-glyph box. `shape/shaped_text.rs` (`ShapedText::set_cluster_advance`): parley's line breaker sets a
tab's advance to the tab stop it reaches (parley's README.pyro.md, "Tab stops"). Covered by
`crates/pyro-render/tests/tab_stops.rs`.

## Soft hyphens

`shape/shaped_text.rs`: `RunMetrics::hyphen` / `ShapedRun::hyphen` (the glyph and advance of the font's U+2010, else
U+002D, kept with the run's cached metrics), `ShapedText::glyph_of` (any other character of a run's font) and
`ShapedText::set_cluster_glyph`; `shape/data.rs`: `CharacterFlags::is_soft_hyphen`. parley's line breaker shows a
hyphen where a line ends at a soft hyphen (parley's README.pyro.md, "Soft hyphens").

## Dictionary line breaks

`analyzer.rs` (`LineBreakConfig::no_dictionary`), `analysis.rs` (`SegmenterKey::no_dictionary`): a range of text
that asks for no dictionary breaks (CSS `word-break: manual`) is segmented by `LineSegmenter::
new_for_non_complex_scripts` although the `complex-scripts` feature is on.

## `@font-face` `size-adjust` and metrics overrides (WPT css-fonts, round 3 lane F)

`shape/shaper.rs` (`shape_run`), `shape/shaped_text.rs` (`FontMetrics::with_face_metrics`,
`FontMetrics::from_font_instance`, `RunMetrics::new`): a font whose `@font-face` rule has `size-adjust`
(`fontique::Synthesis::size_adjust`) is shaped, measured and drawn at the item's size times it (the run's
`font_size` is that size), unless `ShapeOptions::face_size_adjust` is off (`font-size-adjust`); its
`ascent-override`, `descent-override` and `line-gap-override` replace the font's ascent, descent and line gap with
fractions of that size. WPT `size-adjust-01`..`-03`, `size-adjust.tentative`, `ascent-descent-override`,
`line-gap-override`, `font-size-adjust-metrics-override`.

## Face features and variations, automatic `opsz`, `font-language-override` (WPT css-fonts, round 3 lane F)

`shape/shaper.rs` (`shape_run`): a face's `@font-face` features (`fontique::Synthesis::face_settings`) come
before the item's, so the property's win for the same tag; its variation settings come after the automatic `opsz`
(`ShapeOptions::auto_opsz`: the item's first variation) and before the property's (CSS Fonts 4 §7.2). The automatic
`opsz` is the size the face is used at (times its `size-adjust`). The cache keys see the effective variations and
features. `ShapeOptions::language_override` shapes with that OpenType language system (`x-hbot<tag>`, the
private-use subtag harfrust maps to a tag). WPT `font-feature-settings-descriptor-*`,
`font-variation-settings-descriptor-*`, `variations/variable-opsz-size-adjust`, `font-language-override-*`.

## Hiragana and Katakana in one run (WPT css-fonts, round 3 lane F)

`itemize.rs` (`kana`): Katakana is itemized as Hiragana, so kana of both scripts shape in one run (both are
OpenType's `kana` script): the font's kerning and contextual forms reach across `すペ` (WPT
`hiragana-katakana-kerning`). Covered by `crates/pyro-render/tests/font_face_settings.rs`
(`kerning_reaches_across_hiragana_and_katakana`).
