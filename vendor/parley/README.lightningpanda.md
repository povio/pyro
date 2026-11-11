# parley 0.12.0, patched

The published `parley` 0.12.0 crate (crates.io; part of
[linebender/parley](https://github.com/linebender/parley)), used through `[patch.crates-io]` in the
workspace `Cargo.toml`. License: Apache-2.0 OR MIT, as upstream. `.cargo_vcs_info.json` and
`Cargo.toml.orig` were left out of the copy.

## The change

`UPSTREAM.patch` is the whole diff.

`build_cache.rs`, `builder.rs` and `context.rs`: a build cache in `LayoutContext`. Building a layout
(text analysis, shaping, style metrics) depends on the text, the resolved styles and style runs, the
scale and the base direction; the styles' brushes only matter when painting. `build_into_layout`
keys each build on those inputs (brushes left out; resolved families, variations and features by
value) and a build with the same inputs copies the cached `LayoutData` and swaps in its own styles.
Layouts with inline boxes or a builder's own line-break override are not cached (see
`set_line_break_override` below). Bounded at 4096 layouts, emptied when full; `LayoutContext::clear_build_cache` empties it, and blitz-dom calls it when fonts or the
scale change (`invalidate_inline_contexts`).

`layout/line_break.rs`: an in-flow inline box wider than the line at the start of a line is appended
and followed by a soft wrap opportunity, as for a box that fits. Content after it breaks there; a box
ending the paragraph ends its last line. Upstream broke the line right after the box, which left an
empty line below a box that ended the paragraph (Chromium has none). Covered by
`an_atomic_inline_wider_than_its_line_ends_it` and `flow_parity_text_align` in
`crates/lp-layout/tests/flow.rs`.

`layout/line_break.rs`, `FIT_TOLERANCE`: an atom fits up to 0.0005 device px past the line's end. Blitz rounds a
shrink-to-fit box's width to a layout unit leaving out float noise under 1/100 of a unit, so text within that
noise of a unit wrapped its last word in the box measured for it (`16 - 18 + 20 + 23` in 10pt bold Inter, in any
writing mode). Alignment still uses the exact width. Covered by `shrink_to_fit_text_never_wraps` in
`crates/lp-render/tests/vertical_writing_mode.rs`.

`layout/style_metrics.rs`, `from_font`: the half-leading of a quantized line is computed in layout units with a
truncating division, as in Chromium. A line height one unit short of the content area (`line-height: 1.2` of 13.33px
Inter, 16 px of ascent and descent) lost a whole pixel above the baseline, in any writing mode. Covered by
`a_line_a_layout_unit_short_keeps_its_baseline` in `crates/lp-render/tests/vertical_writing_mode.rs`.

Pagination scripts clone rows and paragraphs into pages, so the same text is built again and again
under new elements. On the `cmr-overflow` PDF case Blitz's inline construction went from 222 to 107
ms and pagination from about 597 to 485 ms per run; the PDF is byte-identical. lp-render's
`identical_text_keeps_each_elements_colour` covers the brush swap.

`LineMetrics::unquantized_block_min_coord` and `unquantized_block_max_coord` (`src/layout/line.rs`,
`layout/line_break.rs`) keep a line box's position before quantization: lp-render decides page cuts on
it, as Chrome does, while painting stays on the quantized coordinates. Quantized line boxes overlap by
up to 1 px, and rounding their tops loses what decides whether a line fits. Covered by
`fractional_line_heights_break_as_in_chrome` in `crates/lp-render/tests/pdf_fidelity.rs`.

`GlyphRun::atom_texts` (`src/layout/line.rs`) gives each atom's source text range and glyph count
in glyph order, so PDF output can map a ligature glyph to all the characters it stands for.

`layout/resume.rs`, `TreeBuilder::build_into_keeping_lines` and `BreakLines::resume`: resumed line
breaking. The breaker records its state at the start of each line (`LayoutData::line_starts`; none
when the caller placed boxes, hit a max height or reverted mid-break). Building a layout again with
`build_into_keeping_lines` compares the new shaped data with the old (styles, style metrics, runs,
items, clusters, characters and glyphs, from the start) and keeps the old lines whose break the
breaker decided entirely inside the equal prefix: each line start also records how far the breaker
had read before it (`LineStart::read_end`), and lines are kept up to the last start reached reading
only the equal prefix. (The atom that overflowed a line can lie many lines on, when a word broken at
emergency opportunities runs over lines narrower than the first: a hanging or negative
`text-indent`, or `each-line` after a forced break.) `resume`, called with
the geometry the caller gives lines after the first, continues breaking after the kept lines when
the old break had the same layout max advance, text indent and line geometry; otherwise it starts
over. LTR text without inline boxes only. Blitz builds every inline root this way and resumes
when no floats are involved and the text is not justified, so a paragraph that pagination grows
word by word is broken from its last lines. On `cmr-overflow` the bytes going through the breaker
went from about 240 to 40 per break, with identical PDFs. Covered by the differential tests in
`crates/lp-render/tests/line_break_resume.rs`.

`shape/mod.rs`, `FontSelector` and `layout/style_metrics.rs`, `resolve_style_metrics`: the style
metrics of every style ran a fontique query per layout for the style's first available font, after
the font selector had queried the same families and attributes while shaping. The selector's
charmap probe (`query_yields_font`) visits the fonts in the query's order, so the first font it
visits whose `unicode-range` includes U+0020 is the first available font when it comes from the
style's own families (not the script fallbacks or the emoji family, which the query yields after
them); the probe goes on past the first font with a charmap until it has found that font or left
the style's families. The selector records it per style (`LayoutContext::style_first_fonts`)
and `resolve_style_metrics` takes a style's font from there, querying only a style without one
(its families yield no such font, or it shaped no text). The font is the same `QueryFont` the
query (`Query::first_available_font`) would yield, so the metrics are unchanged. The fonts'
`unicode-range` comes from fontique (`vendor/fontique/README.lightningpanda.md`), whose charmaps
the selector's coverage check reads limited to it.

`LayoutContext::set_line_break_override` (`context.rs`, `builder.rs`, `build_cache.rs`): a `'static`
line-break override every builder of the context starts with (a builder's own
`set_line_break_override` still replaces it). Layouts built with it stay in the build cache: its
address is part of the build key (0 without an override), so a context whose override changes never
gets a layout broken under the other. An override given only to a builder still bypasses the cache.
blitz-dom sets it from `DocumentConfig::line_break_override`. Covered by
`the_override_is_part_of_the_build_cache_key` in `crates/lp-render/tests/hyphen_digit_breaks.rs`.

## Updating or dropping the patch

Copy the published crate from `~/.cargo/registry/src/*/parley-<version>` and re-apply
`UPSTREAM.patch`, or delete this directory and the `[patch.crates-io]` entry when upstream caches
builds itself.
