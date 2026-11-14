# blitz-paint main (c27b4ca), patched

`packages/blitz-paint` of DioxusLabs/blitz main at commit `c27b4ca14158dc457f35c78eb32b8c4fec906b7e` (see
`vendor/blitz-dom/README.pyro.md`), MIT/Apache-2.0. `UPSTREAM.patch` (applies with `git apply`
from the root of an upstream checkout at that commit) is the whole diff.

- `src/text.rs`: paint non-atomic inline ancestor backgrounds behind nested text,
  from outermost to innermost; stop at block/atomic boxes and skip contents boxes.
  Upstream contribution: `xban1x:codex/inline-ancestor-backgrounds` in DioxusLabs/blitz.
- `src/render/border.rs`: skip zero-width border edges (Tailwind's preflight
  `border: 0 solid`); their degenerate polygons cover nothing in a raster but
  print as hairlines in PDF output.
- `src/render/box_shadow.rs`: an outset shadow without blur (a spread ring, such
  as Tailwind's `ring` or a spotlight's `box-shadow: 0 0 0 6000px`) is filled as
  a rounded rectangle path, antialiased, instead of with vello's blurred-rectangle
  paint, which evaluated `powf` for every pixel it covered (and, at a standard
  deviation of 0, drew aliased edges): a ring covering a 3840x2160 frame cost 60
  of the frame's 88 ms of CPU time. The geometry (the border box grown by the
  spread, its average radius) is upstream's.
- `src/debug_overlay.rs`: use the real root-element predicate for fixed boxes;
  adapted from https://github.com/DioxusLabs/blitz/pull/1224.
- `src/lib.rs`, `src/text.rs`, `src/render.rs`: `paint_scene_recording_text` paints like
  `paint_scene_for_print` and also returns each text glyph run it drew (`TextRun`: the element,
  the glyph count and a `glyphs_fingerprint` of the glyphs as drawn, the source text and each
  glyph's range in it, the content area). Only drawn runs are recorded (zero-size text is not
  drawn): the PDF matches each drawn glyph run to the next recorded one. PDF output uses it for
  each glyph's text on page boxes and paper cuts (ligatures, glyphs OpenType features
  substituted), and for link areas and tags on paper cuts. `paint_scene`/`paint_scene_with`
  record nothing.

- `src/render.rs`: `overflow` on `body` does not clip when it is the viewport's.
- `src/render.rs`, `src/text.rs`: vertical writing modes. The inline layout stays logical (x inline,
  y block), and each of its lines is painted through a quarter turn (`draw_vertical_inline_layout`):
  backgrounds, decorations and selection go through the same transform, so `TextRun` bounds, link
  areas and PDF glyph runs need nothing else. A line starts at its unrounded inline position; the box
  is at whole pixels (Chromium rounds text only in the block axis). `VerticalLine` centers the runs of
  `mixed`/`upright` text on the line's central baseline (`sideways` keeps the alphabetic one) and stands
  CJK and `upright` runs upright, one glyph at a time around its em box. Not done: shaping in the
  vertical direction, so `vert` forms (CJK punctuation) are missing and upright Latin keeps its
  horizontal advance. Covered by `crates/pyro-render/tests/vertical_writing_mode.rs`.

- `src/lib.rs`, `src/render.rs`, `src/text.rs`: `paint_scene_for_print` and `paint_scene_recording_text` (PDF) draw an
  inline root's text at its layout-unit x; boxes stay on whole pixels, and so do link areas (`TextRun::bounds`). Chromium
  keeps the fraction in the inline axis (37.781 px of padding: its text at 37.781, ours was at 38). Screenshots
  (`paint_scene`) are unchanged: `text_shift` stays 0 there.

- `src/render/border.rs`: `draw_collapsed_edges` paints the winning borders of a collapsed table without uniform ones,
  snapped to whole pixels of the page as Chromium does. Cells and tables in it draw no border of their own.

