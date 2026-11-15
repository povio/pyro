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
`crates/pyro-layout/tests/flow.rs`.

`layout/line_break.rs`, `FIT_TOLERANCE`: an atom fits up to 0.0005 device px past the line's end. Blitz rounds a
shrink-to-fit box's width to a layout unit leaving out float noise under 1/100 of a unit, so text within that
noise of a unit wrapped its last word in the box measured for it (`16 - 18 + 20 + 23` in 10pt bold Inter, in any
writing mode). Alignment still uses the exact width. Covered by `shrink_to_fit_text_never_wraps` in
`crates/pyro-render/tests/vertical_writing_mode.rs`.

`layout/style_metrics.rs`, `from_font`: the half-leading of a quantized line is computed in layout units with a
truncating division, as in Chromium. A line height one unit short of the content area (`line-height: 1.2` of 13.33px
Inter, 16 px of ascent and descent) lost a whole pixel above the baseline, in any writing mode. Covered by
`a_line_a_layout_unit_short_keeps_its_baseline` in `crates/pyro-render/tests/vertical_writing_mode.rs`.

`style/mod.rs` (`LineHeight::resolve`, `round_to`), `layout/style_metrics.rs` (`BoxMetrics::from_font`,
`resolve_style_metrics`), `layout/data.rs` (`LayoutData::quantum`), `builder.rs`, `layout/line_break.rs`: a quantized
layout rounds the font's ascent, descent and line gap to whole CSS pixels (the layout's scale in layout units), not to
whole layout units, and floors the half-leading to whole CSS pixels, as Blink does at any device scale (`FontMetrics`
rounds them, `FontHeight::AddLeading` floors the leading). At deviceScaleFactor 3 a `line-height: normal` line of
14px system-ui was 16.33px tall (49 device px) where Chromium's is 17px, and a 13px Arial one 14.67px for 15px; text
under an explicit `line-height` sat up to a CSS pixel off Chromium's baseline. At scale 1 nothing changes. Covered by
`a_normal_line_is_whole_css_pixels_at_every_scale` in `crates/pyro-render/tests/line_height_normal.rs`; repro
`bench/fidelity/repros/system-ui-line-height.html`.

`style/mod.rs` (`LineHeight::resolve`): in a quantized layout a line height that is a length or a number is in
Blink's layout units (1/64 CSS px), as `ComputedStyle::ComputedLineHeightAsFixed` has it: a length rounded to a
unit, a number the font size (rounded to a unit) times the number as a percentage, cut to a unit, in single
precision. Lines of `19px/1.43` text were 27.17px apart where Chromium's are 27.15625px, so the fourth line's top
fell on the other side of a half pixel and its text, painted from a whole CSS pixel (blitz-paint), a pixel off.
Not matched: Stylo keeps font sizes in 1/60 px, so an `em` or `%` line height of a size that is not a multiple of
1/60 px (`11.11px`) can be a unit off. Covered by `a_number_line_height_is_cut_to_a_layout_unit` in
`crates/pyro-render/tests/text_line_tops.rs`; repro `bench/fidelity/repros/text-line-tops.html`.

Pagination scripts clone rows and paragraphs into pages, so the same text is built again and again
under new elements. On the `cmr-overflow` PDF case Blitz's inline construction went from 222 to 107
ms and pagination from about 597 to 485 ms per run; the PDF is byte-identical. pyro-render's
`identical_text_keeps_each_elements_colour` covers the brush swap.

`LineMetrics::unquantized_block_min_coord` and `unquantized_block_max_coord` (`src/layout/line.rs`,
`layout/line_break.rs`) keep a line box's position before quantization: pyro-render decides page cuts on
it, as Chrome does, while painting stays on the quantized coordinates. Quantized line boxes overlap by
up to 1 px, and rounding their tops loses what decides whether a line fits. Covered by
`fractional_line_heights_break_as_in_chrome` in `crates/pyro-render/tests/pdf_fidelity.rs`.

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
`crates/pyro-render/tests/line_break_resume.rs`.

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
`unicode-range` comes from fontique (`vendor/fontique/README.pyro.md`), whose charmaps
the selector's coverage check reads limited to it.

`shape/mod.rs`, `shape_text`: each item is shaped with `ShapeOptions::point_size`, its font size
divided by the layout's scale, so that size-specific tracking (the `trak` table) follows the CSS size
and not the device size (see `vendor/parley_engine/README.pyro.md`, "Tracking at the unscaled size").

`LayoutContext::set_line_break_override` (`context.rs`, `builder.rs`, `build_cache.rs`): a `'static`
line-break override every builder of the context starts with (a builder's own
`set_line_break_override` still replaces it). Layouts built with it stay in the build cache: its
address is part of the build key (0 without an override), so a context whose override changes never
gets a layout broken under the other. An override given only to a builder still bypasses the cache.
blitz-dom sets it from `DocumentConfig::line_break_override`. Covered by
`the_override_is_part_of_the_build_cache_key` in `crates/pyro-render/tests/hyphen_digit_breaks.rs`.

