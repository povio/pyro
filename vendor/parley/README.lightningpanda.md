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
Layouts with inline boxes or a line-break override are not cached. Bounded at 4096 layouts, emptied
when full; `LayoutContext::clear_build_cache` empties it, and blitz-dom calls it when fonts or the
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

`GlyphRun::atom_texts` (`src/layout/line.rs`) gives each atom's source text range and glyph count
in glyph order, so PDF output can map a ligature glyph to all the characters it stands for.

## Updating or dropping the patch

Copy the published crate from `~/.cargo/registry/src/*/parley-<version>` and re-apply
`UPSTREAM.patch`, or delete this directory and the `[patch.crates-io]` entry when upstream caches
builds itself.