- `src/render.rs` (`paint_top_layer`, `paint_backdrop`): the document's top layer (blitz-dom's
  `top_layer_nodes`) is skipped in tree order and painted after the root element, bottom first, each element over
  its `::backdrop` (background color, opacity, `backdrop-filter` over the painted area), positioned as a box hoisted
  to the root element. With an empty top layer the only cost is one emptiness check per box. Covered by
  `crates/pyro-cdp/tests/engine_cdp.rs` (`top_layer_*`).

- `src/render.rs` (`render_node`): each level of the paint recursion runs through blitz-dom's `util::ensure_stack`,
  which grows the stack on demand, so a tree of any depth paints instead of overflowing the thread's stack and
  aborting the process (see blitz-dom's "Deep trees grow the stack on demand").

- Fidelity fixes from the ToS screens (`bench/fidelity`; see blitz-dom's section of that name):
  - `src/render.rs` (`draw_text_input_text`): an empty `<input>`/`<textarea>` paints blitz-dom's placeholder
    layout in its `::placeholder` colour, a single line centred in the content box like the value and placed by
    `text-align` (`InputPlaceholder::line_start`), under the caret.
  - `src/render.rs` (`draw_inline_layout`, `truncate_line`), `src/text.rs` (`TextCut`), `src/lib.rs`
    (`TextRun::new`): `text-overflow: ellipsis`. Each line that overflows the content box is cut at an atom
    boundary where blitz-dom's shaped "…" still fits at the end edge (the left one for right-to-left lines),
    and the ellipsis is drawn next to the last atom kept, on the line's baseline, with the inline root as its
    brush. `stroke_text` leaves out the cut glyphs, decorations span what is kept, and a recording paint
    records the kept glyphs with their source text. Covered by `crates/pyro-render/tests/text_overflow_ellipsis.rs`.
  - `src/render.rs` (`draw_svg`): an inline `<svg>` without a `viewBox` is drawn from blitz-dom's
    `SvgImageData::sized` tree for its content box.
  - `src/lib.rs` (`paint_scene_with`, `CORE_TEXT_GLYPHS`), `src/text.rs` (`stroke_text`): text is placed as
    Chromium places it. Screenshots draw an inline root's text, and a text input's value, placeholder and caret
    (`draw_text_input_text`), at its layout-unit x (`fractional_text_x`, which print already used) instead of its
    box's whole pixel, and on macOS glyphs are unhinted (Core Text does not
    hint) and their origins snap to quarters of a device pixel along the baseline (Skia's subpixel positions)
    where a run is drawn only translated into a raster; a recording paint (the PDF's) keeps the exact x. Covered
    by `screenshot_text_keeps_its_layout_unit_x` and `glyphs_sit_at_quarter_pixels_on_macos` in
    `crates/pyro-render/tests/render.rs`.
  - `src/text.rs` (`synthetic_bold`, `skia_mac_luminance_level`): a run whose font synthesis asks for bold (no
    face of the weight asked for; fontique's rule, see its README) is drawn emboldened by Skia's fake bold amount,
    as in Chromium (a stroke of the text size times 1/24 at 9px and below, 1/32 at 36px and above, by the CSS
    size; half of it per edge); the advances stay. On macOS each edge is then drawn thinner by what Skia's mask
    gamma takes off text it renders from a path, a constant in device pixels at every size by the text colour's
    luminance level (0.26, 0.175, 0.085, 0 from black to white; measured on 49 colours at device scales 1 to 3,
    the level being Skia's canonical 3-bit luminance of the colour). anyrender_vello_cpu's patch brings the amount
    to glifo's outline units; without it an unhinted run got none. Covered by
    `a_missing_bold_face_is_synthesized`.
  - `src/render.rs` (`draw_inline_boxes`): an inline box that is not atomic paints as a box does (outer
    shadow, background, inner shadow, border with its radii, outline), where only its background was painted,
    as wide as its glyphs. It has one fragment per line it has content on, from the start of its first
    content there to the end of its last, its own edges (blitz-dom's span edges, without their margins) among
    them, as tall as its content area plus its block-axis padding and border. The first fragment has the
    start side and the last the end side; the sides between are open (`box-decoration-break: slice`). A side
    without a border style has no border. A fragment's edges are snapped to whole CSS pixels at any device
    scale, as Chromium snaps a box's decorations under an emulated device scale factor (and as block boxes
    are laid out). Covered by `crates/pyro-render/tests/inline_box_decorations.rs`.
  - `src/render/border.rs` (`draw_dotted_dashed_sides`, `straight_side_marks`, `dash_pattern`, `dot_period`,
    `CenterLine`), `src/kurbo_css/css_box.rs` (`border_ring`): dotted and dashed sides have Blink's marks
    (`StyledStrokeData`, `GraphicsContext::DrawLine`, `BoxBorderPainter`), where dashes were a fixed share of a
    side between mitred corners and dots were round at every width. In whole CSS pixels: a dash is twice the
    width from 3px and three times it below, the gap one width and two, the gap then fitted to the length
    (`SelectBestDashGap`); a dotted line up to 3px has square dots and gaps of its width with a dot at each end
    (`EnforceDotsAtEndpoints`), a wider one round dots a fitted distance apart; a line of two dashes or less is
    solid. On square corners a side runs from corner to corner and lies over its neighbours there (mitred only
    against a neighbour of another style or colour; corner dots drawn once). On rounded corners all sides
    stroke one closed line, inset by half the width floored to CSS pixels, starting after the top left corner,
    clipped to each side (to the ring when all sides are alike): the pattern is sized by the length Skia
    measures at half a CSS pixel's tolerance cut to a whole number, and laid out along the chords Skia's dash
    effect measures at half a device pixel's (linear in the curve parameter inside a chord), which is where
    Chromium's dashes fall to 0.05px.
  - `src/render/border.rs` (`draw_outline`, `outline_bounds`), `src/render.rs` (`render_element`,
    `draw_inline_boxes`): an outline is drawn as a border of its style (dotted and dashed as above, the others
    solid) around the border box moved out by `outline-offset` plus its width, where it was always solid, on
    the border box whatever its offset, and under the box's background. As in Chromium the width is floored to
    whole CSS pixels (at least one), the offset cut to whole pixels and no further in than half the box, a
    rounded corner's radius grows by the distance (a square one stays square), and the outline is painted
    after the box's content, with the element's opacity. Not matched: one outline around the fragments of a
    wrapped inline box (each fragment has its own), `outline-style: auto`, and a positioned descendant
    painting over its ancestor's outline. Covered by `crates/pyro-render/tests/dotted_dashed_lines.rs`.
  - `src/lib.rs` (`snapped_line_top`), `src/text.rs` (`LineTops`, `DrawTextContext::line_tops`, `stroke_text`),
    `src/render.rs` (`text_placement`, `draw_inline_layout`, `draw_inline_boxes`, `draw_text_input_text`,
    `draw_marker`): a line of text is painted from a whole CSS pixel, as Chromium paints it at any device scale:
    its top in the document, in layout units, rounded half up. parley rounds a line's top to a device pixel inside
    its layout, and the layout's fractional offset in its box (a vertically centred cell's, an input's) was added
    as it was, so lines of a fractional pitch (`21.5px`, `1.43` times 13px) and the text of centred cells and
    inputs of an odd height sat a device pixel off at a scale of 2 or 3. Each line moves by its own amount, with
    its selection, its ellipsis and its inline boxes; an outside list marker follows the first line, a
    single-line input's value and placeholder their line, a textarea's lines each their own. Where text is placed
    at its layout-unit x (`fractional_text_x`). Not moved: a textarea's caret and selection. Covered by
    `crates/pyro-render/tests/text_line_tops.rs`; repro `bench/fidelity/repros/text-line-tops.html`.
  - `src/render.rs` (`draw_marker_symbol`, `text_placement_of`), `src/text.rs` (`text_fill_color`): an outside
    `disc`, `circle` or `square` marker is a shape sized from the item's font, as Blink paints it
    (`ListMarker::RelativeSymbolMarkerRect`; blitz-dom lays the marker out as a line of that font): with the
    font's ascent `a` in whole pixels and `o = a * 2 / 3`, a
    square of `(o + 1) / 2` px, `o + 7` px before the item's border box and `a - 3 * (a - o) / 2` px above the
    baseline of the item's first line (its own or the first box's in it), at whole CSS pixels; a circle is a
    1px line on the square's edge. It was a glyph of a bullet font, up to 2px right of Chromium's for 13-20px
    text and 4px for 24px, and of another size. Other marker types and inside markers stay text. Covered by
    `a_list_markers_symbol_is_a_shape_sized_from_the_font_at_whole_css_pixels` in
    `crates/pyro-render/tests/text_line_tops.rs`.

- Paint gaps from the ToS feature videos (Chromium 147):
  - `src/render.rs` (`render_element`, `paint_element`, `paint_effect_group`), `src/bounds.rs`: `opacity`, `filter`
    and `backdrop-filter` apply to everything an element paints, as one group (Chromium's effect node): its outset
    box-shadow, its box, its content and descendants (positioned ones outside its box too: it is their stacking
    context) and its outline, all inside its `clip-path` and `mask`, which clip the shadow and the outline as in
    Chromium. The group is recorded first, and its layer covers what it painted (`scene_bounds`: its commands'
    bounds, grown by the filter's reach, cut to the area its ancestors leave visible), where the layer was the
    border box: the outset shadow was drawn outside it at full strength, and descendants outside the border box
    were cut off (a zero-size wrapper fading a popover painted nothing). A group of one shape in one solid colour
    (a spotlight ring) takes the opacity into its colour and gets no layer (`fold_opacity`), as Skia folds a
    layer's alpha into its single draw. The outline has no layer of its own any more. Covered by
    `crates/pyro-render/tests/opacity_group.rs`.
  - `src/filters.rs`, `src/render.rs`: filter lengths are converted to device pixels (the layers are drawn without
    the device scale in their transform), and a `drop-shadow()` without a colour has the element's `color`.
    Chromium takes drop-shadow's third length as the blur's standard deviation (measured), where `text-shadow` and
    `box-shadow` take half of it. anyrender_vello_cpu applies `blur()` and `drop-shadow()` (see its README).
    Covered by `crates/pyro-render/tests/filter_drop_shadow.rs`.
  - `src/text.rs` (`TextShadow`, `draw_shadows`, `stroke_text`, `flush_line_decorations`): `text-shadow`. Each glyph
    run draws its shadows, the last first, right before its glyphs (Chromium paints a text fragment's shadows right
    before it): the glyphs in the shadow's colour moved by its offset (the text's own colour does not matter:
    transparent text casts one), and a shadow with a blur radius inside a layer with a `blur()` of half the radius
    (Chromium's `BlurRadiusToStdDev`). A line's decorations cast the shadows of the first run they cover, each line
    right after its shadows. Not drawn in a `background-clip: text` mask, in a recording paint (the PDF matches each
    glyph run to the text run recorded for it by its glyphs, which a shadow has too), nor in vertical writing modes
    (a shadow's offset is physical, and the lines are turned). Covered by `crates/pyro-render/tests/text_shadow.rs`.

- Text decorations, emphasis marks and ruby (WPT reftest lane G):
  - `src/text.rs` (`flush_line_decorations`, `decoration_thickness`, `whole_css_px`, `StrutGeometry`): decoration
    lines are placed as Chromium places them, in whole CSS pixels from the baseline at any device scale (its
    `TextDecorationInfo`, measured row for row on seven fonts at nine sizes, at scales 1 and 3): an underline
    `max(1, ceil(t / 2))` below the baseline (`t` the thickness: `auto` a tenth of the font size, a length floored,
    `from-font` rounded), at the baseline plus `text-underline-offset` when one is given, under the rounded descent
    for `under`; an overline resting on the rounded ascent (it was the OS/2 `usWinAscent`, several pixels high for
    `system-ui`); a line-through centred two thirds of the ascent down from the text's top, rounded half down. A line
    is `t` cut to whole CSS pixels thick. The inline root's decorations (and those of a block it is in), where the
    root has no text of its own on the line, use the root's font (the line's strut), not a descendant run's.
  - `src/text.rs` (`stroke_text`, `RunPaint`, `DecorationPhase`, `draw_shadow`): each line is painted in passes over
    its glyph runs (collected once): the decorations and what each run is drawn with, then underlines and overlines,
    the glyphs, and line-throughs (CSS 2 Appendix E; they were all drawn after the glyphs). Where a line's decorations
    cast text shadows, the shadows go under all the line draws, a layer per shadow (WPT
    `text-shadow/standards-decor-noblur*`); otherwise each run's shadows go right before its glyphs, as before.
  - `src/text.rs` (`propagated_decorations`, `own_decoration`, `stops_propagation`): a block's decorations are drawn
    on the text of its in-flow descendants (they were drawn only on the text of the inline root that set them):
    an inline root takes those of its ancestors up to an out-of-flow box or an atomic inline (`display: contents`
    passes them through), by DOM parent, cached per node slot for the paint.
  - `src/text.rs` (`ink_gaps`, `glyph_intercept`, `is_cjk_char`): `text-decoration-skip-ink` (`auto`, the initial
    value, and `all`), as Blink's `TextPainter::ClipDecorationLine` does it: an underline or overline leaves a gap
    around the ink of each glyph inside the line's band (less half a pixel at each edge; Skia's `getIntercepts`, from
    the glyph's outline), grown by the thickness (at most 13px) on each side, not around CJK glyphs for `auto`, its
    edges on whole device pixels. Intercepts are cached by font, size, glyph and band. Line-throughs skip nothing.
  - `src/text.rs` (`decoration_ink_reach`), `src/render.rs` (`render_element`): an inline root outside the clip
    rectangle is still painted when its own decorations reach into it (`text-decoration-thickness: 4em`).
  - `src/text.rs` (`draw_text_annotations`, `draw_annotation`, `draw_glyph_run`), `src/render.rs`
    (`draw_inline_layout`, `draw_vertical_inline_layout`, `draw_text_mask`): blitz-dom's `TextAnnotation`s. A ruby
    annotation is drawn over the part of its base on the line the base starts on, an emphasis mark over each character
    of the element's glyph runs that takes one: the annotation's line centred over the base (white space left out),
    the bottom of its content area on the top of the base's (the span content area of the base's style), or its top on
    the bottom for `under`; an annotation of a base that already has one on that side goes outside it (nested ruby,
    marks outside ruby). Annotations turn with their line in vertical writing modes, and a `background-clip: text`
    mask holds them.
  - Covered by `crates/pyro-render/tests/text_decoration_lines.rs` and `text_emphasis_ruby.rs`.

The linked blitz-dom patch supplies `Node::is_root_element`. Rendering regressions
are covered in `crates/pyro-render/tests/render.rs`; persistent-tree behavior is
covered by `crates/pyro-render/tests/mirror.rs`.

## Moving from 0.3.0-beta.4 to main

- `src/render.rs`: main computes clipping in `should_clip_node`; `overflow` on `body` that is the
  viewport's is left out there. Print's `backgrounds` switch also guards main's `paints_own_box` branch
  (#1319).
- `src/render.rs`: main draws inline text from the unrounded content box origin (#1382); the offset stays
  zero (see blitz-dom), so screenshots and print are as before. Vertical writing modes return before it.
- `src/text.rs`: glyph runs are recorded (`TextRun`) before main's zero-size check (#1402), and the
  vertical-line and source-text arguments go with main's `TextPaintOptions`; the `background-clip: text`
  mask (#1340) passes none.
- Font embolden (#1357, #1361, #1364, #1444) stays off: pyro enables neither `font-embolden`
  feature.

## From be60597 to c27b4ca

- `src/text.rs`: main skips text whose fill is fully transparent; a recording paint (the PDF's, `runs` given)
  keeps it as invisible text, so it stays selectable and searchable.