Span edges (`inline_box.rs`, `resolve/tree.rs`, `builder.rs`, `layout/line_break.rs`, `layout/data.rs`,
`layout/line.rs`, `layout/layout.rs`): the margin, border and padding of a CSS inline box that is not
atomic (a `<span>`) take room in its line. `InlineBoxKind::SpanStart` and `SpanEnd` are an edge of such a
box: its `width` is the side's margin, border and padding, it adds nothing to the line's height, and its
positioned box is as tall as the span's content area on the line (`Line::span_content_area`, the ascent
and descent of the style's first font; `PositionedInlineBox::style_index`, `Layout::style_parent`). An
edge is no content of its own. The start edge stays on the line of the content after it (a wrap
opportunity before that content is one before the edge, `LineState::after_span_start`,
`wraps_before_item`; the content widths follow, `wraps_before`), the end edge on the line of the content
before it. White space collapses through an edge as if it were not there (`tree.rs`,
`collapse_after_edge`): space before a start edge is outside the span and the space after the edge
collapses into it; space kept before an end edge is inside the span, so the edge moves behind it
(`deferred_edges`, `edge_moves`, `TreeBuilder::finish_tree`); white space before an edge does not hang.
`TreeBuilder::scale` gives the builder's scale to the caller. blitz-dom pushes the edges
(see its README). An edge is an inline box to the build cache and to line resume, which both leave out paragraphs with
inline boxes: a paragraph with an edge is built and broken anew each time. On a reordered line
the edges are put on their sides (`place_span_edges`, below). Covered by
`crates/pyro-render/tests/inline_box_decorations.rs`.

`style/mod.rs` (`TextStyle::annotation_space`, `AnnotationSpace`), `resolve/mod.rs`, `layout/style_metrics.rs`: room a
span needs beyond its content area, over and under it, for what is drawn there (ruby annotations, emphasis marks;
blitz-dom measures it). A span box's line-height-expanded extents grow to the content area plus that room where they
are smaller (CSS Ruby 1, 4.4), so the line box holds the annotations. It is part of the resolved style, and so of the
build cache's key. Zero by default: nothing else changes. Covered by
`emphasis_marks_and_annotations_take_room_in_the_line` in `crates/pyro-render/tests/text_emphasis_ruby.rs`.

`resolve/tree.rs` (`push_bidi_control`, `push_empty_box_strut`, `deferred_controls`, `resolve_unspaced_style_id`),
`builder.rs`: blitz-dom pushes the bidi formatting characters an inline element's `unicode-bidi` stands for (U+202A to
U+202E, U+2066 to U+2069) and the strut of an empty inline box (U+2061) as text of the current span, in its style
without letter or word spacing. They are no content to white space processing: white space collapses across them,
white space pending before a control goes before it when kept, and white space pending at the end of the text is
dropped (`finish`). `inline_box.rs` (`InlineBox::rtl`): a span edge says whether its span is right-to-left.

`analysis.rs`, `shape/mod.rs` (`inline_box_levels`): atomic and out-of-flow inline boxes take part in bidi resolution
as U+FFFC (`AnalysisOptions::objects`, parley_engine) and get the level they resolve to there (each had the level of
the text before it); a span's start edge takes the level of what follows it and its end edge that of what precedes
it.

`layout/line_break.rs` (`reset_trailing_whitespace_level`): UAX #9 rule L1 on each line: its trailing white space
and zero-width formatting characters get the paragraph's level (a text item is split at their start), so a
right-to-left line's trailing space is not reordered into the line. `place_span_edges`: on a reordered line, and on a
line with a right-to-left span's edges, each span edge goes beside its span's content there, on the side CSS 2.1
§ 8.6 gives it (a right-to-left span's start edge on its right); reordering had moved each edge with the level it
resolved to.

`layout/spacing.rs`, `layout/data.rs`: letter spacing adds no gap after an atom with no advance that is not white
space (a bidi formatting character, a zero-width joiner), as in browsers.

All covered by `crates/pyro-render/tests/unicode_bidi.rs` (and `block_in_inline.rs` for the strut).

`layout/line_break.rs` (`BreakerState::item_index`): the index of the layout item to be laid out next, so that
blitz-dom can tell a float it left for below a line from one past the point where that line wrapped (met again on
the next line; WPT `floats-line-wrap-shifted-001`). Covered by `crates/pyro-render/tests/floats.rs`.

`resolve/tree.rs` (`flush_pending_whitespace`): collapsible white space after an inline box that follows a
preserved segment break is kept (the box is content of the new line): the spaces between inline-blocks after a
`<br>` were dropped (WPT `vertical-align-122`; `crates/pyro-render/tests/inline_leftovers.rs`).

`layout/line_break.rs` (`place_span_edges`): a text item without an advance (a bidi formatting character written
in the text, which keeps the level before it) is no content an edge goes beside: a span starting with U+202E had
its start edge left at that character, away from the content the override reordered (WPT `bidi-011`).

## `text-box-trim` (WPT round 3, lane I)

- `src/style/mod.rs`, `src/resolve/mod.rs`: `TextStyle::text_box_trim` (`TextBoxTrim`, `TextEdge`): the font
  metric each side of a span box's content area is trimmed to.
- `src/layout/style_metrics.rs`: `StyleMetrics` keeps the cap height, the ideographic em box and character face
  (the font's `BASE` table through `parley_engine::IdeographicBaselines`, else derived from the em box) and the
  trimmed content edges (`content_over`, `content_under`); `StyleMetrics::text_edge`.
- `src/layout/line.rs`: `Line::span_box_area` (the content area as the span's box has it, trimmed) next to
  `span_content_area` (the font's, which text decorations go by), and `Line::span_text_edge` (a metric edge of a
  span box on the line: what a block's `text-box-trim` trims its first and last lines to).
- `src/layout/line_break.rs`: `LineBreakData::text_end`, by which the inline layout finds the `<br>` that ended a
  line (its `clear`).

Covered by `crates/pyro-render/tests/text_box_trim.rs`.

## Updating or dropping the patch

Copy the published crate from `~/.cargo/registry/src/*/parley-<version>` and re-apply
`UPSTREAM.patch`, or delete this directory and the `[patch.crates-io]` entry when upstream caches
builds itself.
