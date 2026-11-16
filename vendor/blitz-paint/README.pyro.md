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
    records the kept glyphs with their source text. A line too narrow for any atom beside the ellipsis keeps
    its first atom (Blink's `LineTruncator`; `TextCut::end_atom`), and emphasis marks skip the atoms cut
    (`draw_text_annotations`' `cut`). Covered by `crates/pyro-render/tests/text_overflow_ellipsis.rs`.
    A line clamp (`TextLayout::clamp`, blitz-dom's `layout::line_clamp`) ends its line with "…" whether the line
    overflows or not (`line_truncation`, `truncate_line`'s `forced`: after the whole line when the ellipsis
    fits there), whatever `text-overflow` says, and the root's later lines, their annotations and their part
    of a text mask are not painted. Covered by `crates/pyro-render/tests/line_clamp.rs`.
    `draw_text_input_text` ends the overflowing lines of a text control (`<input>`, `<textarea>`) with its
    `TextInputData::ellipsis` the same way while the control is not focused and not scrolled, as Chromium does.
  - `src/render/box_shadow.rs` (`overflow_clip_path`), `src/render.rs` (`render_element`, `paint_content`,
    `draw_text_mask`, `ElementCx::hoisted_child_clips`, `ElementCx::draw_hoisted_children`): the overflow clip. A scroll container clips at its padding
    box, as before. `overflow: clip` on both axes (and `contain: paint`) clips at the box `overflow-clip-margin`
    names, along that box's own curve, grown by the margin's length as a shadow's spread grows its shape (a
    negative length shrinks it); `clip` on one axis with `visible` on the other clips that axis only, with square
    corners. A box hoisted to a stacking context (positioned, with a `z-index`) is painted inside the clips of the
    boxes between it and that context on its containing-block chain: such a box makes no stacking context, so the
    child was painted outside its clip layer and showed whole outside an `overflow: hidden` ancestor. The clip
    layers stay pushed from one hoisted box to the next as far as their clip chains share their outer clips (only
    the differing inner ones are popped and pushed): a clip layer costs the raster its whole area, and a stack of
    them per box (a table's sticky cells, the buttons in its cells, all in one scroller) made a ToS table screen's
    capture cost 50-60% more instructions. A layer the layer limit refused is not popped (popping one per clip
    unconditionally popped the ancestors' layers past the limit). Where the device pixels are known (screenshots:
    `paint_scene`, `paint_scene_for_device` with the capture's transform; `BlitzDomPainter::device_pixels`), each
    hoisted box is recorded first, and when each of its clips covers in whole every pixel the box's ink touches
    (`src/bounds.rs`, `clip_covers_pixels`: a rectangle or rounded rectangle under a scale and translation, the
    pixels inside its box and off its corners' curves; an edge between pixels covers the pixel it cuts only in
    part, so a box flush with such an edge keeps its clips, and its edge stays antialiased by both), none is
    pushed: they cut nothing, and one layer per icon button in its `overflow: hidden` cell still cost a ToS
    finances capture 9-15% more than v0.0.38 (Chromium skips a clip that contains what it clips too). A box one
    clip cuts keeps all of them, so it is composited through the same layers as before. Left out, a box's
    translucent pixels that its own draws overlap round as drawn directly (as before round 3), a level apart in
    places from a clip layer's second 8-bit composite. Every clip stays in a local-raster layer or a 3D plane
    (their own pixel grids) and around a box with a backdrop filter or a blend mode (their layers read what the
    clips hold); printed output pushes them all. Covered by
    `crates/pyro-render/tests/overflow_clip.rs`, `crates/pyro-render/tests/hoisted_clip_layers.rs`, the unit test
    in `src/bounds.rs`, WPT `css-overflow/overflow-clip-margin-*`, `clip-00*`.
  - `src/render.rs` (`replaced_overflow_is_visible`, `should_clip_node`, `draw_image`, `draw_svg`): the content of
    an `<img>`, `<video>` or `<canvas>` whose author made `overflow` visible on both axes is not cut at its box
    (an SVG image only at its own viewport), as in Chromium; blitz-dom's UA sheet gives these elements
    `overflow: clip`, the default, which cuts their replaced content only (the box itself is clipped as before:
    at its padding box when it holds a raster image, not at all otherwise). WPT `css-overflow/overflow-img*`,
    `overflow-canvas`, `overflow-video`.
  - `src/render.rs` (`draw_svg`): an inline `<svg>` without a `viewBox` is drawn from blitz-dom's
    `SvgImageData::sized` tree for its content box, in user units of a CSS px times its `zoom` (its content
    was drawn at zoom 1 in a zoomed box: WPT `clip-path-path-with-zoom-002`). An SVG image (`<img>`, not an
    inline `<svg>`) whose content blends (`anyrender_svg::has_blend_modes`) is drawn in a layer of its own, an
    isolated group: its `mix-blend-mode` blended with the page under it (WPT `mix-blend-mode-in-svg-image`).
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
  - `src/render.rs` (`draw_marker_image`, `marker_baseline`): an outside `list-style-image` marker (blitz-dom's
    `Marker::Image`), as Chromium places it (`ListMarker::InlineMarginsForOutside`): the image at its natural size
    ends 7px before the item's border box, its bottom on the baseline of the item's first line (its own or the first
    box's in it, else the marker's line), at whole device pixels; a raster image is scaled to that size, an SVG drawn
    at its own. Covered by `an_image_marker_ends_7px_before_the_item_on_its_first_baseline` in
    `crates/pyro-render/tests/generated_content.rs`.
  - `src/render.rs` (`draw_marker`, `draw_marker_symbol`, `marker_baseline`): an outside marker moves with the first
    line when floats move that line's start (parley's `inline_min_coord`), as Chromium places it at the line's start:
    beside a float an item's marker stayed by the item's own box, behind the float or off the page (WPT
    `list-style-*-applies-to-017`).
    Covered by `an_outside_marker_goes_with_the_line_a_float_moves` there.

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
    `box-shadow` take half of it. anyrender_vello_cpu applies `blur()`, `drop-shadow()` and the colour functions
    (see its README). Covered by `crates/pyro-render/tests/filter_drop_shadow.rs`.
  - `src/filters.rs`: the colour functions' matrices are Chromium's (`cc/paint/render_surface_filters.cc`), f32 for
    f32: the filter effects spec's `feColorMatrix` equivalents with grayscale's and saturate's last column of each
    row what the row leaves of 1, on sRGB values. They were anyrender's, which pyro's renderer did not apply.
    Covered by `crates/pyro-render/tests/filter_color.rs`.
  - `src/render.rs` (`render_element`, `paint_effect_group`), `src/layers.rs` (`document_effects`,
    `backdrop_root`): `backdrop-filter`. An element with one records two markers for anyrender_vello_cpu (see its
    README): a capture where it starts to paint, before its own layers (its clip-path, mask and blend mode are no
    backdrop root for it), and a draw first in its effect group, so that its opacity and filter apply to the
    backdrop it shows; both clip to its border box with its radii. A backdrop root that none of its layers marks
    (a `clip-path`, a mask, `will-change` of a property that makes one, an animation of one in effect: blitz-dom's
    `animates_backdrop_root`) is painted in a layer with an empty filter,
    for elements with a backdrop filter below them only (the layer costs a composite). An opaque group without a
    filter around a backdrop filter is a clip layer. Covered by `crates/pyro-render/tests/backdrop_filter.rs`.
  - `src/filters.rs` (`reference_filter`): a `url(#id)` filter (stylo parses it now) is the document's SVG
    `<filter>` with that id when that is a chain of `feColorMatrix` (every type) and `feComponentTransfer`, each in
    its `color-interpolation-filters` space (linearRGB unless an ancestor's attribute says sRGB; sRGB for a filter
    in a `display: none` subtree, as in Chromium), when its functions keep transparent pixels transparent.
  - `src/filters.rs` (`filter_steps`, `reference_graph`), `src/render.rs` (`render_element`,
    `paint_filter_steps`): any other `url(#id)` filter is the `<filter>`'s graph (anyrender_svg's `FilterGraph`, see
    its README): usvg parses the filter for a stand-in of the element's border box (the object bounding box, and the
    viewport its percentages resolve against) in `svg_reference_document`'s document, and the graph applies in the
    element's user space (CSS px from its border box's corner) within its filter region. The list then paints as a
    layer per step around the element's group (filter functions in a row stay one CSS chain), the first innermost,
    the opacity around them. A graph fills its region whatever the element paints: an empty element's filter
    region is painted, and so is a `visibility: hidden` element's (its content not), as in Chromium (WPT
    css/filter-effects/empty-element-with-filter-*, visibility-hidden-element-with-filter-001,
    effect-reference-obb-dimensions). A `drop-shadow()` coloured from `currentcolor` (`uses_current_color`, the
    computed colour's structure) before it taints the graph's source (see anyrender_svg's README). Culling takes
    the steps' reach and the graphs' regions (`steps_reach`): a region reaching into view from a box outside it is
    painted, an `feTile` in view keeps the content of its whole region, and an empty `overflow: hidden` element still
    paints its region.
  - `src/bounds.rs` (`layer_bounds`, `stays_with_input`): a layer whose filter can paint where its content is not (a
    graph with a flood, turbulence, a tile, an image, a displacement, morphology or a composite; a colour function
    adding alpha) is bounded by its whole clip, its filter region, so that an ancestor's opacity or filter layer
    keeps an empty element's filter region and a flood past the content.
  - `src/filters.rs` (`svg_filter_notes`, `primitive_notes`), `src/render.rs` (`draw_svg`): an inline `<svg>`'s
    tree is rendered with what the document knows of its filters' primitives that usvg does not keep: which
    `flood-color` or `lighting-color` is computed from `currentcolor` (tainted), which lighting primitive has no
    light source and which is a real `feFlood`. A tree filter is the `<filter>` of its id in the `<svg>` (else in
    the document) whose primitives it has; usvg names the second and later uses of an `objectBoundingBox` filter
    `filterN`, and those take the notes of the uses with the same primitives and colours. A `<filter href>`
    template's primitives are read through the reference. What its filters make past its box is cut off by its
    viewport clip (below). Covered by `crates/pyro-render/tests/svg_filter_primitives.rs`.
  - `src/layers.rs` (`blend_isolation`), `src/render.rs` (`render_element`): a stacking context that holds an element
    with a `mix-blend-mode` (its nearest ancestor that is one, the root element's at the latest) is painted as an
    isolated group, as Chromium isolates one: the element blends with what the context painted, not with what lies
    under it (the page under an absolutely positioned parent, the canvas under the root element's group). The
    contexts are found in one pass over the nodes' styles when the painter is made. Covered by
    `crates/pyro-render/tests/mix_blend_mode.rs`.
  - `src/text.rs` (`TextShadow`, `draw_shadow`, `stroke_text`, `flush_line_decorations`): `text-shadow`. Each glyph
    run draws its shadows, the last first, right before its glyphs (Chromium paints a text fragment's shadows right
    before it): the glyphs in the shadow's colour moved by its offset (the text's own colour does not matter:
    transparent text casts one), and a shadow with a blur radius inside a layer with a `blur()` of half the radius
    (Chromium's `BlurRadiusToStdDev`). Each run under a decoration casts its own shadows on its part of the line
    (lane G2, below), each line right after its shadows. Not drawn in a `background-clip: text` mask, in a recording paint (the PDF matches each
    glyph run to the text run recorded for it by its glyphs, which a shadow has too), nor in vertical writing modes
    (a shadow's offset is physical, and the lines are turned). Covered by `crates/pyro-render/tests/text_shadow.rs`.

- Borders, backgrounds and shadows for the WPT paint reftests (`css/css-backgrounds`, `css/CSS2/backgrounds`,
  `css/compositing`), as CSS Backgrounds 3 and Chromium paint them; covered by
  `crates/pyro-render/tests/borders_backgrounds.rs`:
  - `src/render/border_image.rs` (new), `src/render/border.rs`: `border-image`, drawn instead of the border's styles
    when its image can be drawn (an image that did not load leaves them), as Blink's `NinePieceImageGrid` and
    `NinePieceImagePainter` cut and lay it out: the border image area (the border box grown by
    `border-image-outset`, on whole pixels), the sides' widths (`border-image-width`: numbers times the border
    widths, `auto` the slice, rounded, all shrunk alike and rounded when opposite sides overlap), the slices clamped to the
    image, the corners stretched, the sides and the middle (`fill`) tiled by `border-image-repeat` (`round` resized
    to whole tiles, `repeat` centred, `space` with the space left over around the tiles, none when not one fits).
    Raster images are cut into a brush per piece, so that sampling never reaches the next piece's pixels; a piece
    without spacing is one repeating fill. SVG images (their own size, or the area's) and gradients (the area's
    size) are drawn per tile. blitz-dom loads the `border-image-source` image (`border_images`).
  - `src/render.rs` (`canvas_background_source`, `paint_element`, `paint_scene`), `src/render/background.rs`
    (`draw_canvas_background`): the canvas background (CSS Backgrounds 3 §2.11.2). The root element's background,
    or the body's when the root has none (Blink's `BackgroundTransfersToView`: an HTML root, its first `body`
    child rendered, neither with containment), is painted over the whole canvas by the root element, first (under
    its shadow) and in its effect group (opacity, filters and blend modes apply to it, as in Chromium) but not under
    its own transform, its images positioned against the root element's untransformed boxes; neither element paints
    it again, and their outset shadows are cut out of their boxes (an opaque background no longer covers them). It was the colour alone, painted
    before the document at full strength, and the root and the body painted their backgrounds in their boxes too
    (a translucent root colour twice, images only in the root's box, a `display: none` body's colour on the canvas).
    A root with `visibility: hidden` leaves the colour to `paint_scene`, as before.
  - `src/render/background.rs`: image tiles cover the whole background painting area: a repeating raster image
    one fill over the clip box (it covered the positioning area and the part left and above of the image's
    position, so a `border-box` clip left the right and bottom border areas empty) or over the canvas, and SVG
    images repeat too (drawn once before); `space` repeats its pattern over the painting area and places a single
    copy at its position; an axis that does not repeat clamps the image at its edge instead of wrapping the
    opposite edge's pixels in; `repeat` and `round` stay one repeating fill (seams sampled across, as Chromium's
    image shader samples them), while spaced tiles are drawn one by one, each clamped in both axes (Chromium's
    picture shader of one image per cell). `background-clip: border-area` clips to the area the border paints (`border.rs`
    `border_area_path`: each side with a style and a width, a `double` side's two lines, dotted and dashed marks
    of a box with square corners). `background-attachment: local` on a scroll container positions and clips a
    layer against the scrolled content (the padding box grown to the scrollable overflow, moved by the scroll
    offset), cut to the padding box. `fixed` degrades to `scroll` under an identity transform (`translate(0)`),
    an individual transform property and `will-change: transform` too.
  - `src/render/box_shadow.rs`: a spread shadow's corner radii grow by the spec's spread formula (a radius smaller
    than the spread grows by the spread times `1 - (1 - r / spread)³ (1 - coverage³)`, `coverage` the corner's
    share of half the box, as WPT's `box-shadow-radius-generated-ref.html` computes it: a circle's shadow stays a
    circle), each corner and axis on its own, and a shadow without blur is the exact shape; the average radius stays only for a blurred shadow with differing
    corners (vello's blurred rectangle has one radius). Blurs use a standard deviation of half the blur radius in
    device px (Chromium's `BlurRadiusToStdDev`); outset shadows had the radius in CSS px. Inset shadows: the hole
    is the padding box with its own radii, shrunk by the spread; one without blur is a single even-odd fill in a
    clip (no layers), their offset is scaled to device px, and a transparent shadow no longer ends the list.
  - `src/kurbo_css/css_box.rs`: radii of the padding and content boxes that overflow their box are scaled to fit it
    (Blink's `FloatRoundedRect::AdjustRadii`), their centres in the inner box's corners: the padding curve of a box
    with one large corner (`border-radius: 100% 0 0 0`) left the box, and the border's inner edge with it.
    `border.rs`: a uniform solid border is stroked as a `kurbo::RoundedRect` only when its radii fit one (kurbo
    clamps a radius to half the smaller side, which such a corner exceeds: it was drawn with a much smaller one).
  - `src/gradient.rs` (`to_peniko_gradient`), `src/render/background.rs` (`uniform_gradient_color`): a gradient's
    colour interpolation method (`in hsl`, `in oklch longer hue`, and Stylo's Oklab default for stops in a modern
    colour syntax) is the peniko gradient's (stops were interpolated in sRGB whatever it said), and a gradient of
    one colour is filled with that colour (`linear-gradient(white, white)` came out a level short in places).

- `clip-path` and masks on boxes for the WPT paint reftests (`css/css-masking/clip-path`, `clip-rule`,
  `mask-image`), as CSS Masking, CSS Shapes and Chromium have them; covered by
  `crates/pyro-render/tests/clip_path_mask.rs`:
  - `src/render/clip_path.rs` (`element_clip`, `with_element_clip`), `src/render.rs` (`render_element`): a clip has
    its fill rule (`polygon(evenodd, …)`, `path()` and `shape()` took `nonzero`), and a shape of no area clips
    everything (an inside-out `rect()`, insets past the box, a zero radius: they clipped nothing). A box alone
    (`clip-path: padding-box`) clips to its rounded edges, the inner radii for the padding and content boxes; the
    margin box is the margin box (it was the border box), its radii grown by the margins as Blink's
    `FloatRoundedRect::OutsetForShapeMargin` grows them (by the margin, times `1 + (r / margin - 1)³` where the radius
    is smaller). A rounded box that is a whole ellipse is drawn as `circle()` and `ellipse()` are, as the box of
    `border-radius: 50%` is drawn (WPT's references are such boxes). Basic shapes as CSS Shapes resolves them:
    `inset()` (and `rect()`, `xywh()`) with its `round` radii, a `circle()` percentage of the reference box's
    direction-agnostic size, an `ellipse()`'s side keywords in its own axis, `path()` data zoomed. `shape()`: a single
    arc radius is of the direction-agnostic size, `cw` is SVG's sweep flag 1 (it was the other way round), the
    rotation is in degrees (kurbo takes radians), and a degenerate arc is a line (it dropped the whole shape).
  - `src/render/mask.rs`: `mask-mode: luminance` masks with the image's luminance (0.2125 R + 0.7154 G + 0.0721 B,
    `feColorMatrix`'s `luminanceToAlpha`): the layer is recorded and each solid colour, gradient stop and image pixel
    becomes black with that luminance times its alpha (exact unless translucent paints overlap; converted images are
    kept for the next frame). The mask painting area is the largest `mask-clip` box with its radii (it was the border
    box), everything visible for `mask-clip: no-clip`; each layer covers the whole area, its image cut to its own
    clip box, so its compositing operator applies outside that box too. `src/render/background.rs`: `mask-clip` and
    `mask-origin` take the SVG boxes (`fill-box` is the content box, `stroke-box` and `view-box` the border box) and
    `no-clip` (`BoxModelBox::NoClip`, the visible area): Stylo parses them for servo now (see
    `vendor/stylo/README.pyro.md`).
  - `src/render/clip_path.rs` (`svg_clip`, `svg_reference`), `src/render/mask.rs` (`draw_svg_mask`): `url()`
    references from a box to an SVG `<clipPath>` or `<mask>` of the page (`url(#id)`, or the page's own URL with a
    fragment: a `<base>` does not make one external). The element's outermost `<svg>` is serialized as inline SVG is
    for usvg (blitz-dom `BaseDocument::svg_reference_document`), in `<defs>`, with a box-sized rectangle that
    references the element: user space is the border box's (the element's zoom applied), `objectBoundingBox` units
    are of the border box, percentages of the viewport. A clip path of one shape (with no clip path of its own) is a
    clip layer of that shape with its clip rule; any other is drawn into a `DestIn` layer over the element, as a
    `<mask>` always is (anyrender_svg draws them). A reference to anything else clips nothing; clip paths and masks
    whose references form a cycle (`svg_reference::reference_cycle`, a depth-first walk of `clip-path`, `mask` and
    `style` references) clip and mask everything, without asking usvg, which follows a long cycle without end.
  - `src/render/clip_path.rs` (`draw_inline_effects`), `src/render.rs` (`draw_inline_layout`): an inline box (not
    atomic) with a `clip-path` or a mask is clipped and masked (both were ignored). As in Chromium it is a stacking
    context painted after the rest of its inline formatting context: the layout is painted without the areas of
    such boxes (their border boxes line by line, as tall as the line box, grown by their outline and shadows), then
    into each box's areas under its clip path, whose reference box is its first fragment (as Blink's and Gecko's
    are), and its mask, laid over its fragments side by side (`box-decoration-break: slice`). Inline boxes inside
    one are painted with it. An inline box's margin box has its style's margins on every side.

- Table structure (WPT reftests; see blitz-dom's README):
  - `src/render.rs` (`element_cx`): a table with captions paints its own box (background, borders, shadows,
    outline) around its grid alone (`TableContext::grid_box`); `table_part_transform`: a cell is painted under its
    row group's and row's transforms, each about the part's box (rows are transformable; their cells are the
    table's children). A collapsed table whose cells hold half of the borders around them draws the borders after
    its children (over the cells' backgrounds, as Chromium paints them).
  - `src/render/border.rs`: a collapsed table with cells never draws its own border (its outer borders are the
    collapsed ones that won there); the grid's borders are drawn below the captions above it.
  - `src/render/background.rs` (`draw_table_row_backgrounds`): background images of column groups, columns, row
    groups and rows are painted in each cell, positioned against the part's own box (`TablePartRects`), in that
    order with their colours; a collapsed row (`visibility: collapse`) paints nothing.
  - Lane P2 (tables, second pass): `src/render/background.rs` (`draw_table_part_shadow`): a row's outer box shadow
    is painted around the row's box (`TablePartRects`) at its first cell, before its background, row after row
    (as Chromium paints a row's shadow, then its background). `src/render/border.rs`: borders laid out as one
    uniform grid of gutters take the colour conflict resolution gave them (`TableContext::gutter_border_color`), not
    the first cell's top border's (which may be `none`, or lose to another); in `draw_collapsed_edges` the square
    where rules cross is painted in the colour of the edge that wins there (by width, then style:
    `CollapsedEdge::outranks`; among equals the first of the row line's rule on the left, the column line's above,
    the row line's on the right and the column line's below), when its rules are not all one colour (its top half
    was the row line's rule on the right, its bottom half the column line's below). Covered by
    `crates/pyro-render/tests/table_parts.rs`.

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
- Inline fragments and text painting (WPT reftest lane G2); covered by `crates/pyro-render/tests/inline_fragments.rs`:
  - `src/text.rs` (`line_insets`, `decoration_span`, `own_decoration`): `text-decoration-inset` applies at a
    decorating box's ends: with `box-decoration-break: slice` (the initial value) the insets cut the box's fragments
    (the lines it has text on) laid side by side as one strip, the start inset from the start of its first fragment
    and the end inset from the end of its last, what is longer than a fragment carrying into the next (WPT
    `text-decoration-inset-percentage-slice-multiline`); with `clone` both at the ends of every fragment (they
    applied at both ends of every line). Percentages are of the box's whole decoration line (its runs on all its
    lines, with its inline padding and borders), or of the fragment's for `clone` (they were of the line's runs). `auto`
    is a twelfth and a half of the font size, at least 1px (Stylo's servo value; it was no inset). The start is on the
    right in a right-to-left box. `stroke_text` takes the whole layout to find a box's lines (kept in
    `DrawTextContext::spans` while the same layout is drawn line by line); without it (an input's text, a marker, the
    ellipsis) each line is the whole box, as before. Vertical writing modes are left to lane L: the padding and
    borders counted are the physical left and right ones, not the inline axis's.
  - `src/text.rs` (`stroke_text`, `hanging_start`): white space hanging at the end of a left-to-right line is not
    decorated, nor counted in a box's decoration line (Chromium removes it): the line ends where the last run ends,
    less the hanging advance.
  - `src/text.rs` (`snapped_ends`, `flush_line_decorations`): a decoration line's ends are on whole device pixels
    where it is drawn only moved into a raster, so that the lines of adjacent boxes meet without a partly covered
    pixel, and a line ends where a reference's clip cuts the same line (WPT `text-underline-offset-calc`,
    `text-decoration-inset-003`, `text-decoration-subelements-002`; Chromium's lines end on whole pixels there because
    its glyph advances are whole on WPT's Linux runs).
  - `src/text.rs` (`flush_line_decorations`, `LineDecoration::segments`): a line's decorations are painted outermost
    box first, an inner box's line over an outer box's (the order of Chromium's applied decorations; it was the
    reverse), and each run under a decoration casts its own shadows on its part of the line, in pieces of runs with
    the same shadow cut on whole pixels, none where a run has no shadow at that index (they cast the first run's;
    WPT `text-shadow/decorations-multiple-zorder`).
  - `src/text.rs` (`draw_inline_text_mask`, `DrawTextContext::only_within`, `InlineTextMask`), `src/render.rs`
    (`draw_inline_boxes`, `draw_text_mask`): `background-clip: text` on an inline box (not atomic) masks with the
    glyphs of its own text and its descendants', drawn where its inline root draws them (line tops, layout-unit x),
    in each fragment's border box; the mask was empty. A block's mask is placed as its text is drawn and cut by
    `text-overflow: ellipsis` as the text is (`stroke_inline_text`, which `draw_inline_layout` shares), and so is an
    inline box's (`InlineTextMask::cut`). An inline box's mask does not hold ruby annotations or emphasis marks yet.
  - `src/render.rs` (`draw_inline_boxes`): a box sliced across lines that has box shadows or background images
    paints them as those of the whole box, its fragments laid side by side in line order (right to left for a
    right-to-left box), each fragment its piece: the background and the inner shadows in the fragment's border box,
    the outer shadows outside it but not past its open sides (Blink's `PaintRectForImageStrip`; WPT
    `box-shadow/slice-inline-fragmentation-*`). Each fragment cast a closed shadow, and a gradient started over in
    each. A box of colours alone paints as before, one box per fragment.
- Vertical writing modes (WPT reftest lane L):
  - `src/text.rs` (`CentralMetrics`, `underline_over_in_vertical`, `flush_line_decorations`): on the central baseline
    of `vertical-rl`/`vertical-lr` text (`mixed` and `upright`) underlines and overlines are placed from the text
    fragment's line-over edge by the em box, as Blink's `TextDecorationOffset` does with `FontBaseline::kCentral`
    (measured on Chrome 157, Inter at 10 to 54 px, whose rounded ascent and descent are Blink's float height too): an
    underline `floor(H/2 + em/2 + offset) + 1` px from the edge (1px under the em box), an overline on the font's
    whole-pixel ascent (`floor(H/2 - (H - floor(H/2))) - t`). H is the style's first available font's (Blink's
    `PrimaryFont()`), centred on the central baseline, also where a fallback font draws the run.
    `text-underline-position: right`, and Japanese or Korean text without `left`, trade their sides
    (`ResolveUnderlinePosition`'s `kOver`): the underline goes 1px over the em box (`text-underline-offset` moving it
    outward) and the overline where the underline would be; a `double` line's second line still goes away from the
    text. Mongolian trades them too, as css-text-decor-3's `auto` and Gecko do (Blink leaves it out; WPT's
    `text-underline-position-auto-001`). As in horizontal lines, each decorating box draws one straight line per line,
    placed by the fragment of its own text (its runs share the style's first available font, so which one does not
    matter), else of the first run it decorates; the inline root and the blocks it is in are not placed by the line's
    strut there. That run's baseline is where `VerticalLine`
    draws it. Sideways text keeps the alphabetic baseline and the horizontal rules. A vertical line's decorations
    have their edges on whole device pixels (Blink draws them unantialiased; `snap_turned_band`).
  - `src/text.rs` (`ink_gaps`, `VerticalLine::turned_run_shift`): `text-decoration-skip-ink` cuts the decorations of
    vertical lines too, around the glyphs turned with the line (upright glyphs leave no gap), their gap edges on whole
    device pixels (only in vertical lines: horizontal text under a quarter-turn transform keeps unsnapped gaps).
  - `src/render.rs` (`draw_vertical_inline_layout`, `draw_turned_lines`): `sideways-lr` lines are turned a quarter
    counterclockwise, running bottom to top from the content box's bottom with their line-over side on the left
    (they were drawn as `sideways-rl`'s, top to bottom).
  - Covered by `crates/pyro-render/tests/vertical_text_decorations.rs`.
- `src/render.rs` (`render_element`): an element whose 3D transform hides it (blitz-dom's
  `NodeFlags::TRANSFORM_HIDES`: its back face shows with `backface-visibility: hidden`, it lies behind the
  viewer, or its matrix has no inverse) is not painted, nor is its content. Covered by
  `crates/pyro-render/tests/transform_3d.rs`.
- `src/render.rs` (`render_element`, `BlitzDomPainter::composite_3d`), `src/layers.rs` (`local_raster_marker`),
  `src/lib.rs` (`paint_scene_inner`): a flat element with a 3D transform (blitz-dom's `NodeFlags::FLATTENS_3D`)
  that has element children paints inside a layer marked by a filter that moves nothing (an offset by negative
  zero), which anyrender_vello_cpu rasterizes in the element's plane and draws through its transform, as Chromium
  composites the element: edges its boxes share resolve where the layout puts them, on whole pixels of the plane.
  Drawn each through the transform, a child's edge over the element's took in what the element's edge let
  through, and a child blending there came out up to 6 levels off the same colours painted without a blend (WPT
  `mix-blend-mode-parent-with-3D-transform`). A box with no element in it shares no edge with another and keeps
  drawing its fills through the transform, without a resampled image's rounding (which put a lone box's edges a
  level apart inside and outside a scroll container, and 11 levels off a 2D rotation for an interpolated
  `rotate3d`). The layer goes inside the element's blend and isolation layers and around its clip-path, mask and
  effect group. Inside it (`local_raster_depth`), the groups clipped to the viewport (a blend's isolation, a backdrop
  root's mark) reach 8 px past it: the plane is rasterized a pixel or two past the frame for the pixels on the
  frame's edges to sample, and a clip on that edge cut through them (a fifth lighter along the screenshot's edge).
  Screenshots only (`paint_scene`): print, `paint_scene_with` and the text-recording paint stay
  vector. Covered by `crates/pyro-render/tests/transform_3d.rs`.
- `src/render/three_d.rs` (new; `BlitzDomPainter::paint_plane`, `bsp`), `src/render.rs` (`render_element`,
  `BlitzDomPainter::three_d`), `src/bounds.rs` (`layer_bounds`), lane H2: 3D rendering contexts and planes drawn
  through a perspective, as Chromium's compositor draws them. blitz-dom marks the planes (`Node::plane_3d`: the
  elements of a context with a transform node in Chromium, a context's root, a flat element its 3D transform maps
  projectively). The root of a context (or such a flat element) records its painting and each plane's on its own
  (a plane painted inside another leaves a marker there; culling is off and the viewport-sized group clips reach
  everywhere in a recording, as the stand-in transforms are not where planes are drawn), cuts the recordings into
  chunks at the markers in paint order, and draws the chunks back to front: each chunk is a polygon (its content's
  bounds in its plane through its 4x4 matrix, cut at the viewer's plane), sorted by a BSP tree as viz's `BspTree`
  sorts a 3D sorting context (the first polygon splits the others by its plane; coplanar ones keep paint order;
  one crossing a plane is split and each piece drawn clipped to it). A chunk whose plane keeps W constant is drawn
  through the affine of its matrix (its commands as recorded when that is the stand-in). One whose W varies is
  drawn through its homography: when it holds only fills in one solid colour and clips (`project_commands`), each
  path is flattened, cut at the viewer's plane, divided by W and snapped to a 1/1024 px grid (the same plane reached
  by another product of matrices lands on the same pixels); otherwise it is a projective layer for
  anyrender_vello_cpu (`projective_marker`: six offsets carrying the homography; its content in the plane's own
  pixels, its clip the projection's bounds), which `scene_bounds` takes as its clip. A plane that reads a backdrop
  and cannot be projected keeps its stand-in (the raster has no backdrop). The cut of a split piece is a clip layer
  with anyrender_vello_cpu's aliased mark (`aliased_clip_marker`); without the mark (another backend) it is an
  ordinary clip. A plane met inside a clip or layer of another stays there, flattened into that plane (the affine
  closest to the map between the two planes, `flattened_into`). An element of a context that is no plane paints in
  its parent's plane at its layout place (blitz-dom gives it no transform). A plane drawn through a perspective
  skips the local-raster layer above: the projective layer rasterizes it in its plane already.
  `backface-visibility: hidden` on a `preserve-3d` element showing its back hides its own chunks. Print
  (`composite_3d` false) gets the sorting with the stand-ins, their polygons where the stand-ins put them; a plane
  crossing the viewer's plane has no stand-in and is left out there.
  WPT `transform3d-sorting-*`, `preserve3d-and-flattening-*`, `css-transform-3d-transform-style`,
  `perspective-split-by-zero-w`, `transform-3d-rotateY-stair-*`. Covered by
  `crates/pyro-render/tests/three_d_contexts.rs` and the unit tests in `src/render/three_d.rs`.
- `src/render.rs` (`draw_children`): a fixed box that blitz-dom lifts into the root element's paint list from a
  containing block that is no layer root (a `backdrop-filter` box) is placed by its offset
  (`lifted_paint_offset`), not against the viewport (WPT `backdrop-filter-containing-block`).
- `src/render/multicol.rs` (new), `src/render.rs` (`paint_element`): a multi-column container (blitz-dom
  `layout::multicol`) paints its content once per column: moved to the column and clipped to the column's range of
  the strip in the block axis only (what overflows a column sideways shows, as in Chromium; the first column shows what
  lies above the strip, the last what lies below it), its content culled to the column. A box a break goes through is
  drawn in each column it reaches, each showing its part: `box-decoration-break: slice` (its border, background,
  shadow and clip path cut at the break, WPT `slice-block-fragmentation-*`, `clip-path-columns-shape-*`). A
  column outside the clip is not painted. A container whose content fits one column paints as before, without a
  clip layer. `column-rule` is painted over the border and under the content (inside the overflow clip, scrolled
  with the content), in the middle of the gap between neighbouring columns of a row (both have content), as tall
  as the columns, snapped to whole pixels: `solid` (and the 3D styles, as solid), `double`, `dashed` and `dotted`.
  Covered by `crates/pyro-render/tests/multicol.rs`.
- `src/render.rs` (`render_element`), `src/layers.rs` (`filter_reach`, `filter_source_area`): culling counts what
  an element's `filter` moves or spreads (the sum of its primitives' reaches, a drop shadow's offset, a blur): an
  element out of view whose shadow is in view is painted, and so is content a clip leaves out of view that the
  filter brings into it (its descendants are culled against the clip grown back by the reach). WPT
  `filters-drop-shadow-003`, `drop-shadow-clipped-001`; covered by `crates/pyro-render/tests/filter_drop_shadow.rs`.
- `src/render/background.rs` (`draw_background_layers`): `background-blend-mode`. Each image layer is drawn in a
  layer of its blend mode, and an element with one that is not `normal` (among the modes of its image layers)
  paints its background in an isolated group (not blending with what lies behind it), pushed through the layer
  manager. Covered by `crates/pyro-render/tests/mix_blend_mode.rs`.
- `src/render/clip_path.rs` (`draw_inline_effects`, `paints_alone`, `disjoint_rects`): inline elements (not
  atomic) with `opacity` or `backdrop-filter` join those with a clip path or a mask: the layout is painted in their
  areas in a group of their opacity (all fragments as one group), over their backdrop filtered in their fragments'
  border boxes (snapped as `draw_inline_boxes` snaps them). Only when no inline ancestor between the element and its
  inline root paints a box there (a background, a border, a shadow), which the group would take in. The areas go out
  to whole device pixels around the snapped boxes, and the layout outside them is clipped by pieces that do not
  overlap (overlapping areas, lines closer than their text is tall, were painted twice), inside the layout's ink
  (recorded first, `bounds::scene_bounds`) rather than the whole visible area (a frame-sized clip layer). WPT
  `t32-opacity-offscreen-multiple-boxes-1-c`/`2-c`, `backdrop-filter-inline-positioning`; covered by
  `crates/pyro-render/tests/opacity_group.rs` and `backdrop_filter.rs`. Each element's areas leave out those of
  the elements before it (neighbours' areas met in a pixel column, painted in both passes). A recording paint (the
  PDF's text, `paint_scene_recording_text`) leaves inline opacity and backdrop filters out: each pass draws and
  records glyph runs, and the PDF writes every run drawn, so the text was written once per pass (its text stays
  unfaded, once, in reading order; `pdf_fidelity.rs`). A pass outside the areas that is left out (out of view)
  drops the runs it recorded. Inline clip paths and masks still go through the passes in PDFs, as before. Not
  faded: atomic inline descendants (inline-blocks, images, inline SVG, painted by `draw_children`), floats, the
  ink of a faded element's glyphs or text shadows beyond its areas (the outside pass paints it unfaded).
- `src/text.rs` (`RunFilter`, `stroke_text`), `src/render.rs` (`stroke_inline_text`): in an element's pass
  `draw_inline_effects` draws only the glyph runs whose ink (an em around the run, its text shadows) can reach the
  element's areas, in screenshots only their glyphs within that reach, and only the lines with such a run or a
  decoration: each pass drew every glyph of the layout, so a paragraph cost its length times its faded spans (a
  4K frame of a paragraph with 30 faded spans: 4.19 G instructions, now 0.61 G; 0.49 G unfaded). A recording paint
  keeps whole runs (the PDF maps runs to their text). The `text-overflow` ellipsis is not filtered.
- Backgrounds and box edges, wave 3 of the WPT paint reftests (lane F2); covered by
  `crates/pyro-render/tests/background_images_and_edges.rs`:
  - `src/render/background.rs` (`round_layer_size`): the round step of `background-repeat` scales an axis to a
    whole number of copies (at least one: an image over twice the area's size divided by zero copies), and with
    `round` in one axis and `background-size: auto` in the other, the other axis with it, so the image keeps its
    aspect ratio (CSS Backgrounds 3 §3.9; WPT `background-repeat-round-2`, `background-size-029`). The image is
    positioned at its rounded size (it was positioned at its unrounded size, a phase off).
  - `src/render/background.rs` (`draw_svg_image_layer`): an SVG image without `viewBox`, `width` or `height`
    (blitz-dom keeps its source, `SvgImageData::for_url`) is laid out in the size the layer draws it at, its
    percentages against that size (`SvgImageData::sized`, as an inline `<svg>` without a `viewBox`); it was laid
    out at usvg's 100x100 and stretched (WPT `background-size-043`/`-044`, `diagonal-percentage-vector-background`).
  - `src/render/background.rs` (`bleed_avoiding_border_box`): Chromium's background bleed avoidance
    (`BoxDecorationData::ComputeBleedAvoidance`, `kBackgroundBleedShrinkBackground`): under a rounded border that
    hides the background's edge (every side's colour opaque, its style neither dotted, dashed nor hidden, no
    border image), layers cut to the border box are cut to it inset by half of each side's width (a sixth with a
    `double` side), its radii shrunk with it. Drawn to the outer edge, the background showed through the border's
    antialiased outer edge: a dark seam around a light border, which the same border over a white page lacks (WPT
    `background-clip-content-box-with-border-radius-003` and `-padding-box-` compare against such a border). Not
    for the canvas background or a scroll container's `local` layers, as in Chromium, and not under a side
    narrower than two device pixels, where the background's inset edge and the border's two antialiased edges
    share pixels and what lies under the box showed through between them (WPT
    `filter-effects/blur-clip-border-radius`). A stroked border's curved outer edge covers a little less than
    Skia's fill of it, so a rounded box whose background has the border's colour now has a slightly lighter
    edge there than Chromium's, where the background's coverage used to add to it (ToS buttons and radio
    buttons: SSIM changes in the sixth decimal).
  - `src/render.rs` (`with_replaced_content_clip`): replaced content of a box with a border radius and a border or
    padding is cut to the content edge's curve (CSS Backgrounds 3 §5.3); the padding box's clip around it was the
    only one (WPT `border-radius-clip-001`).
  - `src/render/border.rs` (`draw_square_solid_sides_overdrawn`): a border with square corners whose sides are all
    solid and opaque, in more than one colour, is painted by overdraw as Blink's `BoxBorderPainter` paints it: the
    sides go in its order (top, bottom, right, left), a side reaches into a corner in full unless a side of another
    colour was painted there before it (then it stops at the corner's diagonal), so the later side's antialiased
    diagonal lies over the earlier side's colour. As two trapezoids, both diagonals were antialiased over the
    background, which showed through between them (WPT `CSS2/border-seams-001`).
- `src/render.rs` (`inline_box_fragments`, `paint_inline_box_fragment`, `InlineBoxFragment`; together what the
  entries above call `draw_inline_boxes`: the fragments are collected, then painted): an inline root
  with an inline box that reaches above its line, over the line before (tall padding or borders), paints line by
  line, each line's box fragments and then its text (CSS 2.1 Appendix E), so a line's backgrounds cover the text of
  the line above as in browsers; other roots paint all fragments, then all text, as before. A split inline
  (blitz-dom's `NodeFlags::IS_SPLIT_INLINE`) paints no box of its own: its pieces' lines paint its fragments, with
  its start and end sides only in the pieces that hold them (`TextLayout::split_pieces`). Relatively positioned
  inline boxes move their fragments.
- `src/text.rs`: the text of relatively positioned inline boxes is painted moved
  (`BaseDocument::inline_span_relative_offset`), only in inline roots that have one. Covered by
  `block_in_inline.rs`, `inline_relative_position.rs` and `line_paint_order.rs` in `crates/pyro-render/tests`.
- `src/text.rs` (`stroke_text`, `DrawTextContext::first_line`), `src/render.rs` (`inline_box_fragments`,
  `paint_inline_box_fragment`, `draw_inline_layout`): `::first-line`. The first line of an inline root whose
  layout has a `::first-line` style node (blitz-dom's `TextLayout::first_line`) is painted as if its content were in
  an inline box of that style: the box is on each run's stack between the root and the elements on the line, so
  text that inherits takes its colour and text shadows and its decorations are drawn under the line's text (placed
  by the root's own font); the elements on the line paint their text and boxes in their styles reparented onto it
  (blitz-dom's `BaseDocument::first_line_style`: inherited properties from the first line, `currentcolor` its
  colour); its background, borderless, is a fragment around the line's content, painted before the elements'.
  Nothing changes for roots without one. Covered by `first_line.rs` in `crates/pyro-render/tests`.
- `src/highlight.rs` (new), `src/text.rs`, `src/render.rs` (`draw_inline_layout`): the selection is painted as
  Chromium paints highlights (CSS Pseudo-Elements 4 § 3). Behind each selected stretch of a line goes the
  highlight's background, from the line box's top to its bottom or the text's content area where that reaches
  further (Blink's `ExpandSelectionRectToLineHeight`), on whole CSS px as inline boxes are; the line is then
  drawn twice, clipped: outside the stretches as usual, inside them in the highlight's colour, decorations of the
  text's colour (`currentcolor`, `ResolvedDecoration::current_color`) in it too and no text shadows. The highlight
  is the `::selection` style of the nearest element with one, else the system's `HighlightText` on `Highlight`
  (Stylo's servo values); a `::selection` style's own `text-decoration` is drawn over the selected text as a
  line of that element (`LineDecoration::selection`); text under `user-select: none` is not highlighted. A line cut by an ellipsis keeps
  upstream's light blue rectangles; a recording paint (the PDF's) draws no selection. Covered by
  `selection_highlight.rs` in `crates/pyro-render/tests`.
- `src/text.rs` (`LineDecoration::shift`, `stroke_text`): the decoration line of a relatively positioned inline box
  (or one inside such boxes of the root) moves with its text (it stayed where the box's text was laid out, under the
  moved glyphs). White space hanging at the end of a right-to-left line, at its left, is not decorated, as at the
  right of a left-to-right one. Covered by `direction_text.rs` in `crates/pyro-render/tests`.

- `src/filters.rs` (`image_filter`), `src/render/background.rs` (`draw_image_layer_in`),
  `src/render/border_image.rs` (`draw_border_image`): a `filter()` image (the CSS `<image>` function of Filter
  Effects 1, parsed by the vendored stylo) is the image it filters, drawn in a layer with its filters (nested ones
  innermost first) over the layer's painting area, or over the border image area for a border image: a
  `drop-shadow()` stays inside that area. The filters apply to the image as drawn, after scaling, in device pixels
  (WPT css/filter-effects/filter-function/filter-function-002, -003, the gradient pages). Tiled layers are filtered
  as one image, not tile by tile, and the result is not cut to the image's own size: WebKit filters each image in
  its own rect, so there a `drop-shadow()` or blur of an image smaller than its painting area (or of one tile)
  stops at the image's edge, while here it spreads up to the area's edge. No WPT page tells them apart (each sizes
  its image to the area). `draw_border_image` looks for filters only when the source is a `filter()`. Covered by
  `crates/pyro-render/tests/filter_image_function.rs`.

The linked blitz-dom patch supplies `Node::is_root_element`. Rendering regressions
are covered in `crates/pyro-render/tests/render.rs`; persistent-tree behavior is
covered by `crates/pyro-render/tests/mirror.rs`.

## The SVG viewport clip

`src/render.rs` (`draw_svg`, `with_replaced_content_clip`, `ReplacedClip`, `content_edge_is_rounded`,
`should_clip_node`): an SVG's content is cut at its viewport as Chromium cuts it (`LayoutSVGRoot::ShouldApplyViewportClip`,
`ReplacedPainter`): an inline `<svg>` unless its `overflow` is `visible` (blitz-dom's UA sheet now has Chromium's
`svg:not(:root) { overflow: hidden }`, and `overflow="visible"` is the presentation attribute), an SVG image always.
The cut is the content box (`overflow-clip-margin: content-box`) along the content edge's curve, so a rounded
`<svg>` is cut at its corners. It is a layer only where it cuts: the tree's layer bounds (strokes and filter regions
included) reach past the content box, or the content edge is rounded; an icon that fits pays nothing. An inline
`<svg>` with `overflow: visible` is not cut at all (Chromium skips its clip). `should_clip_node` leaves an SVG's
`overflow` to `draw_svg`: the padding box clip it pushed for every overflowing element would cut at the wrong box
and push a layer for every icon. Train 4's sniffing of the `overflow` attribute and `style` for filter output, and
its 300 x 150 floor for an `<svg>` without a size, are gone with it. WPT CSS2 `absolute-replaced-width-002` and 28
other `svg:svg` pages; covered by `crates/pyro-render/tests/svg_viewport_clip.rs`.

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

## Floats painted in the phases of their stacking context (WPT reftests, lane P3)

- `src/render.rs` (`FlowPhase`, `ElementCx::draw_children_in_phases`, `draw_flow_children`): a box whose paint
  children include floats (blitz-dom `NodeFlags::HAS_FLOAT_PAINT_CHILDREN`) paints its children as CSS 2.1 Appendix
  E does: negative `z-index`, the backgrounds of the blocks of its normal flow (`FlowPhase::Background`, the boxes
  that paint in its phases walked through, any other block-level box whole), the floats, its and their inline
  content (`Foreground`, inline-level boxes whole), their outlines (`Outline`, only when one has an outline, under
  the positioned boxes as in Chromium; WPT `z-index-020`), then the positioned boxes and stacking contexts. A box
  painted in a phase pushes no layer (it has no effect or clip). Pages without floats paint as before.
- `src/render.rs` (`enclosing_inline_effects`): a float in inline boxes with an `opacity` or a `filter` paints in a
  group with their effects (WPT `inline-opacity-float-child`, `filtered-inline-applies-to-float`).
- `src/render.rs` (`inline_box_fragments`) and `src/render/clip_path.rs`: a line's content ends at its
  `inline_min_coord` plus its advance: on a line beside a float the boxes of inline elements were cut to nothing
  (WPT `clear-inline-001`). `src/render/border.rs`: `outline_line` is visible to `render.rs`.

Covered by `crates/pyro-render/tests/floats.rs`.

## SVG viewports and references to other SVG documents (second pass, lane P6)

- `src/render.rs` (`draw_svg`): the root `<svg>` of an SVG document always clips at its viewport, whatever its
  `overflow` (Chromium's `LayoutSVGRoot::ShouldApplyViewportClip`; the UA sheet's `overflow: hidden` is for
  `svg:not(:root)`, so the root computed `visible` and painted past its box). An SVG image (`<img>`, `<object>`) is
  cut at its concrete object rect as well as at the content box: under `object-fit: contain`, `none` or `scale-down`
  content outside the image's viewport no longer shows in the rest of the box. That second clip layer is pushed
  only when the object rect leaves some of the box and the content's ink reaches past the rect; icons and images
  that fill their box pay nothing.
- `src/render/clip_path.rs` (`svg_reference::external_reference`, `svg_clip`), `src/render/mask.rs`
  (`draw_svg_mask`), `src/filters.rs` (`reference_graph`, `filter_steps` takes the element's
  `reference_images`): `clip-path: url(shapes.svg#clip)`, `mask-image: url(masks.svg#mask)` and
  `filter: url(filters.svg#f)` name elements of other SVG documents (`data:` URLs too). The document blitz-dom
  loaded for the element (`ElementData::reference_images`, or the mask layer's image) stands in `<defs>` of the
  reference document in place of the page's `<svg>` (`SvgImageData::reference_document_from_source`). A mask
  layer whose URL names a `<mask>` of its document is that mask, not the document's drawing. Another document's
  filter is always a graph (no notes about its primitives). WPT `clip-path-url-reference-external`,
  `mask-image-1c`, `mask-image-url-remote-mask`, `filter-external-002-test`.

- `src/render/mask.rs` (`mask_layer_clip`, `names_svg_mask`, `draw_svg_mask`): a mask layer that references an
  SVG `<mask>` is not cut to its `mask-clip` box (that cuts mask images; a `<mask>` applies in its own region), and
  a mask in user units throughout (`maskUnits="userSpaceOnUse"`, no `objectBoundingBox` content) is taken from
  everything visible rather than from the border box, drawn with `anyrender_svg::render_svg_tree_within`: it shows
  the element's box shadow where its content is (WPT `mask-image-3f`, `-3g`).

Covered by `crates/pyro-render/tests/svg_viewport_clip.rs`, `svg_external_references.rs` and `svg_css_effects.rs`.
Covered by `crates/pyro-render/tests/svg_viewport_clip.rs` and `svg_external_references.rs`.
## `visibility` on descendants; in-flow blocks that overlap (WPT reftests, lane P5)

- `src/render.rs` (`render_element`, `paint_element`, `ElementPaint::hidden`), `src/text.rs`
  (`DecorationStackEntry::hidden`, `node_is_hidden`): an element whose `visibility` is not `visible` paints nothing
  of its own (box, text, marker, image, outline) but its descendants that are `visible` again still paint (CSS 2.1
  § 11.2); the element and everything in it was skipped. A glyph run is hidden by the `visibility` of the inline box
  it is in, with its decorations and shadows, and so is that box's fragment (WPT `visibility-005`,
  `position-relative-032`).
- `src/render.rs` (`paint_element`): a box whose in-flow block children overlap (`NodeFlags::FLOW_BOXES_OVERLAP`,
  set by layout) paints them in the phases of CSS 2.1 Appendix E, as one with floats does: the block backgrounds
  first, then the inline content, so an inline-block or a line of an earlier block is not covered by the background
  of a later one (WPT `inline-block-zorder-001`/`-002`, `inline-table-zorder-001`/`-002`, `position-absolute-007`,
  `margin-collapse-004`).

- `src/render.rs` (`inline_box_fragments`): a glyph run of bidi formatting characters alone is no content of an
  inline box's fragment (WPT `bidi-011`).

Covered by `crates/pyro-render/tests/inline_leftovers.rs`.

## Lane I of WPT round 3 (`text-box-trim`)

- `src/render.rs` (`render_node`): the stacking contexts of negative z-index are painted before the box's own
  inline content (Appendix E step 3 before step 7); they were painted over its text.
- `src/render.rs`, `src/render/clip_path.rs`, `src/text.rs` (`draw_inline_backgrounds`): the boxes of inline
  fragments use `Line::span_box_area` (the content area less `text-box-trim`); decorations keep the font's.
- `src/text.rs` (`VerticalLine::turned_offset`), `src/render.rs`: relatively positioned inline boxes move their
  text and decorations in vertical lines too, the offset turned with the line.

Covered by `crates/pyro-render/tests/text_box_trim.rs`.
## Sticky boxes painted where they are held (WPT reftests, round 3 lane S)

- `src/render.rs` (`node_transform`): a box is painted at its layout position plus its `Node::sticky_offset`, so a
  `position: sticky` box and its descendants are where their scroll container holds them (they were painted at
  their static position whatever the scroll positions, while geometry and hit testing already followed them).
  The offset is snapped to device pixels from the box's unrounded position, so a box laid out at 10.5 and held
  9.5 further is painted at 20 (WPT `position-sticky-fractional-offset`).
- `src/render.rs` (`table_part_transform`), `src/render/background.rs` (`draw_table_row_backgrounds`,
  `draw_table_part_background`): the offsets of a sticky row group and row move their cells and the backgrounds
  painted behind them, as rows and row groups are no boxes of their own.

Not done: a sticky (or relatively positioned) row or row group is not painted as a positioned layer, so boxes
painted after the table's backgrounds can cover its background (WPT `position-sticky-table-tr-top` and the like).

Covered by `crates/pyro-render/tests/sticky_paint.rs`; WPT `css/css-position/sticky`: 33 more reftests pass.
- `src/render.rs` (`inline_box_fragments`), `src/render/clip_path.rs`: an inline box's fragment is cut by the
  collapsible white space hanging at its line's end (`LineMetrics::collapsed_advance`), not by all that hangs:
  preserved spaces and ideographic spaces that hang keep their background (WPT `trailing-ideographic-space-*`;
  `crates/pyro-render/tests/hanging_spaces.rs`).
## Gradient colour stops, `image-set()`, `image(<color>)` (WPT css-images, round 3 lane F)

- `src/gradient.rs` (`Ramp`): a gradient's colour stops are fixed up (CSS Images 3 §3.4.3: a stop before its
  predecessor moves up to it, stops without positions are spaced evenly) and interpolated here, and the renderer
  gets sRGB samples it interpolates premultiplied. In sRGB without hints those are the stops themselves, cut at
  the ends of the gradient box; in another colour space (`in oklch`, `in hsl longer hue`, the Oklab default of a
  modern colour) or under a transition hint each segment is cut in halves until the renderer's line is within a
  quarter level of the curve (`subdivide`). The colours are mixed by Stylo's `mix_many` (premultiplied, missing
  and powerless components, hue fix-up). Before, the stops went to the renderer as written with the colour space
  as a setting: stops outside the box were not cut where it ends (`linear-gradient(green -50%, blue -50%)` drew
  green), the padding stops the old code added were interpolated with their neighbour (`longer hue` after the
  last stop went once round the hue circle), and a gradient 131,070,000px long was sampled in `f32`.
  A repeating gradient without a period is its last stop's colour, as is a radial one whose ending shape has no
  area or whose stops are all before the centre; a radial one ending in a hard stop keeps room after it for the
  colour that follows; a radial centre or radius beyond 1e9 px is one colour over the box; a repeating radial or
  conic gradient is sampled as one period from 0 (`sample_period_from_zero`: the first stop's position was
  ignored). `to <corner>` uses the corner-to-corner rule of §3.1.1 (the line's 50% joins the two other corners;
  it was the diagonal, which is right for a square only).
- `src/render/background.rs` (`draw_image_layer_in`, `draw_generated_layer`): `image-set()` paints the option
  Stylo selected, a raster one at its `<resolution>` (`ImageLayerStyles::resolution`); `image(<color>)` is an
  image of one colour without a natural size, sized, positioned and tiled as a gradient is.
- `src/render.rs` (`draw_inside_marker_image`): a `list-style-image` marker inside the item, at the start of
  its first line with its bottom on the baseline.
- `src/text.rs` (`synthesis_allowed`): `font-synthesis-weight: none` and `font-synthesis-style: none`
  (`oblique-only` for italic) stop the faked bold and slant of a run.

Covered by `crates/pyro-render/tests/gradient_stops_image_set.rs`.

- `src/gradient.rs` (`has_missing`): a stop with a missing component (`color(srgb none 0.5 0.5)`) takes it from
  its neighbour across the whole segment, the segment's ends included, in sRGB too (WPT
  `gradient-none-interpolation`).
- `src/render.rs` (`draw_image`, `draw_svg`): `object-view-box`: the view box is what `object-fit` and
  `object-position` fit and place; the content around it is drawn at the view box's scale where the content box
  has room. Covered by `crates/pyro-render/tests/object_view_box_embed.rs`.

- `src/render.rs` (`draw_marker_symbol`): a disc, circle or square marker is drawn in the colour of the item's
  `::marker` style (`ListItemLayout::style_node`).
- `src/render.rs` (`draw_image`): a pixelated replaced raster image is sampled clamped to its edge (a smoothed one keeps blending with the opposite edge, as background tiles do across seams). At a fractional
  position (`object-position: 50% 13%`) the partly covered edge row took the opposite edge's pixels, the brush
  repeating (WPT `object-position-png-001*`/`-002*`).
- `src/text.rs`: under `font-size-adjust` a run has the used font size; `text-decoration-thickness` and
  `text-underline-offset` percentages stay of the computed one (WPT `text-decoration-thickness-percent-001`).

## Highlight colours of `::selection` (WPT round 3, lane I)

- `src/highlight.rs` (`highlight_of`), `src/text.rs`: the highlight is the nearest `::selection` style that declares
  `color` or `background-color` (`BaseDocument::style_declares`: author and user rules, a `revert` declares
  nothing), an ancestor's when the nearer ones declare neither; with none, the system's `HighlightText` on
  `Highlight` (CSS Pseudo-Elements 4 § 3.4, as Chromium: an empty `::selection {}`, one with other properties only
  or invalid values still paints the selection). A style that declares a background but no `color` leaves the text
  its own colour (`Highlight::color` is `None`). The nearest style's own decoration is drawn either way, in
  `HighlightText` with the system's colours (`ResolvedDecoration::with_current_color`).

Covered by `crates/pyro-render/tests/selection_highlight.rs`.

## `font-palette` (WPT css-fonts, round 3 lane F)

- `src/palette.rs`, `src/text.rs` (`draw_run`): a COLR glyph run is drawn in the palette of its element's
  `font-palette`. The glyph painter under vello_cpu (glifo) reads palette 0 of the font's CPAL table only, so a
  run whose palette differs is drawn from a copy of its font whose palette 0 holds the selected colours (records
  patched in place; copies cached per font and colours, 32 at most). `light` and `dark` are the first palettes the
  CPAL table marks so (else palette 0); a `<dashed-ident>` is the last `@font-palette-values` rule of that name
  whose `font-family` names the run's font (the `@font-face` family it was loaded under, or an installed font's
  own name): its `base-palette` (an index the font does not have is palette 0) with its `override-colors`;
  `palette-mix()` mixes two palettes entry by entry in its colour space (`mix_many`). Covered by
  `crates/pyro-render/tests/font_palette.rs`.

- `src/text.rs`: decoration thickness and offset percentages are of the element's font size where a face's
  `size-adjust` scales the run (WPT `size-adjust-text-decoration.tentative`).
- `src/render/border_image.rs` (`unturned_raster`): under `image-orientation: none` a border image is sliced from
  its pixels as stored, before the EXIF orientation turned them (WPT `image-orientation-border-image`). Covered by
  `crates/pyro-render/tests/border_image_orientation.rs`.
- `src/gradient.rs` (`srgb_filled`, `Ramp::curved`): in sRGB a segment whose stops miss a component is drawn
  between its exact ends (each missing component the other end's, 0 where both miss it) by the renderer's own
  interpolation, as the equivalent hard-stop gradient is; it was subdivided through the mix, off by a level here
  and there (WPT `gradient-none-interpolation`). Covered by `crates/pyro-render/tests/gradient_stops_image_set.rs`.

## Inline boxes' margins in vertical writing modes (WPT round 3, lane TS)

`src/render.rs` (inline box fragments): a span's own edge leaves out its inline-start or inline-end margin as
`blitz_dom::span_edge_margins` gives it: left and right in horizontal text, top and bottom (by the inline direction)
in a vertical writing mode, whose lines are laid out turned (blitz-dom keeps those margins as span edges now).
## Corner shapes and border shapes (WPT round 3, lane CB)

- `src/kurbo_css/contoured.rs` (new), `src/kurbo_css/css_box.rs` (`CssBox::with_corner_curvature`, `contour`,
  `contoured_like`), `src/render.rs` (`create_css_rect`): `corner-shape` (CSS Borders 4 § 4). Each corner is the
  quarter of a superellipse of exponent `2^k` (`superellipse(k)`; `bevel` 0, `scoop` -1, `notch` -∞, `squircle` 2,
  `square` ∞, which leaves the corner without a radius), drawn as Blink draws it (`ContouredRect`,
  `PathBuilder::AddContouredRect`): a conic for `round`, a line for `bevel`, two cubics per corner otherwise
  (`ApproximateSuperellipseHalfCornerAsBezierCurve`). A box with a concave corner has its radii scaled until the
  hulls of opposite corners no longer overlap (`RadiiConstraintFactorForOppositeCorners`). An edge inset or outset
  from the border edge (the padding and content edges, a shadow's spread, the outline, the bleed-avoiding
  background) keeps its curves parallel to the border edge's: its rectangle less each corner's clip-out region
  (`CornerClipOutPath`, `Corner::AlignedToOrigin`), taken out by a polygon boolean on flattened outlines
  (`contoured::boolean`, in place of Skia's path ops). Corner curves are flattened to 0.02 device px (vello_cpu
  flattens fills to 0.25px, which moved antialiased pixels by up to a quarter of their range against Chromium's).
  The border of one colour fills the area between the edges once; sides of different colours split it by the
  miters from each outer corner to the inner one (`side_region`). The background, borders, overflow clips,
  `clip-path` boxes, backdrop filters, box shadows and outlines all follow the shapes. Interpolated keywords snap
  back to their parameter (`exponent`), so a no-op animation stays round. Covered by
  `crates/pyro-render/tests/corner_border_shape.rs` and `contoured.rs`'s unit tests.
- `src/render/clip_path.rs` (`flattened`, `is_oval`): `path()`, `shape()` and an SVG `<clipPath>` of one path are
  flattened to 0.02 device px too (ovals are not: `border-radius: 50%` boxes are their references), so that a
  corner-shaped box and a `clip-path: path()` reference drawn alike cover the same pixels.
- `src/render/border_shape.rs` (new), `src/render/border.rs`, `src/render/background.rs`,
  `src/render/box_shadow.rs`: `border-shape` (CSS Borders 4 § 11), as Blink's `BorderShapePainter` draws it. With
  one shape the border is a stroke along it (mitered; a polygon's miters are never cut, curves' at 4 widths) in the
  relevant side's width and colour (the first of block-start, inline-start, block-end, inline-end whose style is not
  `none`), its reference box `half-border-box` by default; with two, the area between the outer and the inner shape
  (border box and padding box by default). `border-radius` and `corner-shape` are ignored. The background is
  clipped to the inner shape (the shape, with one) and painted out to the border's outer bounds, `background-clip:
  border-area` to the border's area, overflow to the border's inner edge; outer shadows are the outer edge grown by
  the spread (an oval as an oval; a circle's blurred shadow is vello's blurred rounded rectangle, as a
  `border-radius: 50%` box's), cut out of it and blurred through a filter layer; inner shadows fill around the
  inner edge shrunk by the spread; solid and double outlines are bands of the outer edge grown by the offset. Edges
  worked out here (stroke outlines, offsets) use vello_cpu's own tolerance, so that a clip at a stroke's inner edge
  falls where the drawn stroke's edge does.
- `src/render/box_shadow.rs` (`fill_shadow_path`): a blurred outer shadow whose corners are not all one circular
  radius (elliptical, differing or shaped corners) is its shape's path blurred through a filter layer; it was vello's
  blurred rounded rectangle at the corners' average radius (a `border-radius: 50%` ellipse's shadow came out boxy).
- `src/render/clip_path.rs` (`ellipse_radius`): `closest-corner` and `farthest-corner` radii of `ellipse()` (CSS
  Shapes 2, pref `layout.css.ellipse-corners.enabled`, set by pyro) are the distance to that corner, for either axis.
- `src/render.rs` (`draw_marker`, `draw_marker_symbol`): in a right-to-left list item the outside marker is placed
  after the border box's right edge (a disc, circle or square mirrored), not before its left edge.

`src/text.rs` (`VerticalLine::run_placement`): a run stands upright or not by its first visible character; an
empty inline box's strut (U+2061) or a bidi control at its start made the whole run sideways (WPT
`text-spacing-trim-start-oof-001` in `vertical-rl`).
## `display: contents` spans paint no box (WPT round 3, lane D)

blitz-dom's inline build makes a `display: contents` element a style span of its text (font, spacing, colour:
its README, "Shadow trees"), so its id is a glyph run's brush. `src/render.rs` (`paints`) gives it no inline box
to paint (background, border, shadow, outline), and `src/render/clip_path.rs` no clip, mask, opacity or backdrop:
it has no box (WPT css-display `display-contents-inline-001`, `-flex-001`, `-oof-*`, `-button`).
- `src/render/background.rs` (`draw_positioned_table_part_backgrounds`), `src/render.rs`: the background of a
  relatively positioned or sticky row group or row is painted by each of its cells, under the cell's own, in the
  positioned layer the cells paint in (blitz-dom `paint_tree.rs`); the table's background phase leaves it out (WPT
  `position-relative-table-*`, `position-sticky-table-*`). Covered by `crates/pyro-render/tests/table_part_positioning.rs`.

## An inline `<svg>` visible on one axis (WPT round 3, lane SM)

`src/render.rs` (`ElementCx::draw_svg`): an inline `<svg>` whose `overflow` is `visible` on one axis is cut on the
other axis alone, at its content box, with square corners whatever its `border-radius` (Chromium clips such a box per
axis and does not round the cut; it was cut on both axes along the rounded content edge). WPT
`css/css-overflow/rounded-overflow-clip-visible`, `-visible-clip`, `overflow-clip-x-visible-y-svg`,
`overflow-clip-y-visible-x-svg`. Covered by `crates/pyro-render/tests/overflow_clip.rs`.

Tables (round 3 lane C2T), covered by `crates/pyro-render/tests/table_paint_order.rs`:
- `src/render.rs` (`table_borders_in_phases`, `draw_children_in_phases`): a collapsed table paints its parts in phases
  (`Node::paints_in_table_phases`) and its borders after the block backgrounds in its cells, before their floats,
  inline content and positioned boxes, as Blink (WPT `css-tables/tentative/collapsed-borders-painting-order-*`,
  `collapsed-border-paint-phase-001`). The phase path no longer paints the box of an element whose `visibility`
  hides it. A collapsed table with positioned parts leaves its borders to its last cell in one of them
  (`last_positioned_table_cell`, `draw_deferred_table_borders`), which paints them after itself, over all of them
  (WPT `collapsed-border-positioned-tr-td`).
- `src/render/background.rs` (`draw_table_part_background`): a row's, row group's or column's background is painted
  over the whole cell (the canvas mode of `draw_background_layers`), its images positioned against the part: a cell
  spanning rows shows the row's colour below the row (WPT `css-tables/tentative/paint/background-image-row`).
  `draw_table_part_outlines`: the outlines of rows and row groups, which paint no box, after the table's content.
- `src/gradient.rs` (`linear_gradient`): an angled linear gradient's line goes through the centre of its tile, not
  of the element's border box (a tile larger than the box or moved off it showed the wrong part of the gradient).
- `src/render.rs` (`marker_line_x`): an outside marker moves with its line past floats only when the line holds
  something in flow (text with an advance, an in-flow inline box); a list item holding only a float keeps its marker
  before its border box, as Blink (WPT css-display `display-flow-root-list-item-001`).
