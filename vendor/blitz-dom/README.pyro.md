# blitz-dom main (c27b4ca), patched

`packages/blitz-dom` of [DioxusLabs/blitz](https://github.com/dioxuslabs/blitz) main at commit
`c27b4ca14158dc457f35c78eb32b8c4fec906b7e` (#1525, 142 commits past 0.3.0-beta.4 = `92b11f1b`; its version
is still 0.3.0-beta.4), used through `[patch.crates-io]` in the workspace `Cargo.toml`. License: MIT OR
Apache-2.0, as upstream. `src/` and `assets/` are copied; `Cargo.toml` is the package manifest with the
workspace's dependency versions written in, git dependencies replaced by their crates.io releases (Taffy and
Stylo come from `vendor/`, Stylo at 0.24.0 where main pins 0.23.0: "Stylo 0.24" below, anyrender 0.14 from crates.io: main's anyrender API is unchanged). blitz-traits,
blitz-html and stylo_taffy are vendored from the same commit unchanged (`vendor/blitz-traits`,
`vendor/blitz-html`, `vendor/stylo_taffy`): blitz-dom main needs them.

`UPSTREAM.patch` (applies with `git apply` from the root of an upstream checkout at the commit above) is
the whole diff: every section below.

## Updating or dropping the patch

- To move to a newer blitz main: in a blitz checkout, commit this directory's `src/` and `assets/` over
  `packages/blitz-dom` at the commit above (or apply `UPSTREAM.patch`) and merge or cherry-pick onto the new
  main; copy `src/` and `assets/` back, regenerate `Cargo.toml` from the package and workspace manifests,
  and update the commit above and `UPSTREAM.patch`. Check `assets/default.css` by hand: a merged CSS
  conflict that drops a `}` silently drops the rest of the user-agent sheet.
- A section upstream has: delete it from `UPSTREAM.patch` and from here, naming the upstream PR below.

## Screenshot correctness follow-ups

- `layout/paint_tree.rs`: keep out-of-flow boxes in their structural stacking
  context even when their positioning containing block is outside that context.
  Resolve the relative paint offset against both positioning chains and keep
  cached paint contributions free of duplicates. This is a targeted local fix
  for the issue addressed more broadly by upstream PR #807:
  https://github.com/DioxusLabs/blitz/pull/807 (currently conflicts with main).
- `node/node.rs`, `layout/paint_tree.rs`: root-element detection for fixed boxes,
  adapted from https://github.com/DioxusLabs/blitz/pull/1224.
- `layout/table.rs`: auto-table width preferences, backported from
  https://github.com/DioxusLabs/blitz/pull/1345 (no duplicate PR opened).

`UPSTREAM.patch` carries these sections with the rest (it is regenerated from `src/` and `assets/`
against the upstream commit above); they are covered by pyro-render's rendering tests.

## Fidelity fixes from the ToS screens (bench/fidelity)

- `layout/table.rs` (`collect_table_cells`): `max-width` no longer applies to a table cell. CSS 2.1 leaves it
  undefined and Chromium ignores it: a cell spans its column however the column grew. ToS's finance table puts
  `max-width: 60px` on a header cell over a 60px `<col>` that grows to 63.4px, so the header's border stood
  3.4px left of the body's. Repro `bench/fidelity/repros/table-cell-max-width.html`.
- `node/serialize.rs` (`replace_current_color`): the `currentcolor` keyword is resolved in any case when an inline
  `<svg>` is serialized for usvg (which knows only `currentColor`). CSSOM, and so React, writes
  `style="fill: currentcolor"`: those icons were filled black. Repro
  `bench/fidelity/repros/svg-currentcolor-keyword.html`.
- `layout/construct.rs` (`input_placeholder`), `node/text.rs` (`InputPlaceholder`), `layout/mod.rs`,
  `layout/damage.rs`, `mutator.rs`, `assets/default.css`: an `<input>`/`<textarea>`'s `placeholder` is laid out
  with its editor, in the style Stylo computes for its `::placeholder` (a lazy pseudo-element), with Chromium's UA
  rules (colour #757575, `white-space: pre` on inputs, a `normal` line, a single-line input's text without line
  breaks); `opacity` goes into the colour. blitz-paint paints it while the value is empty. A textarea's wraps at
  its width (layout); a change of the attribute builds the box again; a new scale or font set shapes it again.
  Repro `bench/fidelity/repros/input-placeholder.html`, tests `crates/pyro-render/tests/input_placeholder.rs`.
- `layout/inline/build.rs` (`line_end_ellipsis`), `node/text.rs` (`TextLayout::ellipsis`, `ellipsis_text`): an
  inline root that clips its inline overflow and has `text-overflow: ellipsis` on its end side (Stylo keeps a single
  value as `second`) gets "…" shaped in its own style; blitz-paint ends each overflowing line with it. Repro
  `bench/fidelity/repros/text-overflow-ellipsis.html`. `text-overflow: "<string>"` shapes the string instead, on
  one line (white space collapsed; U+000A to U+000D, U+0085, U+2028 and U+2029 count as spaces), in the base
  direction of the root's lines (WPT `css-overflow/text-overflow-string-*`). A text control (`layout/construct.rs`
  `create_text_editor`, `TextInputData::ellipsis`) gets the ellipsis of its own `text-overflow` too, whatever its
  `overflow` (it always clips); blitz-paint draws it while the control is neither focused nor scrolled. A
  `<textarea>`'s lines that do not wrap are its scrollable overflow (`layout/mod.rs`, the textarea leaf), so
  script can scroll to them (pyro-dom `scroll_extent` lets a textarea scroll). An anonymous block (lines beside a
  block-level sibling) takes the ellipsis of its parent's style, which its own does not inherit.
- `layout/line_clamp.rs`, `layout/mod.rs` (`compute_child_layout_in_current_wm`), `layout/inline/layout.rs`,
  `node/text.rs` (`TextLayout::clamp`, `line_box_top`, `counted_lines`, `LineClamp`), `node/node.rs`
  (`NodeFlags::CLAMPS_LINES`): the legacy line clamp (`-webkit-line-clamp` on a vertical `-webkit-box`; Stylo's
  `WEBKIT_LINE_CLAMP_APPLIES`). After the box's own layout its line boxes are counted in tree order: its own and
  those of in-flow block descendants in its formatting context (not floats, positioned boxes, scroll containers,
  fieldsets, flex/grid/table boxes). With more lines than the clamp, the block holding the clamp line gets a
  `LineClamp` (lines kept, "…" shaped in its style) and an auto-height box ends at that line's bottom (within
  `min-`/`max-height`). A measuring run of such a box runs as a full layout and drops the box's layout cache,
  since the count reads laid-out content. A box that stops clamping clears the mark (`CLAMPS_LINES`). Lines in
  later blocks are left to the box's `overflow`. Tests `crates/pyro-render/tests/line_clamp.rs`.
- `assets/default.css`: `img, video, canvas { overflow: clip }`, as Chromium's
  UA sheet: replaced content is cut at its content box unless the author's `overflow: visible` says otherwise
  (blitz-paint `replaced_overflow_is_visible`). Layout ignores a replaced element's overflow.
- `node/svg.rs` (`SvgImageData::sized`, `inline_source`), `layout/construct.rs`: an inline `<svg>` without a
  `viewBox` keeps its source, and is parsed again (cached) with the root's `width`/`height` set to its CSS
  content box for painting: its viewport is the box, as in Chromium, so user units are CSS px and percentages (a
  `<use>` of a `<symbol>`, 100% by default) resolve against it. Parsed alone, usvg resolved them against its
  100x100 fallback and shrank the tree to its content's bounds, which drew sprite icons too large. Repro
  `bench/fidelity/repros/svg-use-symbol-size.html`, tests `crates/pyro-render/tests/inline_svg_viewport.rs`.
- `layout/inline/build.rs` (`span_edge_widths`, the `(Inline, Flow)` branch), `layout/inline/layout.rs`,
  `layout/damage.rs`: the inline-axis margin, border and padding of an inline box that is not atomic take
  room in its line, as in Chromium: a badge (`<span>` with padding and a border) was as wide as its text, so
  its background ended at the glyphs and the text after it stood too far left. Each side with a width is
  pushed as one of parley's span edges (`InlineBoxKind::SpanStart`/`SpanEnd`, see parley's README) with the
  element's node id; the border counts only where the side has a border style (the used width, as
  `stylo_taffy` takes it for boxes). Sizing, float placement and box positioning pass over the edges, and a
  style change that changes a side's width builds the inline root again (a paint-only restyle otherwise).
  Percentages resolve to 0. Block-axis padding and border do not move lines (CSS 2.1 10.8.1); blitz-paint
  paints them. Repro `bench/fidelity/repros/inline-box-badges.html`, tests
  `crates/pyro-render/tests/inline_box_decorations.rs`.
- `layout/construct.rs` (`input_line_is_normal`, `create_text_editor`), `layout/mod.rs`: a single-line input's
  text is laid out at `line-height: normal` where Chromium's inner editor is
  (`TextControlInnerEditorElement::CreateInnerEditorStyle`): when the line height (whole pixels, cut) is no more
  than the font size, when the input has a fixed height above the line height, and when its height is a
  percentage or a `calc()`. An input of `height: auto` keeps its line height, and is as tall as a `normal` line
  when the line height is no more than the font size. In a 33px input of `13px/1.43` system-ui Chromium centres
  a 16px line, not an 18.59px one: the value stood a CSS pixel high. The placeholder's line is `normal` already.
  `node/node.rs` (`text_input_v_centering_offset`): the line is centred in the unrounded content box, as Chromium
  centres in layout units; in the box rounded to whole pixels a 33.5px input's line was 8.5px down (painted from
  9px) where Chromium's is 8.25px (8px). Repro `bench/fidelity/repros/text-line-tops.html`, tests
  `an_inputs_line_is_normal_below_a_fixed_height` and `an_inputs_value_starts_at_a_whole_css_pixel` in
  `crates/pyro-render/tests/text_line_tops.rs`.
- `layout/list.rs` (`node_list_item_child`): an outside `disc`, `circle` or `square` marker is laid out as a
  line of the item's font (an "x"), not as a glyph of the bullet font: blitz-paint paints these markers as
  shapes sized from that font's ascent, as Chromium does. Test
  `a_list_markers_symbol_is_a_shape_sized_from_the_font_at_whole_css_pixels` in
  `crates/pyro-render/tests/text_line_tops.rs`.
- `layout/table.rs` (`row_tracks`): in a collapsed table whose borders are laid out as gaps (uniform borders), a
  row's `height` covers half of the border above it and half of the one below, as a cell's does, so its track
  is the height less the gap. Rows of 33, 41 and 37px with 1px borders were 34, 42 and 38px apart where
  Chromium's are 33, 41 and 37px. A table whose cells keep half borders (see "Collapsed tables without uniform
  borders") was right. Repro `bench/fidelity/repros/text-line-tops.html`, test
  `a_rows_height_in_a_collapsed_table_covers_half_of_the_borders_around_it` in
  `crates/pyro-render/tests/table_cell_height.rs`.

## Stacking contexts from `will-change`

- `node/node.rs` (`is_stacking_context_root`): `will-change` of a property that would make a stacking context
  makes one, as the spec asks and Gecko does (stylo's `STACKING_CONTEXT_UNCONDITIONAL` properties: `filter`,
  `backdrop-filter`, `mix-blend-mode`, `clip-path`, `isolation`, `mask-image`, `position`; and `opacity`, the
  transform properties and `perspective`): its positioned descendants paint inside it (WPT
  backdrop-filter-image-size-filter-size-mismatch: an absolutely positioned box's backdrop root).

## The page's CSS on inline SVG

`node/svg_css.rs` (new; `node/serialize.rs` `svg_source` calls it): an inline `<svg>` is serialized for usvg
with the page's CSS applied, as Chromium styles SVG content. usvg resolves presentation attributes, `style`
attributes and the `<style>` sheets inside the SVG, but never saw the page's sheets: `rect { fill: blue }`,
`transform-box`, a CSS `clip-path` or `mask-image` on an SVG element did nothing (WPT
`css/css-transforms/transform-box`, `transform-origin`, `*-styles`, `css/css-masking/clip-path*`).

- Each SVG element's properties set by author-level declarations (style sheets, `style` attributes,
  animations; not the UA sheet or presentational hints, found by walking its rule node) are written with their
  computed values: painting, text, `opacity`, `display`, `visibility`, `mix-blend-mode`, `isolation`,
  `filter` and `mask-type` as declarations appended to the element's `style` attribute (usvg ranks those above
  presentation attributes, as CSS does; its own `style` content stays first for what Stylo does not parse),
  geometry properties (`x`, `y`, `width`, `height`, `cx`, `cy`, `r`, `rx`, `ry`, `d`) as the attributes of the
  elements they apply to. `url()` values are written as `url(#fragment)`, the only form usvg follows.
- Transforms (`transform`, `translate`, `rotate`, `scale`, `transform-origin`, `transform-box`, or the
  `transform` attribute when only the origin or box comes from CSS) are resolved here, as usvg knows neither
  CSS transform syntax nor `transform-box`: `stylo_to_kurbo::resolve_2d_transform` about the origin resolved in
  the reference box (the `viewBox` size of the nearest `<svg>`, at its origin; or the element's fill or stroke
  box), written as a `matrix()` (`patternTransform` and `gradientTransform` on patterns and gradients). The
  origin of an SVG element without a CSS one is its `transform-origin` attribute or `0 0`.
- A CSS `clip-path` basic shape (`circle()`, `ellipse()`, `inset()`, `polygon()`, `path()`) or box becomes a
  `<clipPath>` of its own in its reference box (by default the stroke box, as Chromium maps `border-box`), its
  shape written as the SVG element usvg builds the same geometry from (`<circle>`, `<ellipse>`, `<rect>` with
  radii, `<polygon>`); `url(#id)` and `none` are written as they are. `mask-image: url(#mask)` becomes the
  `mask`.
- `stylo.rs` (`build_presentational_hints`): an SVG element's `clip-path` and `transform-origin` attributes are
  presentational hints of the CSS properties (Chromium's presentation attributes), so a basic shape, CSS units
  and keywords in them reach usvg as above, and an invalid one (`transform-origin="top 100%"`) is ignored, as in
  CSS, where usvg read some of them. Other presentation attributes stay usvg's.
- Declarations of the properties written here are taken out of the element's own `style` attribute: Stylo
  parsed them, and those it rejected (`transform: scale(invalid)`) must not reach usvg, which read `transform`
  there as an invalid transform. The `font` shorthand goes too (its longhands are written): usvg's tokenizer
  stopped at `font: 40px/1 Ahem` and dropped every declaration after it. The computed declarations come first,
  then what is left of the element's own (`stop-color`, `marker-end`, ... which Stylo does not parse for servo).
- The root gets the inherited painting and text properties it has from its HTML ancestors when they differ from
  what usvg assumes (`<div style="fill: blue"><svg>`, the page's font for SVG text). Its box effects
  (`opacity`, `clip-path`, `mask`, `filter`, `transform`, ...) are its CSS box's, which blitz-paint applies, and
  are not written.
- A shape of no geometry (a `rect` without a positive `width` and `height`, an `image` of a zero one, a `circle`
  without a positive `r`, an `ellipse` of a zero radius or neither, a `path`, `polygon` or `polyline` without `d` or
  `points`) is written with `filter:none`: it renders nothing, its filter included (SVG 2, Chromium; WPT
  `svg-empty-element-with-filter-001`), where usvg keeps the group its filter makes and a flood would show. An empty
  `<g>`'s filter still renders.
- Fill and stroke boxes are the elements' bounding boxes from usvg: when an element needs one, the subtree is
  written twice, the first time with ids on those elements, and parsed (cached) for them.
- `filter` is written in the `style` attribute (which wins over the `<svg>`'s own `<style>` sheets in usvg, as the
  page's value does), with drop shadows' colours resolved to `#rrggbbaa` (usvg knows neither `currentcolor` nor
  `color-mix()`, and its `style` tokenizer ends a function at its first `)`, which cut `drop-shadow(rgb(0, 128, 0)
  50px 0px 0px)` short and left the element unfiltered); a value that still has parentheses in parentheses is
  written as the attribute. Its lengths, and a text shadow's, are divided by the element's `zoom` (Stylo's computed
  lengths have it; user units are scaled by the `<svg>`'s box).
- A `<text>`'s `text-shadow` (one shadow) is a copy of the text written before it, in a group moved by the
  offset (and blurred by half the blur radius), whose fills and strokes that are not `none` are the shadow's
  colour, their opacities times its alpha: Chromium draws a text shadow as the text painted again, which a
  reference drawing the text twice matches to the level (a drop shadow of the text was 2 levels off at its
  edges; WPT `css-text-decor/text-shadow/svg-*`). Not in a `<switch>` (which would choose the copy, without the
  text's conditions) or a `<clipPath>` (which drops groups): only where the parent draws its children in order
  (`in_group_container`), as for the `plus-lighter` group below.
- `clip-path` and `mask` references (attributes or CSS) to an element of the `<svg>` that is no `<clipPath>` or
  `<mask>` are written as `none`: such a reference is ignored (CSS Masking), where usvg left the element out (WPT
  `clip-path-invalid-reference`, `mask-invalid-reference`). A `transform`, `patternTransform` or
  `gradientTransform` attribute that is no transform list in SVG's grammar (a comma that separates nothing:
  `rotate(90,)`, which svgtypes reads) is ignored, as in Chromium (WPT `svg-external-styles-014`).
- CSS mask layers on an SVG element (other than one `url(#mask)` of the `<svg>`, which stays usvg's `mask` whatever
  its `mask-mode`, so that its region keeps a stroke past the fill box, and a `url(doc.svg#m)` naming a `<mask>` of
  another document, which is not resolved and leaves the element unmasked) become a `<mask>` made for it: SVG
  images (embedded as `data:` URLs from the document they were loaded from, `SvgImageData::source`, which `net.rs`
  keeps for documents of up to 256 KiB) and linear gradients (as `<linearGradient>`s) as tiles placed by
  `mask-size`, `mask-position`, `mask-repeat` (`repeat`, `space`, `round`, `no-repeat`) and `mask-origin`, clipped
  to `mask-clip`, in SVG's boxes (`content-box` and `padding-box` are the fill box, `border-box` the stroke box,
  `view-box`, `no-clip`); a reference to a `<mask>` as a white rectangle over the fill box under it; a luminance
  layer as the alpha of a white rectangle under a luminance `<mask>` of its tiles; each layer above the bottom one
  in a group whose id names its `mask-composite` operator (`__pyro_compose_srcout_1`), which anyrender_svg
  composites with it. An image that failed to load is a transparent layer (as the only layer it leaves the element
  unmasked, as in Chromium: WPT `bad-mask-image-svg-2`). More than 4,096 tiles along an axis (counted before any
  is placed: a tile below the float spacing at its position never moved on) or in all leave the element unmasked,
  as do raster images and other gradients, as before. `document.rs` (`apply_loaded_image`): a mask image that
  loads for an SVG element damages its outermost `<svg>`, whose source is written again with it (WPT
  `css-masking/mask-image` `mask-clip-*`, `mask-repeat-*-svg`, `mask-position-*`, `mask-composite-2d`,
  `mask-mode-e`, `bad-mask-image-svg`).
- `mix-blend-mode: plus-lighter`, which usvg does not know, wraps the element in a group whose id names the
  operator (`__pyro_compose_pluslighter_1`).
- A 3D transform (`stylo_to_kurbo::resolve_transform`'s `Spatial`) about its origin, and for the root's children
  the root's `perspective`, is flattened into the plane (`flatten_transform`): exactly for an element that stays
  parallel to it, as a depth under a perspective is a scale about its origin (WPT `perspective-svg-child-*`). The
  `transform` attribute applies first when CSS sets no `transform` (only `rotate: y 30deg`, say), as in 2D.

Stylo computes `transform-box` for servo too since this change (see `vendor/stylo/README.pyro.md`). Tests
`crates/pyro-render/tests/svg_content.rs` and `svg_css_effects.rs`.

## `<br>` as inline text

- `node/node.rs` (`is_line_break`), `layout/construct.rs`: a `<br>` is a piece of inline text in every container, as
  in Chrome: as a flex or grid child it joins neighbouring text and `<br>`s in one anonymous item, one line each,
  instead of being blockified into an empty box. All of its CSS is ignored except `display: none` and
  `display: contents`, which remove it. In flex and grid, white space after text or a `<br>` stays when it's preserved.
  A `display: none` child no longer ends a run of inline content. Covered by `crates/pyro-render/tests/br_flex_item.rs`
  and `bench/pdf/fixtures/generators/fx-br.mjs`.

## Incremental passes for layout queries

- `src/resolve.rs` (`resolve_layout_children`): an undamaged subtree without damaged descendants is not
  walked (its layout children are those of the last pass); anonymous boxes are always visited.
- `src/resolve.rs` (`resolve_layout_children`), `src/layout/damage.rs` (`propagate_damage_flags`,
  `clear_damage_and_dirty_flags`), `src/node/node.rs` (`Node::is_clean_layout_child_of`,
  `Node::has_no_damage_below`): the walks that follow damage skip a child whose `damaged_descendants` flag is
  clear from the parent's loop (an element, for `resolve_layout_children`; any node but an anonymous box for the
  DOM walks) instead of entering it to return. This relies on the invariant `mark_damaged` keeps: a node with damage
  has its flag set, and so do its ancestors. Debug builds assert it at each skip, and assert after every resolve
  that no node in the document keeps damage (`assert_no_damage_left`). The two places that set a pseudo-element's
  damage without its flag (`flush_pseudo_elements`, creating and updating one) now call `mark_damaged`.
- `src/layout/table.rs` (`cells_to_visit`), `src/resolve.rs`: a change inside a table's cells reaches the table as
  `CONSTRUCT_DESCENDENT` without `RELAYOUT` (`propagate_damage_flags` drops `ONLY_RELAYOUT`), so the table is neither
  built again nor updated. When the table keeps its cells (`keeps_cells`), `resolve_layout_children` visits only the
  cells `note_changed_table_cell` listed as changed in layout (`TableContext::changed_cells`, read before the grid
  takes it) and the pseudo-elements. Any other cell has nothing to build: construction below a cell reaches it as
  layout damage. Debug builds assert that every unlisted cell is clean or has no layout damage. Before this, a cell
  edit in a 1,200-row table entered all 4,800 cells to return, about a quarter of the CPU of an edit-and-measure
  loop. Covered by `a_deep_cell_edit_beside_cells_with_flags_but_no_layout_change_matches_fresh_builds` and the
  other table mirror tests in pyro-render.
- `src/resolve.rs` (`resolve_transforms`): a node's cached scrollable overflow is reused only when it and
  its subtree are undamaged (a descendant's relayout reaches ancestors as damage, not as
  `RECALCULATE_OVERFLOW`).
- `src/layout/writing_mode.rs`, `src/layout/mod.rs`, `src/node/element.rs`: incremental rounding, keyed by
  `LayoutData::rounded_from` (see "Incremental rounding" in `vendor/taffy/README.pyro.md`).
  `Node::update_unrounded_layout` is the only writer of unrounded layouts, and marks changes as damage. A box that
  only moved (its location alone changed: the rows below a table cell that grew) is marked on its ancestors, not on
  itself: what it holds in its own coordinates (scrollable overflow, its subtree's layouts) is unchanged, so
  `resolve_layout_children`, `resolve_transforms` and `clear_damage_and_dirty_flags` no longer descend into every
  moved subtree. Its rounding still follows the move: the `rounded_from` key holds its location. The key's offset
  is the fractional part of the parent's absolute position, and `rounded_layout` rounds relative to the pixel grid
  (half up, as Blink), so the subtree of a box moved by whole pixels is not rounded again either: an edit of a cell
  in a 200-row table reaches the cells of the rows below, not what is in them
  (`rounding_skips_subtrees_moved_by_whole_pixels`). `Node::mark_layout_changed` stops at a node already flagged,
  as `mark_damaged` does: a flag is only set with its ancestors' and cleared after those below it, which debug
  builds assert at both early exits (`debug_assert_ancestors_flagged`; anonymous boxes' flags are not maintained
  and are passed through). The rows below a cell that grew walked to the document each, thousands of times per edit.
  A transform resolves against the rounded size (`Node::set_transform`), which a fractional move of the box or an
  ancestor can change: `LayoutPassState::set_final_layout` marks a box with a transform style
  (`Node::has_transform_style`; one that was the identity at the old size is stored as no transform) whose rounded
  size changed (`Node::mark_layout_changed`), so `resolve_transforms` resolves it again instead of reusing the old
  one. Nothing else the passes after layout keep depends on the rounded size or the absolute position: scrollable
  overflow uses unrounded sizes, and paint (radii, backgrounds), sticky offsets and pyro-render's geometry derive
  positions from the layouts when they are used.
- `src/resolve.rs` (`resolve_layout`): no Taffy pass or rounding with nothing damaged and the same viewport.
  `BaseDocument::needs_resolve` tells a caller whether a `resolve` could change anything.
- `src/node/node.rs`, `src/resolve.rs` (`resolve_layout_children`): `Node::layout_child_of` names the node whose
  `layout_children` list holds a node, entered and left where those lists are rebuilt (a child another container
  collected in the same pass keeps pointing there). A node is a box of the current layout when it equals its
  `layout_parent`, which inline content of an inline root also has without being listed, or when it is a hoisted
  (out-of-flow) box whose `oof_containing_block` is its `layout_parent`: pyro-render answers that in O(1) instead of
  indexing the parent's list (a table's cells) after every sync. The incremental table update keeps it too: when it
  lists a table's cells again (`update_rows`) and when a former box becomes a table part (`drop_part_box`), the
  boxes that left a list leave it (`leave_layout_children`) and the new ones enter it.
- `src/layout/paint_tree.rs` (`build_paint_tree_impl`): a rebuilt paint list already in paint order (tree order with
  no positioned, floated or z-indexed children: a table's cells) is not sorted again.
- `src/stylo.rs`, `src/document.rs` (`BaseDocument::has_sticky`): the style traversal records the elements it styles
  `position: sticky`, and drops the entries whose node is gone or no longer sticky after each traversal, so whether a
  document has sticky boxes is not a scan of every node per sync. Pseudo-elements are not recorded (pyro
  reports sticky offsets for elements only).

pyro-render's mirror tests (incremental documents against fresh builds after random mutations) cover them.
- `assets/default.css`: text inputs take Chromium's UA box (`border: 2px inset`, `padding: 1px 2px`; a
  40x30 input is 48x36), textareas keep `1px solid` / `2px` and wrap like Chromium's
  (`white-space: pre-wrap; overflow-wrap: break-word`: a word longer than the box breaks).
- `src/document.rs`: `set_hover_node` puts `:hover` on a node and its ancestors (an embedder that tracks
  the pointer itself: pyro mirrors pyro-dom's hovered element).
- `src/document.rs`: Stylo's prefs for `:nth-child(An+B of S)`, container and style queries,
  `@starting-style`, `@scope`, `line-clamp`, `fit-content()` and `@supports at-rule()` are on, as in
  pyro-dom's cascade. The prefs are process-wide and read while sheets are parsed, so Blitz used to drop
  those rules unless pyro-dom's cascade had been created first.
- `src/document.rs`, `src/mutator.rs`, `src/form.rs`: a URL that cannot be resolved against the
  document's (`src="x"` in a `data:` document) loads nothing instead of panicking, as in browsers.
- `src/resolve.rs`: `resolve_style` runs the style pass of `resolve` alone (messages, device changes,
  the traversal), leaving damage, box construction and layout to the next `resolve`: pyro
  answers `getComputedStyle` from Blitz's styles without laying the document out.
- `src/mutator.rs`: a child moved to another parent leaves its old one as `remove_node` does: the old
  parent's `:empty` and `:has()` anchors are restyled (they were not, so `ul:empty` stayed unmatched
  after its last child moved out).
- `src/stylo.rs`, `src/mutator.rs`: an inserted or removed element invalidates `:has()` anchors through
  Stylo's `RelativeSelectorInvalidator` (`invalidate_relative_selectors_for_insertion`/`_removal`, as
  Gecko's `MaybeInvalidateRelativeSelectorForInsertion`/`ForRemoval`), which looks the element's and its
  descendants' tags, ids, classes and attributes up in the stylist's relative selector invalidation map
  and restyles an anchor itself (and what the rest of its selector reaches). An argument's `:empty`,
  `:nth-child()`, `:first-child` or `:last-child` matches by the tree around an element, not by its
  identity, so `restyle_for_child_list_change` looks the parent (`:empty`) and the siblings whose
  position changed up in the stylist's tree-structural dependencies
  (`invalidate_relative_selectors_for_ts_state`, as Gecko's
  `MaybeInvalidateRelativeSelectorForEmptyDependency`/`ForNthEdgeDependency`/`ForNthDependencyFromSibling`);
  `remove_and_drop_all_children` invalidates for the dropped elements as `remove_node` does. Blitz's own
  walk (`invalidate_has_anchors`, kept for attribute and state snapshots) restyled the subtree of every
  anchor that could see the position on every insertion: a `.sheet:has(.top)` page restyled whole for
  each row appended to it. `each_custom_state` is a no-op (it was `todo!()`; the invalidator asks).
- `src/document.rs`, `src/util.rs`: the documents of a thread share one Stylo lock, and an author sheet
  without `@import` is parsed once per thread for its text and base URL (`make_stylesheet`): every
  document of a PDF template or a test suite carries the same bundle, which each new document parsed
  again. Sheets are not edited through Blitz's CSSOM in pyro (its CSSOM edits the text).
- `src/net.rs`: decoded raster images are kept by their encoded bytes for every document of the process
  (64 images, 64 MB of pixels): a template's logo is decoded once, not once per document.
- `assets/default.css`, `src/layout/mod.rs`: a drop-down `<select>` is an inline-block box with a 1px
  border and `line-height: normal`, sized from its longest option label (about half an em per character)
  plus the arrow, a line of its font tall (Chromium's menu list). Its baseline is its text line's, centred
  in the content box (as for text inputs), not its bottom edge.
- `assets/default.css`, `src/layout/construct.rs`: `option` and `optgroup` compute as Chromium's (`display:
  block`; Blitz had `option { display: none }`). A drop-down select's children generate no boxes (Chromium's
  menu list); a list box's (`multiple`, `size` > 1) and loose options are laid out.
- `src/stylo.rs`: `BaseDocument::pseudo_style` reads an element's `::before` / `::after` style (resolved on
  demand inside `display: none`, or for one the traversal did not keep), for `getComputedStyle(el, pseudo)`
  and the generated content of accessible names.
- `src/layout/mod.rs`: a text input in system-ui on macOS (SF Text) is `ceil(0.525em x size) +
  round(0.4375em)` wide, Chromium's width there (measured at 10-16px); other fonts keep the
  `(size / 2 + 0.4)em` approximation.
- `src/layout/table.rs`: table rows grow under a rowspan cell as in Chromium (through taffy's
  `grid_rows_are_table_rows`), and a rowspan stops at the end of its row group instead of adding implicit
  rows past it.
- `src/stylo.rs`: `BaseDocument::pseudo_element_style` answers `getComputedStyle(el, pseudo)`: the stored
  eager `::before`/`::after`/`::first-letter`/`::selection` style (resolved on demand under `display: none`,
  like `resolve_undisplayed_style`; a `::before`/`::after` the traversal dropped for having no `content` is matched
  against its rules on demand), Stylo's lazy cascade for `::marker`, `::placeholder` and the other lazy
  pseudo-elements, else a rule-less style inheriting from the element. `generated_only` limits it to the
  stored eager style (no work otherwise), for accessible names.
- `assets/default.css`: only a `<select>`'s options and optgroups are `display: none` (the control paints
  them); an `<option>` elsewhere and every other `<optgroup>` are blocks, as in Chromium (Blitz hid every
  option and left optgroup inline).
- `assets/default.css`: checkboxes and radios are Chromium's 13x13 border boxes (radios with Chromium's
  `3px 3px 0 5px` margin); Blitz had 14x14.
- `src/layout/mod.rs`: a textarea or select with a percentage width contributes zero to min-content sizing,
  as text inputs already did (CSS Sizing 3 §5.2.2, compressible): in a flex-wrap row it shrinks into the
  space left on the line instead of wrapping below.

`src/resolve.rs`: scrollable overflow as in Blink: an empty rect adds nothing, a box fixed to the viewport is left out, and `overflow` on `body` is the viewport's (`BaseDocument::body_overflow_goes_to_viewport`). `src/layout/list.rs`: `decimal-leading-zero`, `lower-roman`, `upper-roman`, `lower-latin` and `upper-latin` markers.

`src/layout/table.rs`: auto tables with percentage cells or `<col>`s distribute widths as css-tables and Blink do (`TableTreeWrapper::percent_column_widths`).

`BaseDocument::set_layout_size` lays out at a fractional CSS size instead of the viewport's whole pixels (`make_device`); printing uses it for the paper's content box.

`src/resolve.rs` (`phase_profile`): cumulative time per `resolve` phase and counters (resolves, restyles,
rebuilds, tables, rounding) read by pyro-render's profiling. The phase timers run only while
`phase_profile::set_timing(true)` (pyro-render's `profiling::enable`, turned on by `Pyro.geometryStats`,
`Performance.enable` and `PYRO_GEOMETRY_PROFILE`) is on: `mark` is a no-op otherwise. The counters always count.

## 3D transforms flattened

Upstream dropped every transform whose matrix is not 2D: the element painted untransformed.

- `src/stylo_to_kurbo.rs` (`resolve_transform`, `flatten_transform`, `is_back_face_visible`,
  `resolve_perspective_matrix`): a 2D transformation matrix resolves as before; a 3D one (a 3D matrix of the
  `transform` list, `translate` with a depth, `rotate` about another axis than z, `scale` with a z factor) is the
  4x4 matrix in f64, `transform-origin`'s depth included, flattened onto the plane of the screen as browsers
  composite an element whose `transform-style` is `flat`: the 3x3 matrix of rows and columns 1, 2 and 4.
  - Where W does not vary over the box (no perspective, or a perspective with the box parallel to the
    screen), that is an exact affine: `rotateY(60deg)` paints at half width, and a depth under a perspective
    scales the box about the perspective origin.
  - Otherwise (a rotation out of the plane under perspective) the affine closest to the projection at the
    box's corners (least squares) stands in for culling and scrollable overflow; blitz-paint draws the
    trapezoid through the 4x4 matrix and layout queries place it by its homography (`Plane3D`, below). A box
    that crosses the viewer's plane (`Flattened::Crossing`, the part in front of it non-empty: `clipped_projection`,
    cut at W = `MIN_W`) is drawn through its matrix too; its projection has no bounds, so layout keeps its
    untransformed box (no transform), and print, which draws stand-ins, leaves it out.
  - A box wholly at or behind the viewer, or whose 4x4 matrix has no inverse (`scale: 1 1 0`), is not drawn.
  - Coefficients within 1e-12 of 0 and ±1, and translations within 1e-9 px of a whole pixel, snap
    (`snap`): `rotate(360deg)` left `sin` noise that made text skip its pixel snapping.
- `src/node/node.rs` (`set_transform`, `transform_context_for_children`, `TransformContext`,
  `transform_reference_box`, `scroll_offset_changed`, `NodeFlags::TRANSFORM_HIDES`,
  `NodeFlags::GIVES_3D_CONTEXT`, `NodeFlags::FLATTENS_3D`):
  - A 3D transform takes its parent's `perspective` (about its `perspective-origin`, which does not scroll),
    offset by the box's location less the parent's scroll offset (CSS Transforms 2, the accumulated 3D
    transformation matrix). Only the element's children in the DOM take it (WPT
    `perspective-children-only-*`): not an out-of-flow box it contains further down.
  - In a 3D rendering context (a parent with `transform-style: preserve-3d`) an element's plane is the
    projection of its accumulated matrix, which the parent's enters (a child turned -60deg in a parent
    turned 60deg faces the viewer at full width), stored relative to the parent's place
    (`Context3D::base`), so paint and layout queries, which compose transforms down the tree, land on
    it. A `preserve-3d` element whose plane is edge-on (`rotateX(90deg)`) keeps a negligible thickness
    (`regularize`) so its children can still be placed relative to it. Its planes are not sorted by
    depth: they paint in tree order.
  - The used `transform-style` is `flat` under a grouping property (CSS Transforms 2: `overflow` other
    than `visible` and `clip`, `opacity` below 1, a `filter`, `clip-path` or mask, `isolation: isolate`,
    a `mix-blend-mode`, paint containment): `Node::is_preserve_3d`.
  - `backface-visibility: hidden` with the back face showing (Chromium's `IsBackFaceVisible`: the cofactor
    of m33 against the determinant) sets `TRANSFORM_HIDES`, which blitz-paint reads, as do a box behind the
    viewer and a non-invertible matrix. In a 3D rendering context the accumulated matrix decides which face
    shows (a card flip: a child turned 180deg in a parent turned 180deg faces the viewer). A `preserve-3d`
    element itself is not hidden: its children are planes of their own, which paint would hide with it. Its
    own painting is (`Plane3D::own_hidden`, lane H2: WPT `backface-visibility-with-sibling-001`).
  - `transform-box` picks the reference box: a CSS box's content box for `content-box` and `fill-box`, its
    border box otherwise.
  - A flat element whose transform is a 3D matrix flattened (drawn, not hidden) sets `NodeFlags::FLATTENS_3D`:
    blitz-paint has such an element that holds elements rasterized in its own plane, as Chromium composites
    it.
- `src/resolve.rs` (`resolve_transforms`, `table_cells_overflow`, `ContainingTransform`): each box resolves its
  transform with what its parent gives (`ContainingTransform::for_child`). While a parent gives its children a
  perspective or a 3D rendering context (or stopped giving one in this pass) their transforms are resolved
  again even when they are undamaged: they may have moved, or the parent changed. A scrolled parent that
  gives one is marked (`Node::scroll_offset_changed`, called from `scrolling.rs` and pyro-render's mirror), so
  the next pass reaches it.
- `src/node/node.rs` (`is_stacking_context_root`): `perspective` and `transform-style: preserve-3d` make a
  stacking context.
- `src/layout/paint_tree.rs` (`build_paint_tree_impl`): a stacking context that is not positioned (a
  transform, an opacity, a filter) paints as a positioned box with `z-index: 0` does (CSS 2.1 Appendix E step
  8): above the in-flow content and floats, in tree order with the positioned boxes, where it painted with
  the blocks. `z-index` applies to positioned boxes and to flex and grid items only: such a stacking context
  keeps its place in tree order whatever its `z-index` (WPT `css/css-transforms/z-index-does-not-apply.html`).
  pyro-dom's hit testing already ordered them so (`Layout::paint_step`). Positioned boxes a box that is no
  layer root lays out (the anonymous block holding a positioned or filtered inline's out-of-flow boxes) are
  lifted to the nearest layer root's positioned level, as in-flow positioned boxes are
  (`attach_hoisted_past_parent`): they painted under the blocks after the inline (WPT
  `filtered-inline-is-container`).
- `src/layout/paint_tree.rs` (`place_hoisted`, `tree_order_place`, `precedes_in_tree_order`): an out-of-flow box
  its containing block lists past its layout parent takes its place in tree order among what the same layout
  child's subtree lifted to that level (positioned boxes, stacking contexts without a `z-index`), where it was
  appended after all of them (CSS 2.1 Appendix E step 8; WPT `clip-path-reference-restore`,
  `clip-path-path-with-zoom-001`, `canvas-change-opacity`, `position-relative-016`).
  `restore_out_of_flow_stacking_owners`: one without a `z-index` moved into a stacking context its containing block
  lies outside of joins that context's `paint_children` at its positioned level in tree order (and
  `StackingContext::positioned_level`, for `hoisted_content_bbox`), where it was painted after the context's
  content (WPT `mix-blend-mode-root-element-group`). The search first compares with the last box of the same level
  (in tree order already, the usual case: no search, no allocation). `paint_tree_snapshot` (the debug check that
  the incremental tree equals a fresh build) compares `positioned_level` too. Paint order is covered by
  `crates/pyro-render/tests/opacity_group.rs`. `src/node/node.rs` (`hit`): an out-of-flow child placed so is hit
  where it is painted (`lifted_paint_offset`), under a window scroll too: covered by
  `a_fixed_box_moved_into_a_stacking_context_is_hit_where_it_is_painted` in `crates/pyro-render/src/lib.rs`.

Covered by `crates/pyro-render/tests/transform_3d.rs` (paint, a scrolled perspective, the card flip, a
composed 3D rendering context, grouping properties, a filtered inline's positioned boxes, layout boxes), `crates/pyro-layout/tests/transform_3d.rs` (`getBoundingClientRect`, `elementFromPoint`) and the unit
tests in `src/stylo_to_kurbo.rs`.

### Planes drawn through their 4x4 matrix (lane H2)

- `src/node/node.rs` (`Plane3D`, `Node::plane_3d`, `set_transform`, `Context3D::root`, `Context3D::root_offset`),
  `src/node/element.rs` (`ElementData::plane_3d`): `set_transform` gives blitz-paint the elements it draws as
  planes of their own, each with its 4x4 matrix from its border box to the untransformed border box of the root of
  its 3D rendering context (CSS px, depth and perspective divisor kept):
  - every element of a 3D rendering context that Chromium gives a transform node: a transform property (even an
    identity one), one of `translate`, `rotate`, `scale`, `will-change: transform`, `transform-style: preserve-3d`,
    `backface-visibility: hidden`. The other elements of a context paint in the plane of the element they are in,
    in paint order, at their layout place there: they get no transform (WPT `preserve3d-and-flattening-*`);
  - the root of a context (a `preserve-3d` element whose parent gives it no context): its matrix is its own;
  - a flat element outside a context whose 3D transform maps it projectively (`is_projective`: W varies over
    the box).
  blitz-paint sorts a context's planes by depth and draws projective ones through a perspective warp; the 2D
  transform (`Node::transform`) stays the stand-in that layout queries compose.
- `src/node/node.rs` (`Plane3D::local`, `Plane3D::places_by_local`, `Context3D::frame`,
  `Node::hit_bound_in_parent`), `src/resolve.rs` (`resolve_hit_overflow`): a plane drawn through a perspective, and
  any plane of a context element placed so, also carries the homography that places it in its parent's
  coordinates as layout queries place the parent (the parent's frame: its own homography, or its 2D stand-in when
  it has none), which pyro-render's layout queries compose instead of the stand-in. The chain of a context's
  planes so composes to each plane's own projection, the perspective applied once. A box's hit-test bound in its
  parent holds both that projection and the stand-in's placement (what is placed by the stand-in, inline fragments
  and table rows, stays inside it), so hit testing culls nothing paint draws. Scrollable overflow keeps the
  stand-in. A plane crossing the viewer's plane is not placed so (`Plane3D::crosses_viewer`).
- `src/resolve.rs` (`ContainingTransform::for_child`): a parent without a box passes its parent's perspective and
  3D rendering context on: a `display: contents` element always (WPT `transform3d-preserve3d-014`), a table row or
  row group (Blitz lays a table's cells out in the table's grid) when its own `transform-style` is `preserve-3d`
  (WPT `transform-table-009`, `-010`); one that is not ends the context (`-007`, `-008`).

Covered by `crates/pyro-render/tests/three_d_contexts.rs`, `crates/pyro-layout/tests/transform_3d.rs` and the unit
tests in `src/stylo_to_kurbo.rs`.

## Style changes for transitions

- `src/stylo.rs`, `src/document.rs`: while `BaseDocument::set_track_style_changes(true)`, the style traversal
  records every element whose primary style it replaces, with the style before and after
  (`StyleChange`; one restyled again before the embedder reads keeps its first old and last new style);
  `take_style_changes` hands them over. A first style, or styles dropped, is no change. pyro-webapi's
  transition events read them instead of scanning the document for elements that may transition on every
  style flush: the cost is the elements restyled, as in Chrome, where transitions start in style recalc.

## Animated values the embedder samples into the cascade

- `animation.rs` (new), `document.rs`, `node/element.rs`, `lib.rs`, `stylo.rs` (`animation_rule`,
  `transition_rule`, `may_have_animations`, `has_animations`, `LayoutThreadScope`), `resolve.rs`: pyro keeps the
  timing of CSS animations, CSS transitions and Web Animations itself (its UA sheet sets `animation` and
  `transition` to `none !important`, so Stylo's servo animations never start) and hands the document, per element,
  the keyframe effects in effect with their iteration progress (`BaseDocument::set_sampled_animations`). The
  document resolves each effect's keyframes against the element's base style (its rules without the animations and
  transitions levels, cascaded; a CSS animation's from the `@keyframes` rule Stylo finds for the element, so `em`,
  `currentcolor`, `var()` and `inherit` resolve as for the element) and samples them as Web Animations samples a
  keyframe effect: the effect's easing (with its before flag), neutral 0%/100% keyframes taking the underlying value,
  the interval's keyframe easing, `add`/`accumulate`, values that do not interpolate flipping at the midpoint. The
  values go on the element as two declaration blocks that `animation_rule` and `transition_rule` answer, so Stylo
  cascades them at their levels, and an element with them is kept out of style sharing. An element is restyled only
  when its values change; resolved keyframes are kept until the base rules or the parent's style change, checked
  after each traversal, where the elements styled anew get their values and one more traversal
  (`resolve_sampled_animations`). Pseudo-elements are not animated. Tests:
  `crates/pyro-render/tests/sampled_animations.rs`.
- `animation.rs` (`BaseDocument::animates_backdrop_root`): whether an element's sampled values (an effect in effect,
  a filling one after it finished too) set a property that makes a backdrop root (`opacity`, `filter`,
  `backdrop-filter`, `mix-blend-mode`, `clip-path`, `mask-image`): Web Animations has the element act as though it
  had `will-change` of it, and blitz-paint marks it a backdrop root (WPT
  `backdrop-filter-backdrop-root-animation-in-effect`).
- `animation.rs` (`ElementEffects::current`, `Sampled::stacking_context`), `node/element.rs`
  (`animation_stacking_context`), `node/node.rs` (`is_stacking_context_root`): an animation that is current (in its
  delay too, where the embedder lists it under `current`: it gives no value) or in effect, of a property that makes
  a stacking context (`opacity`, the transforms, `perspective`, `filter`, `backdrop-filter`, `clip-path`,
  `mask-image`, `mix-blend-mode`, `isolation`), makes its element one, as Web Animations' `will-change` side effect
  has it (WPT `individual-transform/stacking-context-001`). A change of it damages the element
  (`REBUILD_STACKING_CONTEXT`). Tests: `crates/pyro-render/tests/sampled_animations.rs`.

## Inline layouts in vertical writing modes

Layout keeps a vertical inline root's lines logical (x inline, y block). `Node::vertical_block_axis` says which
way its block axis runs; paint turns the lines, and these read them physically:

- `src/resolve.rs` (`resolve_transforms`): the lines' scrollable overflow.
- `src/node/node.rs` (`inline_fragment_boxes`): inline boxes' rects and `offsetLeft`/`offsetTop`.
- `src/layout/writing_mode.rs`: `physical_unrounded_layout` for callers outside this crate (`unrounded_layout` is in
  the writing mode of the box that placed the node); `physical_unrounded_geometry` is public for paint.

`sideways-lr` lines run bottom to top from the content box's bottom, their line-over side on the left (paint turns
them counterclockwise): `inline_fragment_boxes` and the lines' overflow (`line_bounds`, a scroll container's end
padding the top one) mirror the inline axis there, and an emphasis mark's `right` is under the line. Inline layout
itself still runs top to bottom in `sideways-lr`: atomic inlines (their own boxes), the static positions of
out-of-flow boxes and floats are placed top to bottom, so a line beside a float, which takes its slot from the top,
is painted from the bottom where the float is (a known gap for inline layout: mirror the slots and boxes when
`line_left_is_bottom`).

`Node::text_offset_at_point` (hit testing) still takes logical coordinates.

## `@page` rules of linked sheets

`src/net.rs`, `src/document.rs` (`linked_page_css`): a `<link>` sheet with `@page` rules keeps its text, for
printing. The servo build of Stylo parses neither `size` nor `page`, so the parsed rule does not have them. pyro-render's
`page_box` reads them.

## `@font-face` `unicode-range` and rule order

`src/net.rs` (`FontFaceOverrides::unicode_range` and `precedence`), `src/document.rs` (`load_resource`),
`src/font_metrics.rs`: each `@font-face` face is registered with its `unicode-range` and a precedence rising in the
order the rules are read (rule order within a sheet, sheets in the order they load), through fontique's
`FontInfoOverride` (`vendor/fontique/README.pyro.md`). A face is then used only for the characters its
range includes, and the faces of one family that match a style equally (Google Fonts' per-subset faces) are checked
last rule first, however the fonts arrive. The metrics of `ex`, `cap` and `ch` take their fonts by the same rule.
Covered by `unicode_ranges_keep_their_code_points`, `font_face_unicode_ranges_and_rule_order_reach_the_query` and
pyro-render's `font_unicode_range` tests.

## Collapsed tables without uniform borders

`src/layout/table.rs` (`CollapsedEdges`), `src/node/stylo_data.rs`, `src/node/element.rs`: upstream has one gutter, from the
first cell's borders. A table whose collapsed borders are not one uniform grid now resolves them by the spec's conflict
rules: each cell keeps half of the borders around it (`LayoutData::collapsed_border`, applied to the cell's
`LayoutStyles::cell`) and the table half of its outer ones, as Chromium's boxes do. Uniform tables keep the gutter.
Values in `bench/pdf/fixtures/chrome-156/table-collapse.json`, tests in `crates/pyro-render/tests/table_collapsed_borders.rs`.

`src/layout/table.rs`: the table places its cells once, as it is built (`place_cells`, stored as
`TableContext::placement`), and hands the areas to the grid through taffy's `LayoutGridContainer::track_counts`
and `resolved_placement` (see `vendor/taffy/README.pyro.md`). It runs the grid's `row dense`
placement over each row's occupied column ranges (`OccupiedColumns`: sorted, adjacent ranges merged, so a
row of contiguous cells is one range and a wide row is not rescanned per cell), which is not always the column `ColumnCursor` gave the cell
(`TableCell::column`): a colspan cell that does not fit beside a rowspan hole moves past it and a later cell
backfills the hole. Upstream ran taffy's placement on every layout of the table. A cell whose `grid-row` is
not a positive line and a span (or whose `colspan` is 0) leaves the placement to the grid.
`table_cell_placement_is_the_grids` compares the grid info and every box's layout with the grid placing the cells.
An incremental table (see "Incremental tables": no cell spans rows) keeps the placement up to date in `update_rows`:
the areas of the rows collected again are spliced in from the cells' rows and cursor columns (`unspanned_cell_area`,
where dense placement puts a cell when nothing spans rows), and the rows after them are renumbered with the cells.
`updated_rows_keep_the_cells_placement` checks appends, inserts and removals against `place_cells`.

Time inputs use the existing text-editor construction and intrinsic measurement paths so captured values are painted.

- `assets/default.css`: the popover rules (`[popover]` is `position: fixed`, centered, with a border and
  padding, `:popover-open` is in the top layer) are not behind Gecko's `dom.element.popover.enabled` pref,
  which no embedder sets: a showing popover is laid out as Chrome lays it out.

## `:has()` invalidation for state and attribute changes

- `src/has_invalidation.rs`, `src/document.rs` (`snapshot_node_and`), `src/mutator.rs` (`set_attribute`,
  `clear_attribute`): a state change or attribute change runs Stylo's `RelativeSelectorInvalidator`, as
  Gecko's restyle manager does, with the dependencies the relative selector invalidation map has for what
  changed (state bits, attribute name, ids, classes, custom states). Only the anchors whose `:has()`
  argument depends on the change, that a `:has()` search reached and whose own compound matches restyle,
  as far as the rule needs. The element's selector flags gate it (no `:has()` search visited it: one flag
  test), then `relative_state_dependency` (the union of the relative map's states, a vendored Stylo
  change). A search the relative-selector bloom filter fast-rejects visits (and flags) nothing, so
  `add_element_unique_hashes` (`src/stylo.rs`) flags every element it adds to such a filter as searched
  in both directions; otherwise a change under an anchor whose earlier `:has()` matched early would never
  reach it. This replaces `invalidate_has_anchors` for those changes, which restyled every anchor that
  could see the element with its whole subtree; DOM insertions and removals still use it.
- `src/node/element.rs` (`sync_lp_attribute_state`), `src/stylo.rs`: the tokens of pyro-dom's
  `urn:x-pyro:state` `state` attribute are `ElementState` bits (`VALID`, `INVALID`,
  `PLACEHOLDER_SHOWN`, `INRANGE`, `USER_INVALID`, ...; `READONLY` unless `read-write`), set when the
  attribute is written, and the pseudo-classes match the bits. Stylo's snapshot invalidation and style
  sharing compare element states, so they follow these like `:checked`. A selected `<option>` (pyro-dom
  writes `selected` for the current selectedness) is `CHECKED`. `each_custom_state` lists the
  `custom-states` attribute instead of panicking.

Covered by `crates/pyro-layout/tests/has_state.rs` and `crates/pyro-browser/tests/has_state.rs`.

## Top layer

- `src/layout/paint_tree.rs` (`note_lp_state`, `top_layer_nodes`, `top_layer_position`), `src/mutator.rs`
  (`create_element`, `set_attribute`, `clear_attribute`, node drops), `src/document.rs` (`top_layer`): elements whose
  `urn:x-pyro:state` `state` attribute has a `top-layer=N` token (pyro-dom's render tree) are kept by `N`
  (`TopLayer`: a rank-ordered map plus each element's rank, so entering, leaving, re-ranking, membership tests
  and drops cost no scan however many popovers show), maintained when that attribute changes and when a node
  leaves the slab. A top-layer element always roots a stacking context in the paint tree
  (`is_paint_stacking_context_root`), so its z-indexed and fixed descendants paint with it. blitz-paint paints
  them above the document.

## Layout styles written with the style

`src/node/stylo_data.rs` (`LayoutStyles`), `src/stylo.rs`, `src/node/node.rs`, `src/layout/{table,construct,damage}.rs`:
what layout reads of a primary style in another form is derived when the primary style is written, not on every
layout pass. `StyloData` keeps, next to Stylo's data:

- `table_part`: the physical `taffy::Style` (`stylo_taffy::to_taffy_style`) of a table, row, column or cell.
  Table construction (`table_taffy_style`, transposed into the table's writing mode) and the column-width and row-height
  reads (`physical_size`) clone or read it instead of converting every cell of the table again.
- `cell`: a table cell's style as its own layout sees it (no definite height; collapsed borders replaced by its half
  of them), which `Node::layout_style` used to clone from the primary style on every call (each Taffy style read of a
  cell in a `border-collapse: collapse` table).

They are rewritten only when what they are derived from changes:

- The style traversal (`RecalcStyle::process_preorder`, and `sync_pseudo_element_styles` for pseudo-elements) derives
  them for a first style, and for a replaced primary style only when Stylo's restyle damage for it reaches layout
  (`LayoutStyles::affected_by`: the `RELAYOUT` bit). Every property they read gives that damage when it changes; a
  paint-only restyle (colour, background, `:hover`, a class that changes only paint) keeps them, and debug builds
  assert there that they still equal what the new style gives.
- `Node::update_layout_styles` wherever a primary style is set outside the traversal (anonymous blocks,
  pseudo-elements, refreshed anonymous styles, the latter again only on layout damage).
- `Node::set_collapsed_border` rewrites only `cell` when table construction changes a cell's collapsed borders
  (`table_part` does not depend on them).
- `StyloData::clear` drops them with the style.

Debug builds also assert at every read that they are what the current primary style gives
(`assert_layout_styles_are_current`). pyro-render's mirror tests `tables_lay_out_from_the_styles_written_at_restyle`
(text edits, rows, spans, collapsed borders, display changes, paint-only restyles of every table part) and
`vertical_tables_lay_out_from_the_styles_written_at_restyle` (the same in a `vertical-rl` table, through the transposed
table style) cover them. On a 1,200-row table, `to_taffy_style` was ~6% of a cell edit and the cell style clone ~15%
of a width resize.

## Incremental tables

A change inside a table updates what it touched instead of building the table again. What a table keeps is its
`TableContext` (`src/layout/table.rs`), updated from what changed; nothing in it is a memo of a computation keyed on
its inputs.

- `src/layout/damage.rs`, `src/node/node.rs`: `propagate_damage_flags` sets `NodeFlags::OWN_DAMAGE` on a node with
  damage of its own (a mutation or a restyle, not only its descendants'), cleared with the damage, and notes every
  damaged cell in its table (`LayoutData::table_cell_index`, the cell's index in `TableContext::cells`).
- `src/resolve.rs`, `src/layout/table.rs` (`update_table_context`): a table whose rows or cells changed collects
  again only the rows with damage of their own or a cell with it, and rows added to or removed from a row group
  between rows it keeps; the other rows and cells stay. A change inside a cell (its text, its children) reaches the
  table as relayout only (`propagate_damage_flags` stops construction damage at the cell's box, see "Construction
  damage stops at the box that owns it"): no row is collected again, the cell is noted as changed. Its layout children are listed again only when cells were
  added or removed, and `resolve_layout_children` visits only the cells that changed. A change it cannot place (a
  `rowspan`, an out-of-flow part, a new column, a change to the first row, which gives the columns their widths and
  the first cell the gutters, the table's own damage, ...) builds the table again.
- The collapsed borders (`CollapsedEdges`) are resolved again only on the grid lines of the rows collected again and
  the cells beside them (`offer_box` with an `EdgeRange`); a uniform table keeps its edges too (`spare_edges`) to
  tell when it stops being uniform.
- `TableTreeWrapper::auto_column_widths`: each cell's min-content and max-content widths are kept with the table's
  content width they were measured for (`CellWidths`), and so are the columns' widths summed from them. A cell that
  changed is measured again; a column's widths are summed again only when a cell that set them got narrower. Once
  more cells changed than it holds widths for, it drops them all (every cell is measured again), possibly halfway
  through `update_rows`, so each row replacement checks again whether the widths still follow the cells. Another
  table content width (a shrink-to-fit table whose container got wider or narrower) changes the intrinsic widths of
  the cells with a percentage or `calc()` `width`, `min-width`, `max-width`, padding or margin of their own alone
  (`TableCell::percent_lengths`): those are measured again, the others keep their widths. With the same columns and
  cells and only the available width changed, the grid's last layout stands (`table_reuse::stands` in Taffy;
  `store_table_grid_state` counts no layout for a state marked `stood`). Covered by
  `tables_keep_their_layout_when_only_the_available_width_changes` in `crates/pyro-render/tests/mirror.rs`.
- The grid is laid out again from its last layout when the columns are the same (see "Table grids laid out again
  from their last layout" in `vendor/taffy/README.pyro.md`): `take_table_grid_state` gives back the cells
  that changed (`TableContext::changed_cells`) and the rows replaced since (`grid_splice`: one replacement between
  two layouts; more lay the grid out whole). The indices the table keeps (changed cells, cells to measure, round,
  transform and visit in the paint tree, the cells' overflow, a plain table's paint children) move with the cells
  (`splice_indices`, `PlacedCells::splice`).
- `src/layout/writing_mode.rs`, `src/resolve.rs` (`table_cells_overflow`): rounding and `resolve_transforms` visit
  again only the cells the grid placed or that changed since (`TableContext::placed`), with each cell's overflow in
  the table kept; the others' are as they were. Horizontal tables without out-of-flow parts only.
- `src/layout/paint_tree.rs` (`table_paint_children_kept`): a table whose cells were all plain boxes in it (no
  stacking context, nothing hoisted out of them, one paint order) visits again only the cells damaged or appended
  since, and keeps its paint children while they stay plain.
- `src/node/node.rs` (`update_unrounded_layout`): a box that only moved marks only its ancestors as having changed
  (see "Incremental passes for layout queries", which also covers a moved box whose rounded size, and so its
  transform, changes). The rows below a cell that grew move without their cells' subtrees being transformed again.
- `src/layout/damage.rs` (`compute_layout_damage`): a layout change in the style of a table part or a table (from a
  style sheet as from an attribute) builds its box again, as a mutation does: the table keeps its parts' layout
  styles. Restyles used to relayout with the styles the table was built with.
- `src/layout/construct.rs`, `src/layout/table.rs` (`damage_former_table_parts`, `drop_part_box`): a table built as
  another box (`display: block`) builds its row groups and rows as boxes, which they never were in the table, and a
  table built in place of another box drops the boxes its parts had. The parts are those of the table's last
  `TableContext` (its rows, and their row groups and `display: contents` boxes up to the table), whatever their
  `display` is now: a row group of a table made a flex container is blockified into a block item, and its rows had
  lost their boxes all the same. The context is dropped with them (the box is no table; pyro-render reads a table's
  collapsed borders from it).

### Incremental layout that differed from fresh builds

- `src/node/node.rs` (`Node::set_collapsed_border`): a change of a cell's half collapsed borders clears its layout
  cache, as its layout style changed. `build_table_context` (a table built again: its first row changed, a rowspan,
  an out-of-flow part) gave the cells beside a change their new borders without laying them out again: removing or
  moving the first row of a table with row borders, or giving a cell beside a rowspan a border, left them 0.5-1.25px
  off. Only `update_rows` invalidated them.
- `src/layout/table.rs` (`release_cell`, `release_former_cells`): a node keeps half collapsed borders only while it is
  a cell of a collapsed table. A cell taken out of flow, one no longer collected (made a block, its row removed or
  `display: contents`), and the cells of a table built as another box lose them, and their `table_cell_index`: such a
  box laid out with the table's borders, and paint, which draws a node's own borders only without them, dropped its
  own. `build_table_context` compares the cells of the context it replaces, `update_rows` the cells it replaced; one
  another container collected since (`Node::layout_child_of`) is left to it.
- `src/layout/table.rs` (`enter_layout_tree`), `src/resolve.rs`: a cell collected while no container lists it is new,
  or back in the layout tree from outside it (its row was `display: contents`, which Blitz's tables treat as
  `display: none`, or it was a block the table ignored). Stylo styled it meanwhile, and `clear_damage_and_dirty_flags`
  cleared the damage no pass consumed, so its subtree is damaged and its layout caches cleared: a cell edited while
  its row was `display: contents` kept the text it had. For this `resolve_layout_children` takes the previous layout
  children out of the list (`layout_child_of`) after collecting the new ones, not before.
- `src/mutator.rs` (`add_children_to_parent`, `is_under_display_none`): an element moved into a `display: none`
  element or below one (the nearest styled element from the new parent up is `display: none`) drops its style data,
  inline layout and construction flags, as Stylo keeps none there (it clears descendants when an element becomes
  `display: none` and never visits the children of one). The moved element kept its styles, and pyro-render reported
  the box its old inline layout gave it. Its pseudo-elements and layout tree (`leave_layout_tree`) go too. Every
  insertion looks up the ancestors, not only one of a styled element: pyro-dom builds a new wrapper by moving styled
  children into the new, unstyled element before inserting it, and the paragraph so moved under a hidden element was
  never laid out once shown.
- `src/document.rs` (`leave_layout_tree`, `left_layout_lists`), `src/resolve.rs`: a box that left its container's list
  in a pass (`resolve_layout_children`, `update_rows`, `drop_part_box`) and that no container collected again by the
  pass's end is out of the layout tree, with what its subtree built: its layout children leave its list in turn, and
  its anonymous blocks are freed. A fresh build has none there; the stale lists kept boxes under a `display: none`
  element (and in a cell made a block) in pyro-render's layout tree, and tripped `assert_layout_parents_consistent`
  (a float moved out of an element hidden in the same pass). A box that comes back is built again: styled from
  scratch, added (`process_added_subtree`), or damaged on its way in (`enter_layout_tree`, `damage_former_table_parts`).
  A table with no layout children (it left) is built again, not updated (`is_incremental`).
- `src/layout/table.rs` (`damage_former_table_parts`): the children a table ignored (captions, columns) have their
  subtrees built when the table becomes another box: no pass had built them (`collect_table_cells` drops their
  damage), and a caption was 0px tall.
- `src/layout/table.rs` (`TableContext::group_borders`, `group_border_kept`): a collapsed table whose row group is
  restyled with another border is built again: `update_rows` collects only the group's rows, and the collapsed
  borders kept the group's old border.
- `src/layout/table.rs` (`apply_collapsed_borders`): a cell outside the grid's columns has no collapsed borders, not
  those it had where it was before (a cell pushed past the last column by a rowspan laid out 1px wider).
- `src/layout/list.rs` (`marker_changed`): a list item whose marker changed (its number, after an item before it was
  moved) is built and laid out again: an `inside` marker is text of its inline layout, which kept the old number.

`crates/pyro-render/tests/mirror.rs` compares the incremental document with fresh builds after cell edits, row appends,
inserts and removals, spans, collapsed borders, nested and vertical tables, stacking contexts in cells and `display`
changes (`table_*_match_fresh_builds`), and counts what an edit, an append, a removal and an insert collect and lay
out (`a_cell_edit_collects_only_its_row`). `blitz_tables` and `blitz_table_grids` (pyro-render) report tables built, cells
collected and grids laid out whole or again.

## Construction damage stops at the box that owns it

`src/layout/damage.rs` (`propagate_damage_flags`, `forwards_construction`), `src/mutator.rs`: the mutator marks a
parent whose child list or text changed with `CHILDREN_CHANGED` (`CONSTRUCT_FC`: collect its layout children again),
not `ALL_DAMAGE`; `CONSTRUCT_BOX` means the node's own box changed. A reconstructing node forwards `RELAYOUT |
CONSTRUCT_DESCENDENT` to its parent, plus `CONSTRUCT_FC` only when its own box changed or when its children are laid
out by an ancestor: `display: inline` boxes (to the inline root, replaced ones included, though `is_or_contains_block`
does not look through an atomic inline box: an `<object>` is one only while it shows an image), `display: contents`, table rows, row groups, columns and column groups (to the table) and list
items (to the list). A paragraph appended deep in a page collects its container's children again, not every
ancestor's. `phase_profile::constructions` counts the collections (and, with it, the tables built from scratch).
Covered by `crates/pyro-render/tests/incremental_construction.rs` (each change compared with a fresh build, boxes and
pixels).

## Out-of-flow boxes of the root containing block relaid without their ancestors

`src/layout/damage.rs` (`POS_CHILD_RELAYOUT`, `is_relayout_boundary`, `is_containing_block_for_some`): a dirty
absolutely or fixed positioned box (a box of its own, below the root element, whose own damage has no relayout or
box change and which forwards no construction) returns `POS_CHILD_RELAYOUT` to its parent in place of
`ONLY_RELAYOUT`, keeping `RECALCULATE_OVERFLOW`. Ancestors pass it on without clearing their layout caches; one
that can be a containing block (positioned, transformed, filtered, contained, `will-change`, a size container, or an
inline root claiming for a positioned span, `TextLayout::span_cb_flags`, also through the anonymous boxes it owns)
turns it back into `RELAYOUT`, since its out-of-flow pass runs inside its cached layout. So only the initial
containing block is a boundary: the root's in-flow layout hits its cache and the root positioning pass relays the
dirty box. Taffy's side: a cached root that claims no out-of-flow box records the pass's hoisted boxes again (see
`vendor/taffy/README.pyro.md`). `phase_profile::performed_layouts` counts layout algorithm runs that place
children. Covered by `crates/pyro-render/tests/out_of_flow_boundary.rs`.

## The paint tree is built when something paints or hit-tests

`src/resolve.rs`, `src/layout/paint_tree.rs` (`ensure_paint_tree`), `src/layout/damage.rs`
(`clear_damage_and_dirty_flags`), `src/document.rs`: a resolve no longer builds `paint_children` and the stacking
contexts; it flags the nodes whose propagated damage reached `REBUILD_STACKING_CONTEXT` with
`NodeFlags::PAINT_TREE_DIRTY` (and `PAINT_TREE_OWN_DAMAGE` when the damage was the node's own, which
`table_paint_children_kept` reads in place of `OWN_DAMAGE`, cleared by every resolve) before dropping the damage,
and sets `BaseDocument::paint_tree_dirty`. The first paint (blitz-paint's entry points) or hit test
(`hit_with_scrollbar`, so `hit`, `element_from_point`, `find_text_position`, hover, the pointer handlers, which take
`&mut self`) builds the tree once for all the resolves since the last build, in sub-documents too. A run of forced
layouts (`getBoundingClientRect` and the like) builds it once, not once per resolve.
`paint_tree_matches_fresh_build` (debug-asserted by `ensure_paint_tree`) compares the incremental tree with a fresh
build of every node, tables visited as any box. Covered by pyro-render's `settle_resolution_tests`
(`resolve_leaves_the_paint_tree_to_the_first_hit_test`, `element_from_point_sees_every_resolve_since_the_last_paint`).

## Bounds for hit testing by tree descent

pyro-render answers `elementFromPoint` by descending the tree from the root and skipping the subtrees whose bound
cannot hold the point (`crates/pyro-render/src/hit.rs`), instead of reading every element's box. The bound of a box
is its border box and its hit overflow, brought up to date by the first hit test after a resolve; and two things
are exposed:

- `src/resolve.rs` (`ensure_hit_overflow`, `resolve_hit_overflow`, `unite_hit`, `glyph_reach`),
  `src/node/element.rs` (`LayoutData::hit_overflow`, `LayoutData::hit_stale`), `src/document.rs`
  (`hit_overflow_dirty`), `src/node/text.rs` (`TextLayout::hit_reach`), `src/layout/inline/layout.rs`: the
  transform pass (`resolve_transforms`) computes only the scrollable overflow, and flags each box it computes again
  `hit_stale`; a resolve marks the document's hit bounds stale (`hit_overflow_dirty`), as it does its paint tree.
  `ensure_hit_overflow` (called by pyro-render before a hit test) walks the tree the transform pass walks, through the
  flagged boxes only (a box is flagged only when that pass computed it again, and so were its ancestors), and keeps
  each box's hit overflow (`Node::hit_overflow`, device px, like `scrollable_overflow`). A run of forced layouts with
  no hit test in between (a cell edited and measured in a loop) pays nothing for hit bounds. It is the scrollable
  overflow except for two things. First, the contributions of no area that have a width or a height count
  (`unite_hit`; `unite_overflow` drops them for the scroll size: a 2000 x 0 px box does not make a page scroll, but
  text overflowing sideways on lines of `line-height: 0`, or a zero-width box, is hit). Second, an inline formatting
  context's glyph runs reach outside its line boxes (a tight `line-height` puts the whole-pixel ascent and descent
  past the line box; hanging white space past the line's advance), and layout queries report an inline element's
  box from those content areas (pyro-render's `Geometry::inline_items`): the hit overflow holds the line boxes grown
  by the largest such reach of the context (`glyph_reach`, plus a layout unit of advance rounding). The reach is
  measured per line (past the line's start, end and block extent), which aligning does not change, by the first hit
  pass after the lines were broken (`TextLayout::hit_reach`: breaking resets it, a rebuilt `TextLayout` starts
  without it, and so does aligning when it justifies now or did before, `TextLayout::justified`). Overflow clips
  apply to the hit overflow as to the scrollable one (`overflow_contribution`; a box placed by a translation alone
  adds its location instead of going through `transform_rect_bbox`, which gives the same values: `placement`).
- `src/resolve.rs` (`table_cells_overflow`, `table_cells_hit`), `src/layout/table.rs` (`PlacedCells::hit`,
  `PlacedCells::hit_again`, `PlacedCells::row_overflow`, `TableContext::row_hit_extents`): rows and row groups have
  no box, and a cell can span later rows, so a row's bound is the union of the hit contributions of the cells
  originating in it (kept per cell in `PlacedCells::hit`), kept per row. The transform pass notes the cells it
  transforms again (`PlacedCells::transformed`, kept with `splice` like the other lists); the hit pass computes
  those cells' contributions and their rows' unions again (all rows when the rows changed: `splice` clears them).
  A table whose cells the pass does not keep (`keeps_cells`) reports none and pyro-render computes the contributions
  from the cells.
- `src/document.rs` (`sticky_node_ids`): the sticky set behind `has_sticky`, so pyro-dom learns the sticky
  elements without a pass over the document; `BaseDocument::overflow_contribution` is `pub` for the table
  fallback above.

Covered by `crates/pyro-layout/tests/hit_descent.rs` and `hit_adversarial.rs` (the descent against the scan of every
box, also on random pages) and `crates/pyro-render/tests/mirror.rs` (`deltas_on_long_tables_and_lists_match_fresh_builds`, which compares the descent's
candidates of a delta-synced document with a fresh build's).

## Line breaking resumes after the lines whose text did not change

`src/layout/construct.rs`, `src/resolve.rs`, `src/layout/inline/build.rs`, `src/layout/inline/layout.rs`: an inline
root built again keeps its parley layout (the rest of its `TextLayout` is reset), builds with
`build_into_keeping_lines` (parley keeps the lines its last break decided over the shaped content that is the same,
see `vendor/parley/README.pyro.md`) and, with no floats involved and the text not justified, resumes the
break after them (`BreakLines::resume` with main's `max_advance` for the lines after the first; the float
bookkeeping of #1378/#1390 starts at the kept lines). `phase_profile::line_breaks` counts breaks, lines kept and
the text bytes the breaker went over. Covered by `crates/pyro-render/tests/line_break_resume.rs`.
`TextLayout::content_widths`, a memo nothing read (intrinsic sizing and list markers call
`calculate_content_widths`), is gone with `Node::invalidate_layout_cache`, which only cleared it besides the layout
cache (`clear_layout_cache`).

## Content widths kept with the shaped text

`src/node/text.rs`, `src/layout/inline/build.rs`, `src/layout/inline/layout.rs`: `calculate_content_widths` walks
every cluster, and inline layout called it once per measurement (min-content, max-content) and once per layout (for
the slack it gives the line breaker when content is sized to fit): three walks per cell and width on a table
resize, 9-10% of the pass. The min-content and max-content widths of a layout without in-flow inline boxes depend on
the shaped text and the text indent alone, so `build_inline_layout_into` measures them once, with no indent, as part
of the build (`TextLayout::text_content_widths`, part of the shaped text and replaced with it), and
`TextLayout::content_widths(indent)` answers from them with no indent set, else measures. Layouts with in-flow
inline boxes, whose sizes each measurement sets, keep measuring. Debug builds check the kept widths against a
measurement at every read. Covered by `content_widths_kept_with_the_shaped_text_match_fresh_builds` in
`crates/pyro-render/tests/mirror.rs`.

## Paint-only restyles run no layout and shape no text

`src/layout/damage.rs`, `src/stylo.rs`, `src/layout/construct.rs`, `src/layout/table.rs`, `src/node/node.rs`: a restyle
that changes only what paints (colour, background, outline, `visibility`, opacity, text decorations, box shadows, a
border's colour, `:hover`, a class on `body` that recolours a table) used to run a Taffy pass, collect the cell's row
again and shape its text again. Three causes, each fixed where it is:

- Stylo's property table gave every longhand without an explicit `servo_restyle_damage` the `rebuild_box` (relayout)
  damage: `border-*-color`, `visibility`, `box-shadow`, the border radii, `mask-*`, `appearance`, `scrollbar-color`,
  ... The vendored stylo derives the default from each property's `affects` (see
  `vendor/stylo/README.pyro.md`), so a paint property's change is `REPAINT` and never reaches
  `compute_layout_damage`. `compute_layout_damage` no longer rebuilds the box for `visibility` (nothing in layout
  reads it: hit testing and paint read the style).
- A collapsed table resolves the colours of its borders into `CollapsedEdges` when it collects a row, so
  `RecalcStyle::process_preorder` gave a table part whose border struct or `color` changed full damage: `color` on a
  cell with `border: 1px solid #ddd` collected its row and shaped its text again. It now compares the four borders'
  resolved colours (`resolved_border_colors`: `currentColor` resolved, invisible borders ignored) and gives
  `BOX_DAMAGE` only when one changed.
- `RestyleDamage` has a new bit, `INLINE_CONTENT`: the node's text, the styles it is shaped with, its children or a
  box among them changed. It is part of `ALL_DAMAGE` and `CHILDREN_CHANGED` (so mutations, new nodes, font and
  text-style changes carry it) and propagates to every ancestor. `BOX_DAMAGE` is `ALL_DAMAGE` without it: a box
  built again with its inline content as it was, which `compute_layout_damage` gives a table part (or table) whose
  layout style changed (the table keeps the part's layout style in its `TableContext`; it used to give `ALL_DAMAGE`
  from `box_tree_needs_rebuild`, which shaped the cell's text again) and `process_preorder` a collapsed border's
  colour change. An inline root collected again whose damage, read after
  `flush_pseudo_elements`, has no `INLINE_CONTENT` keeps its `TextLayout` instead of queueing a rebuild; debug
  builds shape it again and assert the text, text nodes and inline boxes are the same, and that the
  content widths kept with the shaped text (`TextLayout::text_content_widths`) are what the new build measures
  (`assert_kept_inline_layout_is_current`). `white-space: nowrap` (`text-wrap-mode`) and a span's `vertical-align`
  (the `baseline-*` longhands) are shaped into the parley layout and gave `RELAYOUT` only, so the layout kept the old
  value: `text_shaping_needs_recollect` rebuilds for them too.
- `NodeFlags::OWN_LAYOUT_DAMAGE`: `OWN_DAMAGE` whose damage reaches layout (`damage::reaches_layout`: relayout,
  construction, reordering), set and cleared with it. Tables read it in place of `OWN_DAMAGE` (`row_changed`,
  `group_changes`, `update_table_context`, `update_rows`): a cell or a table whose colour, background or opacity
  changed is no changed row and no reason to build the table again. `note_changed_table_cell` notes such a cell for
  the transform pass and the paint tree (`PlacedCells::damaged`) but not as a cell to measure and lay out again. Whether
  the damage reaches layout is read before `ONLY_RELAYOUT` is dropped from the stored damage: a cell relaid out
  for a change inside it (a block's width, an inline-block's width) is a changed cell.

On a 1,200-row collapsed table, a `color` toggle on a cell no longer runs layout (0.45 ms to the restyle's cost),
`.dark td { color }` on `body` no longer shapes 2,400 inline layouts, and a `padding` change on every cell shapes
none. `crates/pyro-render/tests/mirror.rs`: `paint_only_restyles_run_no_layout_and_match_fresh_builds` (every paint
property on cells, inline roots, flex and grid items, floats and absolutely positioned boxes, synced as deltas:
boxes, document size and paint against a fresh build, no layout algorithm run, no text shaped, the paint changed)
and `text_style_changes_shape_again_and_match_fresh_builds`.

## Small stack frames per level of the layout recursion

`layout/mod.rs`, `layout/writing_mode.rs`: Taffy's algorithms lay out a child through
`compute_child_layout` -> `compute_child_layout_internal` -> `compute_child_layout_in_own_wm` ->
`compute_child_layout_in_current_wm` -> `dispatch_child_layout` -> the child's algorithm, so every level of a
nested tree keeps these frames on the stack. `dispatch_child_layout` had the leaf boxes (form controls,
selects, replaced elements) and the table code inlined, about 2 KB of frame per level. It now only picks the
algorithm: `leaf_layout_or_algorithm` (not inlined) lays out a leaf box or names the algorithm
(`ChildLayout`), and tables (`compute_table_root_layout`), inline, flex and grid layout are not inlined into
it; `compute_child_layout_in_current_wm` keeps its input adjustments (`adjusted_child_inputs`) and the
out-of-flow pass (`lay_out_out_of_flow_boxes`) out of its frame, and `compute_child_layout_in_own_wm` its
overflow mirroring (`mirror_child_overflow`) and orthogonal flows. With Taffy's part (its README), a nested
block costs about 2.7 KB of stack in a release build instead of 4.9 KB (2.4 KB instead of 5.3 KB in a debug
build): a 2 MB thread lays out about 770 levels instead of 430. Nothing else changes; the layout results are
the same (pyro-layout and pyro-render tests). Covered by pyro-layout `text_clip_facts_deep_nesting_without_recursion`
(400 levels on a test thread's 2 MB) and `deep_nesting_lays_out_on_a_browser_thread_stack` (2,000 on 8 MB).

## Deep trees grow the stack on demand

`util.rs` (`ensure_stack`, `STACK_RED_ZONE`, `STACK_GROW_BY`), `Cargo.toml` (the `stacker` crate, which rustc
uses for the same purpose): the small frames above only move the depth at which a thread's stack overflows,
and an overflow aborts the whole process (every page of a `serve`). Every walk that recurses once per tree
level now makes its recursive call through `ensure_stack`, which moves to a new 4 MB stack segment (mapped, committed
as touched, unmapped on return) when less than 256 KB is left, so a tree of any depth lays out, paints and
prints whatever stack its caller left (a 2 MB spawned thread, or JS recursion near JavaScriptCore's limit
followed by a layout read). With room left it costs a thread-local read and one stack-pointer compare per
level. The red zone covers what one level runs between two checkpoints: a level of the layout dispatch
(about 3 KB) plus a leaf's own non-recursive work (shaping, table and grid sizing).

- Layout: `LayoutPassState::compute_child_layout_internal` (`layout/mod.rs`), which every algorithm's child
  layout goes through (Taffy's `compute_child_layout`, Blitz's block contexts, tables, orthogonal flows).
- Box construction and damage: `resolve_layout_children_recursive`, `classify_flow_children`,
  `flush_inline_pseudos_recursive`, `find_inline_layout_embedded_boxes_recursive`,
  `collect_list_item_children`, `build_inline_layout_recursive`, `propagate_damage_flags`,
  `refresh_anonymous_block_styles`, `clear_damage_and_dirty_flags`.
- After layout: `resolve_transforms` (overflow, including a kept-cells table's cells in
  `table_cells_overflow`), `resolve_hit_overflow` (the hit bounds brought up to date at the first hit test,
  including `table_cells_hit`), `physicalise_and_round_inner` (rounding) and its debug check
  `assert_rounding_is_current`, `build_paint_tree_impl` (paint tree), `hit_inner` (hit testing). The walks
  that skip children with no damage below them (`propagate_damage_flags`, `clear_damage_and_dirty_flags`,
  `resolve_layout_children_recursive`) make the calls they still make through `ensure_stack`.
- `layout/writing_mode.rs` (`placer_geometry`): the position of a box in a vertical tree recursed up its
  containing-block chain; it is a loop over the chain, outermost first, with the same arithmetic.
- Tree upkeep: `iter_subtree_mut_inner`, `collect_all_inline_roots_in_subtree`, `is_or_contains_block`,
  `write_text_content`, `drop_node_ignoring_parent_with`, `deallocate_anonymous_block`, `leave_layout_tree`,
  `remove_pe_ignoring_parent`.

blitz-paint's `render_node` and pyro-render's hit-test, PDF structure-tree walks use the same function. The HTML
parser is not capped (Chrome caps parser depth at 512; a cap would change DOM results). Covered by pyro-layout
`deep_nesting_of_each_algorithm_lays_out_on_any_stack` (5,000 levels of ten kinds on a 256 KB thread) and
pyro-render `deep_trees_lay_out_paint_and_print_on_a_small_stack`.

## A line-break override per document

`src/config.rs` (`DocumentConfig::line_break_override`), `src/document.rs`, `src/iframe.rs`, `src/resolve.rs`: a
document can be given a `'static` parley line-break override (pyro-render passes
`parley::HYPHEN_DIGIT_LINE_BREAK_OVERRIDE` for `serve --hyphen-digit-breaks on`). It is set on the document's
parley `LayoutContext` (and on the per-thread contexts of `parallel-construct`), so every text layout of the
document, its line breaking and its min/max-content widths, uses the same opportunities, and an iframe's document
inherits it. `BaseDocument::line_break_override` reads it back (pyro-render rebuilds a persistent document whose
setting differs). Covered by `crates/pyro-render/tests/hyphen_digit_breaks.rs`.

## The `border-image-source` image is loaded; `background-clip: border-area` parses

`src/util.rs` (`ImageLayerKind::BorderImage`, `ImageType::BorderImage`), `src/node/element.rs`
(`ElementData::border_images`), `src/layout/damage.rs` (`flush_image_layers_from_style`), `src/stylo.rs`
(`needs_style_image_flush`), `src/document.rs`: a `url()` in `border-image-source` is fetched and stored as the
background and mask images are (a list of one), for blitz-paint's `border-image`. `src/document.rs` also turns
on Stylo's `layout.css.background-clip.border-area.enabled`: `background-clip: border-area` was dropped as
invalid, and blitz-paint clips it to the border's area. Covered by `crates/pyro-render/tests/borders_backgrounds.rs`.

## `:lang()`

`src/lang.rs` (new), `src/stylo.rs` (`match_non_ts_pseudo_class`, `lang_attr`, `match_element_lang`): upstream
never matched `:lang()`. An element's language is the nearest inclusive ancestor's `xml:lang`, or `lang` on an
HTML, SVG or MathML element, else the document's pragma-set default language (the `content` of the last
`<meta http-equiv="content-language">` of its `<head>`, unless it lists several); a range matches by RFC 4647
extended filtering, ASCII case-insensitively, as in Chromium (WPT css/CSS2/selectors/lang-*, syntax/
case-sensitive-004). `lang_attr` gives Stylo the element's own attribute for invalidation; `:lang()` is a
revalidation selector, so style sharing stays correct. Covered by `crates/pyro-render/tests/lang_pseudo_class.rs`.

## Document facts: XML documents, HTTP `Content-Language`

`src/config.rs` (`DocumentConfig::xml_document`, `content_language`), `src/tree.rs` (`NodeTree`), `src/document.rs`
(`BaseDocument::new`), `src/iframe.rs`: what the embedder knows of the document beyond its nodes, kept on the node
tree for the selector matching hooks to read.
- `src/stylo.rs` (`is_html_document`, `is_html_element_in_html_document`): in an XML document (XHTML) type and
  attribute selectors match names case-sensitively, so `DIV` does not match `div` (WPT css/CSS2/syntax/
  case-sensitive-007). Upstream treats every document as HTML; HTML documents keep that answer for every element.
- `src/lang.rs` (`default_language`): the response's `Content-Language` (one language tag) is the last fallback of
  `:lang()`, after the elements' attributes and the `<meta http-equiv>` pragma (HTML, "the pragma-set default
  language ... else language information from a higher-level protocol"; WPT css/CSS2/selectors/lang-selector-005).

Covered by `crates/pyro-render/tests/document_facts.rs`.

## `<style>` text is the sheet as it is

`src/document.rs` (`process_style_element`): upstream decoded HTML character references in a `<style>` element's
text before parsing it as CSS. The parser already decoded those the markup has (XML text, an SVG `<style>` in HTML);
an HTML `<style>` is raw text and an XML CDATA section is literal, so a `&#x46;` left there is four characters in
every browser (WPT css/CSS2/generated-content/content-177). Covered by `crates/pyro-render/tests/style_element_text.rs`.

## `filter()` images are loaded

`src/util.rs` (`unfiltered_image`), `src/layout/damage.rs` (`flush_image_layers_from_style`), `src/stylo.rs`
(`needs_style_image_flush`): a background, mask or border image layer that is a `filter()` image (vendored stylo)
loads the image it filters, so blitz-paint can draw it with the filters (`vendor/blitz-paint/README.pyro.md`).

## Tagged images in sRGB

`src/icc.rs` (new), `src/net.rs` (`ImageHandler::parse`), `Cargo.toml` (`moxcms` 0.8, the image crate's own colour
management library): a raster image's embedded ICC profile converts its pixels to sRGB when it is decoded (CSS
Color 4, "Color spaces of tagged images"), as Chromium draws them for an sRGB screen (WPT css/css-color/
tagged-images-*). `parse` decodes as `ImageReader::decode` does, keeping the decoder for `icc_profile()`. Each
profile's transform is made once (32 kept); a profile that maps a probe of primary and gray ramps to itself
within a level (an sRGB profile, which Skia leaves alone too) converts nothing, so sRGB-tagged images keep their
exact pixels, and so do untagged ones. Profiles that are not RGB (CMYK, gray) are left out. Covered by
`crates/pyro-render/tests/tagged_images.rs`.

## Fonts for SVG text given at startup

`src/util.rs` (`add_svg_font`, `FONT_DB`): fonts `add_svg_font` gave are loaded into usvg's font database, beside
the system fonts, when it is made, so SVG text finds them by family name as if they were installed (pyro's
`serve --font`, which the WPT reftest runner uses for Ahem). Call it before the first SVG is parsed. Covered by
`crates/pyro-render/tests/startup_fonts.rs`.

## Rotations by multiples of 90 degrees are exact

`src/stylo_to_kurbo.rs` (`resolve_2d_transform`): the transform's coefficients that lie within 1e-9 of a whole number
are made whole before the transform origin is applied, as Chromium computes a rotation by a multiple of 90 degrees
exactly (`SinCosDegrees`). Stylo's `rotate(360deg)` came out with `b` and `c` of 2.4e-16, so the box was transformed:
its text was not snapped to quarter pixels as untransformed text is, and was a few pixels off its reference, which
the fuzzy limits of WPT css-transforms/transform-input-* hide while text is antialiased but not once it is aliased
(the reftest runner's text). Now such a turn is no transform at all. Covered by
`text_turned_by_a_whole_turn_draws_as_untransformed_text` in `crates/pyro-render/tests/text_aliasing.rs`.

## The `rotate` property at the transform list's precision

`src/stylo_to_kurbo.rs` (`resolve_transform`): a matrix with a `rotate` in it is rounded to f32, as Stylo rounds the
transform list's matrix (`to_transform_3d_matrix` casts its f64 matrix to f32), so `rotate: 45deg` is the matrix
`transform: rotate(45deg)` is. The property's sine and cosine were f64, the function's f32: 1e-8 apart, which moved
antialiased edges of a rotated box a level (WPT css-transforms individual-transform-2a/2b/2c, 27 pixels each).
`translate` and `scale` alone resolve as before (whole products of f32 values). Covered by
`the_rotate_property_draws_as_the_rotate_function` in `crates/pyro-render/tests/compositing_rounding.rs`.

## SVG reference cycles usvg does not break

`src/svg_refs.rs` (new), `src/util.rs` (`parse_svg_image`): usvg sets a clip path, mask or filter reference that
closes a cycle of one or two elements to `none` (`fix_recursive_links`), but follows a longer cycle without end and
overflows the stack, which ended the browser (WPT css/css-masking/clip-path-svg-content/clip-path-recursion-001.svg:
`clip0`, `clip`, `clip2`). Before usvg parses a source that has `url(` and one of those properties, it is read with
roxmltree (usvg's own), and every reference that closes a cycle (a back edge of a depth-first search over the
elements of one kind, through the references their content makes, as attributes or `style` declarations) is set to
`none`, as usvg treats a short cycle. Sources without a cycle are passed on as they are. Covered by
`crates/pyro-render/tests/svg_reference_cycles.rs`.

Patterns have a cycle usvg does not break either (found by the WPT crashtests, `bench/wpt/README.md`): usvg sets a
`fill` or `stroke` inside a pattern that refers back to the pattern, directly or through one other element, to
`none` (`fix_recursive_patterns`), but not one that comes back through a pattern's `href`, whose children a pattern
without children draws (WPT svg/crashtests/chrome-bug-565674533.html: `p2`'s line is stroked with `p1`, and `p1`
takes its content from `p2`; a stack overflow that ended the browser). `pattern_references` gives the same search
the paints of a pattern's content and its `href` (`xlink:href`): a paint that closes a cycle loses its `url()` and
keeps its fallback colour (`none` without one), an `href` that closes one is set to `none`, which names no element.
Unit-tested in `src/svg_refs.rs` (`a_pattern_cycle_through_href_loses_the_reference_that_closes_it`).

Two more rewrites there make usvg read a filter as Chromium does (both unit-tested in `src/svg_refs.rs`, and in
`crates/pyro-render/tests/svg_filter_primitives.rs`):

- A `filter` that is one `url(#id)` naming no element is set to `none`: Chromium draws the element unfiltered (the
  filter chain is ignored), where usvg leaves the element out (WPT css/filter-effects/filter-invalid).
- `default_zero_divisors`: an `feConvolveMatrix` whose `divisor` is zero gets the default divisor, the sum of its
  kernel's values (1 when that is zero), as in Chromium; usvg stands a transparent flood in for such a primitive
  (WPT feconvolve-divisor).

## SVG elements referenced from a box's CSS

`src/node/svg.rs` (`BaseDocument::svg_reference_document`): blitz-paint applies `clip-path: url(#c)` and
`mask-image: url(#m)` naming a `<clipPath>` or `<mask>` of an inline `<svg>` to a box. This gives it an SVG document
for usvg: the outermost `<svg>` around the element, serialized as inline SVG is (`svg_source`, sprites resolved), in
`<defs>` (none of it drawn, all of it referenceable), and content that references the element in a viewport of the
given size (percentages in the element's content resolve against it), parsed by `util::parse_svg_image` (its fonts,
its reference-cycle breaking). The content is in a group of id `__pyro_box_reference`, by which anyrender_svg draws
a box's masks of opaque shapes as the shapes (see its README). See blitz-paint's README.

## SVG images by URL: `<view>` fragments, images without a size

`src/node/svg.rs` (`SvgImageData::for_url`), `src/net.rs` (`ImageHandler::parse` gets the URL): an SVG image loaded
from a URL whose fragment names a `<view>` element (`sprite.svg#icon`) is parsed again with the view's `viewBox`
and `preserveAspectRatio` on its root, so it shows the view and has its aspect ratio (SVG 2 §5.8.3; WPT
`background-size-contain-svg-view`, `-cover-svg-view`); the fragment was ignored. An image with no `viewBox`,
`width` or `height` keeps its source in `inline_source`, as an inline `<svg>` without a `viewBox` does, so that
blitz-paint lays its content out in the size it is drawn at (`SvgImageData::sized`: a background's size, an
`<img>`'s content box); its percentages resolved against usvg's 100x100 and the tree was stretched. `with_root_attrs`
generalizes `with_root_size`. Tests: `an_svg_view_fragment_shows_the_view` and
`an_svg_background_without_a_size_is_laid_out_at_the_background_size` in pyro-render's
`background_images_and_edges.rs`.

## An inline `<svg>` clips at its viewport

`assets/default.css`: Chromium's `svg:not(:root) { overflow: hidden }` (written `svg|svg:not(:root)`, with an
`svg` namespace prefix: the sheet's default namespace is XHTML's), so an inline `<svg>`'s content stops at its box;
blitz-paint's `draw_svg` makes the cut, at the content box. `src/stylo.rs` (`build_presentational_hints`): an SVG
element's `overflow` attribute is the property's presentation attribute (`overflow="visible"` overrides the UA
rule); `parse_svg_presentation_attr` returns every longhand a shorthand sets. In layout that `overflow` counts as
on any box, as in Chromium (measured in Chrome 157): a replaced flex item whose `overflow` is not `visible` has an
automatic minimum size of 0, so SVG icons shrink in a row too narrow for them, and a block-level replaced element
stays clear of floats with any `overflow`. WPT CSS2 `*-replaced-*` pages with `svg:svg`; covered by
`crates/pyro-render/tests/svg_viewport_clip.rs`.

## `<object>` images

`src/mutator.rs` (`load_image`, `set_attribute_impl`, `clear_attribute`, element creation), `src/layout/replaced.rs`
(`is_image_object`, `object_loads_image`, `is_inline_box`), `src/layout/construct.rs`
(`collect_layout_children_with_wrap`, `find_inline_layout_embedded_boxes_recursive`), `src/layout/inline/build.rs`,
`src/layout/first_letter.rs`, `src/node/node.rs` (`is_non_atomic_inline`): an `<object>` whose `data` is an image
(its `type` is `image/*`, or without a `type` the URL's extension names an image format) loads it as `<img>` loads
its `src`. Once it has loaded, the object is replaced content: laid out by `compute_replaced_layout` from the image's
natural size and ratio, an atomic inline box when inline-level, painted by blitz-paint's `draw_image` or `draw_svg`,
and its children (the fallback) build no boxes: `Node::is_or_contains_block` does not look through an atomic
inline box (`is_inline_box`), so a block in the fallback does not split the object. Until then, when the load fails, and for other objects (`text/html`:
nested documents are not done), the children render as before. Removing `data` or changing `type` away from an image
shows the fallback again. An SVG with no natural width, height or ratio keeps the fallback too
(`object_shows_image`, `src/document.rs` `apply_loaded_image`): Chromium draws one at the default object size, which
fails WPT `replaced-intrinsic-002`, a pass so far only because its fallback text lies under the reference's green
square (every browser fails it). WPT CSS2 `replaced-intrinsic-003`..`005`, `margin-border-padding-003`; covered by
`crates/pyro-render/tests/object_image.rs`.

`src/layout/replaced.rs` (`compute_replaced_layout`): a replaced element with only a ratio (an SVG with a `viewBox`
and no size) and an auto width is as wide as the available width less its borders and padding, a stretch-fit size;
its content box took the whole width, so the box overflowed by its padding (`replaced-intrinsic-003`, test
`a_ratio_only_image_fills_the_width_less_its_padding`).

## Links to the document itself are visited

`src/node/element.rs` (`ElementData::flush_link_visited`), `src/mutator.rs` (`create_element`, `set_attribute_impl`,
`clear_attribute_impl`: on an `href` change), `src/document.rs` (`link_urls`), `src/stylo.rs`: a link whose `href`
resolves to the document's own URL (fragment aside) has `ElementState::VISITED` instead of `UNVISITED`, and `:visited`
matches by it, as the page a browser shows is in its history (WPT `css-color/currentcolor-004`,
`color-mix-currentcolor-visited`, `css-backgrounds/background-image-gradient-currentcolor-visited`: `href=""`).
`:visited` matched nothing. `:link` matches every link, visited or not, as it does for Chromium's regular style
(Chromium takes only colours from the visited style), so a page's `:link` rules keep applying to such a link and its
`:visited` rules win where they come later or are more specific. Inside `:has()` `:visited` matches nothing, as
Chromium treats every link there as unvisited (`MatchingContext::relative_selector_anchor`; WPT
`css/selectors/invalidation/link-pseudo-in-has.html`, `css/selectors/has-visited.html`). There is no history, so other
links stay unvisited, and the user agent sheet keeps one link colour (`a[href]`): only a page's own `:visited` rules
change what such a link looks like. Limits: Stylo here computes no separate visited style
(`visited_styles_enabled: false`), so those rules apply in full rather than to colours alone, `getComputedStyle`
reports them (Chromium reports the unvisited values), a `:link` rule after a `:visited` one of the same specificity
wins, and the state is decided when the element is created or its `href` changes, not again when `pushState` changes
the document's URL. Tests: `a_link_to_the_document_itself_is_visited` in pyro-render's `background_images_and_edges.rs`,
`a_link_to_the_document_is_a_link_and_unvisited_inside_has` in `src/stylo.rs`.

## A scroll container's end padding in its scrollable overflow

`src/resolve.rs` (`resolve_transforms`, `padded_inflow_box`): a scroll container's scrollable overflow (which
pyro-render's `scrollWidth`/`scrollHeight` and scroll range read) includes each in-flow child's border box grown by
the container's padding and the child's margin (not below zero), as Taffy's `scrollable_overflow_rect` and Blink's
`ScrollableOverflowCalculator` have it: scrolled to the end, the end padding is in view after the content. The
root's and a `body` whose overflow is the viewport's are left alone. The scroll range was short by the end padding,
and a `content-box` local background reached the scrollport's edge at the end (WPT
`local-attachment-content-box-scroll`). Test: `the_scroll_range_ends_after_the_end_padding` in pyro-layout's
`scroll_origin.rs`.

## Generated content and pseudo-elements (WPT reftests)

- `src/layout/first_letter.rs`, `src/layout/inline/build.rs`, `src/node/text.rs`, `src/node/element.rs`,
  `src/resolve.rs`, `src/stylo.rs`, `src/document.rs`, `src/mutator.rs`: `::first-letter` is applied (it was
  cascaded and never used). The first letter of a block container's first line is its first typographic letter
  unit with the punctuation before and after it (Unicode Ps, Pe, Pi, Pf and Po, as CSS 2.1 and Chromium include);
  collapsible white space before it stays outside, and white space that does not collapse (a no-break space)
  is content before it, so there is no first letter (as in Gecko). A block passes its first line on to its first
  in-flow block child, so the letter may be in a descendant block, an inline or a `::before`; the nearest block
  container with a `::first-letter` style styles it; table cells, inline-blocks, floats and positioned boxes keep
  their first line, out-of-flow boxes are passed over, and an atomic inline or a `<br>` first means no first
  letter. The text keeps its node: before an inline root builds its inline layout (`resolve_deferred_tasks`),
  `first_letter::split` finds the letter (`TextLayout::first_letter`) and the build wraps its bytes in a span of
  the pseudo-element's style. The style lives on a node the originating element owns (`ElementData::
  first_letter`, not in the tree, no damage), the span's brush, so paint reads its colour, background and
  decorations as an inline's; its parent is the letter's text's parent, from which the style inherits (the rules
  the cascade matched, cascaded again with that parent: CSS 2.1's fictional tag sequence). A `::first-letter`
  style that came, went or changed in a style traversal builds the inline layout holding the letter again
  (`first_letter::style_changed`). A floated first letter (a drop cap) is not supported and is left unstyled.
- `src/layout/counters.rs`, `src/layout/construct.rs`, `src/resolve.rs`, `src/stylo.rs`, `src/mutator.rs`:
  `content` generates counters, quotes and images (only strings were). Before damage propagates, a walk in tree
  order applies `counter-reset`, `counter-increment` and `counter-set` (in that order, as Gecko does; a list
  item increments `list-item`) and evaluates the `content` of each `::before`/`::after` that generates a box:
  `counter()` and `counters()` in the predefined counter styles (numeric, alphabetic, additive, cyclic; an
  unknown name or a value out of a style's range is decimal) and `open-quote`/`close-quote` at the document's
  quote depth. `quotes: auto` takes the quotes of the language of the originating element's parent from
  `src/layout/quotes_table.rs` (CLDR 48 delimiters of the ICU locales, generated; a tag not found loses its
  last subtag, as Chromium's `LayoutLocale::GetQuotesData`; English's when none matches), and a `lang`
  change runs the walk again. A counter is visible to its element, its descendants and its
  following siblings; one a sibling made is replaced; an element with no box (`display: none`, `display:
  contents`) changes none. The walk runs once a traversal kept a `::before`/`::after` whose content reads a
  counter or a quote (sticky), and only after a restyle changed what counters read of an element (its and its
  pseudo-elements' counter properties, `content`, `display` and `quotes`: `CounterInputs`) or a child list
  changed (`counters_dirty`); an owner whose content changed is damaged. `content: url()` makes an `img` box (an
  inline replaced box inheriting from the pseudo-element) fetched as `<img>` is (`load_generated_image`).
- `src/layout/construct.rs`: the pseudo-elements of an inline inside an inline are made (the walk read the
  parent's display, so only an inline root's children had theirs).
- `src/layout/damage.rs`: `clear_damage_and_dirty_flags` clears the flags of what a pseudo-element generates (an
  image's box): `mark_damaged` from the image's load stopped at its stale flag, so it was never laid out.
- `src/layout/list.rs`, `src/layout/construct.rs`: a `::before` or `::after` with `display: list-item` gets a
  marker. An inside symbol marker is text in the item's font when the page styles `::marker` (as Gecko draws
  it), the bundled bullet glyph otherwise. An inside marker that is text (a number, or such a symbol) makes a
  line of its own in an item with no other content; an empty item's bullet-font symbol marker stays unpainted
  (painting it exposes WPT `list-style-position-applies-to-015`, an empty item in a table caption, until table
  captions render).
- `assets/default.css`: `area` is `display: none` without Gecko's `!important`, as in Chromium.
- `src/stylo.rs`: `<body text>` maps to `color` (a legacy colour value).
- `src/layout/first_line.rs` (new), `src/layout/first_letter.rs`, `src/node/text.rs`, `src/node/element.rs`,
  `src/resolve.rs`, `src/stylo.rs`, `src/document.rs`, `src/layout/counters.rs`: `::first-line` is applied (the
  pref is on: it was not even parsed). As for `::first-letter`, the nearest block container whose first line is an
  inline root's (`first_letter::origin`, now by pseudo-element) styles it, its style cascaded again with the
  block holding the line as parent when that is a descendant. Before a root builds its inline layout,
  `first_line::style_node_for` finds the style and keeps it on a node the originating element owns (`ElementData::
  first_line`, not in the tree, its parent the root): `TextLayout::first_line`. The layout does not use it (the
  lines are broken in the root's own styles: fonts and spacing of the first line are not applied); blitz-paint
  draws the first line with it. `BaseDocument::first_line_style` gives an element's style on the first line: its
  own reparented onto the first-line box (Stylo's `FirstLineReparenting`, as Gecko does), kept across paints while
  its own and its new parent's styles are the same (`first_line::ReparentCache`). A `::first-line` style that came
  or went builds the block's inline layout again; one that changed on a line that has its node only restyles the
  node (`first_line::style_changed`: the layout does not read it, so a page-wide restyle relays no first line out).
  `first_letter::origin` passes anonymous blocks (whose anonymous style computes `display: inline`) and inlines split
  around blocks on to their container, so a first line or first letter in a block-in-inline's first piece is styled.
  The `::first-letter` and `::first-line` nodes load their background images (`pending_style_image_nodes`).
- `src/dom_selection.rs` (new), `src/document.rs`, `src/lib.rs`: the embedder's selection (`DomSelection`, text
  nodes and UTF-16 offsets; pyro-render sets pyro-webapi's for a screenshot) replaces blitz's own in
  `get_text_selection_ranges`: the text nodes from the start to the end in tree order, mapped through each inline
  root's `text_nodes` to bytes of its text (collapsed white space maps to where it collapsed).

Tests: `generated_content`, `first_line` and `selection_highlight` in pyro-render; `first_letter`,
`counters` and `dom_selection` unit tests here.

## `:dir()`, bidi overrides across `<br>`, `text-indent` of anonymous blocks (WPT reftests)

- `src/direction.rs` (new), `src/stylo.rs`, `src/mutator.rs`: `:dir(ltr)` and `:dir(rtl)` match by the element's
  directionality (HTML: the nearest valid `dir`; `dir="auto"` and a `<bdi>` by the first strong character of their
  text or a text control's value; a `tel` input left to right; the root left to right), the rules pyro-dom applies
  for script. Stylo parses `:dir()` for servo now (`vendor/stylo/README.pyro.md`). A `dir` change restyles the
  element's descendants too.
- `src/layout/inline/build.rs` (`BidiScan::open_controls`, `reopen_bidi_controls`): after a forced line break
  (`<br>`), the bidi formatting characters of the embeddings, overrides and isolates still open are pushed again:
  the break is a paragraph separator to the bidi algorithm, which closes them, and Blink re-opens them
  (`InlineItemsBuilder::AppendForcedBreak`), so `unicode-bidi: bidi-override` holds on the lines after a `<br>`.
- `src/layout/inline/layout.rs`, `src/layout/first_line.rs` (`starts_a_block`): `text-indent` indents the first
  line of an anonymous block only when it is its container's first in-flow child (CSS 2.1 16.1), not the
  anonymous block after an inline's block-level content.

Tests: `direction_text` in pyro-render; `direction` unit tests here.

## Shadow trees (CSS Scoping) and directionality through them (WPT reftests)

pyro-dom's render tree is the flat tree: a shadow host's children are its shadow tree's (it carries the
`urn:x-pyro:state` `shadow-host` marker), a slot of a shadow tree is an element of its own (`slot { display:
contents }`) whose children are the nodes assigned to it (marker `assigned-slot`) or else its fallback content.
Before, shadow styles were global and slots were left out (no `:host`, `::slotted()`, `::part()`, no slot boxes).

- `src/shadow.rs` (new), `src/tree.rs`, `src/node/node.rs` (`IS_SHADOW_HOST`, `IS_ASSIGNED_SLOT`), `src/mutator.rs`
  (markers synced on create and attribute changes): the node trees follow from the flat tree. An element whose
  parent is a host is in that host's shadow tree; one whose parent is an assigned slot, in the tree of the slot's
  host; else in its parent's (`Node::containing_shadow_host_id`, a walk up; `NodeTree::has_shadow_hosts` makes it
  free on pages without shadow trees). Its parent for selectors (`Node::dom_parent_element`) is none at a shadow
  tree's first level (the shadow root; `:host` reaches the host through Stylo's featureless path) and the host for
  an assigned node. Siblings stay the flat tree's: an assigned node's are the nodes assigned to the same slot.
- `src/stylo.rs`: `TShadowRoot` is the host itself (no node stands for a shadow root); `style_data` is the host's
  shadow cascade data. `shadow_root`, `containing_shadow`, `assigned_slot`, `parent_node_is_shadow_root`,
  `containing_shadow_host`, `is_html_slot_element`, `::part()` (`part`, `exportparts` with `inner:outer` mappings)
  and `inheritance_parent` (the flat tree's parent, as Servo; Stylo's default is `parent_element`, now the node
  tree's) are implemented; `implicit_scope_for_sheet_in_shadow_root` answers `None` instead of panicking.
- `src/document.rs`, `src/shadow.rs`: the sheet of a `<style>` or `<link>` element of a shadow tree (adopted sheets
  come as `<style>` elements) goes to its host's Stylo `AuthorStyles` (`ShadowStyles`), not the document's stylist;
  `resolve_stylist` first rebuilds the cascade data of shadow roots whose sheets changed (Stylo's author data cache
  shares one for the instances of a component, whose `<style>`s share their parsed sheet, `make_stylesheet_in`) and
  restyles those hosts' subtrees. A device change marks every shadow root's sheets dirty (media queries).
- `src/direction.rs`: `:dir()` follows the node trees (a shadow tree's first level takes the host's directionality,
  an assigned node its host's, not its slot's), and takes the directionality of a `dir=auto` element or a `<bdi>`
  from the `dir=ltr`/`dir=rtl` token of its state attribute (pyro-dom computes it by the HTML rules, from the light
  tree's text, values and slots); a change of that token restyles the element's descendants. Without a token (not
  pyro-dom's tree), from its own flat-tree text as before, left to right when nothing decides (the current HTML
  rule; it was the parent's).
- `assets/default.css`: the HTML bidi rules as written (`[dir]:dir(ltr), bdi:dir(ltr), input[type=tel i]:dir(ltr)`,
  `[dir]:dir(rtl), bdi:dir(rtl)`, `[dir] { unicode-bidi: isolate }`): `dir=auto` and `<bdi>` now take their
  content's direction.
- `src/has_invalidation.rs`, `src/stylo.rs` (`invalidate_host_relative_selectors`): Stylo's `:has()` invalidation
  walks a change's ancestors up to the shadow root, not to the host: a change in a shadow tree whose sheets have
  `:has()` rules (`:host:has(.x)`) restyles the host's subtree. State changes in shadow trees run the relative
  invalidation whatever the document's sheets depend on.
- `src/stylo.rs`, `src/has_invalidation.rs`: `:defined` matches every element but a custom element not defined yet
  (pyro-dom's `undefined` state token; `ElementState::DEFINED`, so the upgrade restyles it). Upstream matched none,
  so `:not(:defined)` (a component hidden until upgraded) matched every element.
- `src/document.rs` (`set_shadow_hosts_focus`): the shadow hosts whose shadow trees hold the focused element match
  `:focus` too (CSS Scoping 1 § 3.1; WPT shadow-dom `focus-pseudo-on-shadow-host-*`).
- `src/layout/inline/build.rs`: the text of a `display: contents` element (a slot's assigned text) takes the element's
  styles: its font, spacing and white space, and through the brush its colour. In an inline formatting context the
  element is a span (upstream took only its white space); its content hoisted into an anonymous block
  (`collect_layout_children_with_wrap`) is wrapped in the spans of its `display: contents` ancestors
  (`contents_ancestors`), which it lost. `display: contents` still draws no decoration (`own_decoration`).

- `src/layout/list.rs`, `src/layout/construct.rs`: a list's walk numbering its items goes on through `display:
  contents` children (`list_numbering`: a list element among them numbers its own), and such a child flattened into
  its parent's boxes numbers none itself: items assigned to different slots of one `<ol>` are 1, 2, 3 (WPT css-lists
  `counter-list-item-slot-order`; each slot restarted at 1 once slots were elements).

Tests: `shadow_scoping` in pyro-render. Selector matching on the featureless host: `vendor/selectors/README.pyro.md`.

## List markers (WPT reftests)

- `src/layout/list.rs` (`image_marker`), `src/node/element.rs` (`ElementData::list_style_images`, `Marker::Image`),
  `src/util.rs`, `src/layout/damage.rs`, `src/stylo.rs`, `src/document.rs`: `list-style-image` is loaded, as an image
  layer of its own flushed with the background, mask and border images, and an outside item whose image loaded has
  an image marker at the image's natural size (an SVG's own size), which blitz-paint draws (`draw_marker_image`). Until
  the image loads, or when it fails, the item keeps its `list-style-type` marker; the load builds the item again. An
  inside item keeps its type marker (no inline image marker is made).
- `src/layout/construct.rs` (`place_inside_marker`): the inside marker of an item of blocks is on the first line of its
  first box, as in Chromium: the anonymous block of its first inline content takes it, and before a first block it is
  the line of an anonymous block of its own. An empty item gets none (as the bullet above about
  `list-style-position-applies-to-015`).
- `src/layout/construct.rs`, `src/layout/inline/build.rs` (`outside_marker`): an item with an outside marker and no
  in-flow content (empty, white space, out-of-flow boxes) is an inline root whose one line is its strut (a word joiner
  at its start): the line the marker sits on. Chromium gives the marker's box to the item (its height and baseline),
  so an empty item in an inline-block or a table cell lines up as one with text (WPT
  `list-style-*-applies-to-012`, `-014`); it was 0px tall, with the marker a line lower.

Tests: `an_image_marker_ends_7px_before_the_item_on_its_first_baseline`,
`an_inside_marker_is_a_line_before_block_content`, `an_empty_item_has_the_line_of_its_outside_marker` and
`an_outside_marker_goes_with_the_line_a_float_moves` in `crates/pyro-render/tests/generated_content.rs`.
## Table structure (WPT reftests)

- `src/layout/table_fixup.rs` (new), `src/layout/construct.rs` (`collect_layout_children`), `src/layout/table.rs`
  (`build_table_context`, `collect_table_cells`), `assets/default.css`: the anonymous table objects of CSS 2.1
  §17.2.1 (css-tables-3 §3.3). Consecutive table parts among a block container's layout children (cells, rows,
  row groups, columns, column groups, captions; not replaced elements, whose table display is inline content)
  are wrapped in an anonymous table; a table's, row group's or row's children that are not its proper children
  (text, inline and block boxes, nested tables, `::before`/`::after`) in anonymous rows or cells. White space
  between table parts, or before or after one at the start or end, is dropped whatever `white-space` says (as in
  Chromium); `display: contents` boxes are looked through; out-of-flow boxes stay the table's. The anonymous boxes
  are slab nodes (`NodeData::AnonymousBlock`) styled as the UA sheet's `::-servo-anonymous-table`, `-row` and
  `-cell` boxes from the box they are in, owned (`Node::anonymous_blocks`) by the box whose children they wrap;
  their `children` are the DOM nodes they wrap. A table part's `::before`/`::after` are made as its children are
  fixed up. A table, row group or row whose children are all proper ones (or white space, hidden or out of flow)
  and that has no pseudo-element boxes is built from its children as they are, as before (no allocation, one
  style read per child). Tables in a vertical writing mode keep the former construction (their stray children are
  left out).
- `src/layout/table.rs` (`offer_box`): a box without borders offers none to the collapsed borders (most rows,
  groups and columns); the cells' grid areas are worked out once for the collapsed borders and their cells.
- `src/layout/mod.rs` (`lay_out_table_captions`, `table_captions_min_width`), `src/layout/table.rs`
  (`TableContext::captions`, `grid_box`): captions are laid out above (`caption-side: top`) and below the grid at
  its width less their margins; the table is at least as wide as their min-content widths; its box is the
  wrapper box and its grid moves down by the captions above it (`grid_box` tells paint where the table box proper
  is). A table with captions does not keep its grid's last layout (`take_table_grid_state`).
- `src/layout/table.rs`: collapsed borders are resolved before the columns get their widths, and a fixed table's
  first-row cell width gives its column a border box with half of the collapsed borders beside it (CSS 2.1
  §17.6.2); columns and column groups take part in conflict resolution (cell, row, row group, column, column
  group, table); a collapsing table has no padding; a uniform collapsed table laid out as gutters and a separated
  table (border-spacing as padding) take those out of a content-box table's lengths, which count half of the outer
  borders and the spacing; a fixed table is never narrower than its columns' widths and the spacing between them;
  a row or row group with `visibility: collapse` takes no height (`TableRow::collapsed`; `src/stylo.rs` makes the
  change build the table again).
- Incremental: a table with anonymous boxes, generated cells or rows, captions, rows or cells reached through
  `display: contents`, or that is anonymous is built again on any change; `update_rows` builds the table again
  when a changed row is anything but a row, or a group gains text, a non-row child or a pseudo-element.
  `src/layout/damage.rs`: an anonymous table, row or cell's owner clears its layout cache and notes its cells as
  changed (`clear_anonymous_table_box_caches`); its style is refreshed as the anonymous box it is, from its parent
  (`refresh_anonymous_block_styles`); a `writing-mode` change builds the box again. `src/document.rs`: a freed
  anonymous box leaves no `layout_parent`/`layout_child_of` pointing at it, and `drop_part_box` clears a part's.
- `src/stylo_to_kurbo.rs`, `src/lib.rs`: `resolve_2d_transform_for_size` (blitz-paint transforms a row's cells by
  the row's transform); `TableCell` and `TableContext` are exported.

- Lane P2 (tables, second pass; tests in `table_parts` in pyro-render, each fails without its change):
  - `src/layout/table_fixup.rs` (`mark_space_between_inline_table_parts`), `src/layout/construct.rs`
    (`find_inline_layout_embedded_boxes`), `src/layout/inline/build.rs` (the text arm), `src/node/node.rs`
    (`NodeFlags::IS_SPACE_BETWEEN_TABLE_PARTS`, `ENCLOSES_TABLE_PARTS`): white space between two table parts of an
    inline box (laid out there as atomic inlines) generates no box, as if they were one anonymous inline table (CSS
    2.1 §17.2.1). The construction of the inline formatting context notes an inline box with table parts among its
    children and, once it has visited them (its children are taken while it does), flags the white-space text
    nodes between two parts (comments and `display: none` passed over); the inline build skips them.
  - `src/layout/table.rs` (`TableCell::baseline`, `is_baseline_aligned`, `table_cell_baseline`,
    `set_cell_baseline_offset`, `shift_cell_content`), `src/layout/mod.rs` (`compute_child_layout_in_current_wm`),
    `src/node/element.rs` (`LayoutData::table_baseline_offset`), `NodeFlags::HAS_TABLE_BASELINE_OFFSET`: cells with
    `vertical-align: baseline` (or any value but `top`, `middle`, `bottom`; `align-content: normal`), spanning one row,
    in a horizontal table, are aligned on their row's baseline (Taffy's `table_baseline` works out the offsets from
    each cell's baseline, `table_cell_baseline`: its own layout's, else the bottom of its content box when it has
    content, which an empty cell does not, whatever its `height`). A cell's offset is stored on it before its final layout; a change clears its layout cache. Taffy sets the offsets
    only in a table with such cells, so a cell collected as not aligned (`collect_table_cells`) drops its own: a table
    that lost its last baseline-aligned cell, or turned vertical, lays its cells out as a fresh build does. Its layout
    then moves its in-flow and floated children, its lines (`TextLayout::block_offset`), its out-of-flow candidates'
    static positions, its overflow and its baselines down by the offset, inside the layout cache, so a cached layout
    keeps them there. A table with such cells hands its `TableGridState` back only while none of its cells
    or rows changed (then its last layout stands); after a change it is laid out whole.
  - `src/layout/table.rs`: a fixed table's cells may be narrower than their horizontal padding
    (`table_cells_narrower_than_padding`); a first-row cell's percentage width in a fixed collapsed table gets the
    halves of the collapsed borders beside it, as a length width does; a `<col>`'s width holds the halves of the
    collapsed borders (a cell's own width gets them added, then the larger wins); a column group without columns
    stands for its `span` columns (`push_columns`; its width, background and borders are theirs); a cell's length
    `max-width` caps the width it gives its column; in the automatic layout a cell spanning columns gives none of
    them its width (it is a minimum of the columns and the gaps it spans); `TableContext::gutter_border_color`,
    `CollapsedEdge::outranks` (for paint).
  - `src/node/stylo_data.rs` (`cell_layout_style`): a table in the collapsing border model has its layout style
    without padding (CSS 2.1 §17.6.2), so its container sizes it, and paint positions its background, without it.
  - `src/layout/mod.rs` (`lay_out_table_captions`): the out-of-flow boxes of a caption whose containing block is
    further up are passed on with the table's.

Tests: `table_structure` in pyro-render (each fails without its change); `mirror` covers the incremental paths.

## Ruby annotations and emphasis marks

- `src/node/text.rs` (`TextAnnotation`, `TextAnnotationKind`, `can_take_emphasis_mark`), `src/layout/inline/build.rs`
  (`FoundAnnotations`, `build_annotation`, `annotation_room`, `emphasis_mark`): an inline root's build finds its ruby
  annotations and emphasis marks. An `<rt>` (or `<rtc>`) inside a `<ruby>` is not part of the root's text: it annotates
  the text since the ruby (or the annotation before it) started, its base. An element whose `text-emphasis-style` is
  not `none` has marks over its own text. Once the root is built each annotation gets a layout of its own, one line:
  the `<rt>`'s content in its style, or the mark (`dot` `•◦`, `circle` `●○`, `double-circle` `◉◎`, `triangle` `▲△`,
  `sesame` `﹅﹆`, or the string) in the element's font at half its size, as an `<rt>` has it. `under` is
  `ruby-position: under` or `text-emphasis-position: under` (`auto` is `under right` for Chinese, by `-x-lang`;
  in a vertical writing mode `left` is under the line). blitz-paint draws them (`draw_text_annotations`). Marks skip
  word separators, `Z*`, `Cc`, `Cf`, `Cn` and `P*` characters (CSS Text Decoration 3, 3.1).
- Room in the line: each annotation's content area (ascent to descent of its line) is room its holder needs beyond
  its content area, over or under it (`parley::TextStyle::annotation_space`; nested ruby stacks: an annotation of a
  base that has one goes outside it). The root is built again with it once it has any, so the line box grows where
  the line height leaves too little (CSS Ruby 1, 4.4), for ruby and marks alike (WPT `text-emphasis-line-height-*`).
- `src/stylo_to_parley.rs`: `annotation_space` is set by the build only.
- `src/layout/damage.rs` (`compute_layout_damage`): a change of `text-emphasis-style`, `text-emphasis-position` or
  `ruby-position` collects the inline layout again (Stylo gives `text-emphasis-style` relayout damage for it).
- `src/document.rs`: the prefs `layout.css.text-emphasis.enabled`, `layout.css.ruby.enabled` (`ruby-position`,
  `ruby-align`) and `layout.css.text-decoration-inset-percentage.enabled`.
- Not done: ruby layout proper. An annotation wider than its base does not space the base out, `ruby-align` is
  ignored, `<rt>`'s box (background, border) is not painted, and `display: ruby*` stays unparsed (Stylo has it
  for Gecko only), so ruby is found by element name. Covered by `crates/pyro-render/tests/text_emphasis_ruby.rs`.

## Multi-column layout

- `src/layout/multicol.rs` (new), `src/layout/mod.rs` (`ChildLayout::Multicol`, `column_spanners`): a block container (`block`,
  `flow-root`, `inline-block`, `list-item`) whose `column-count` or `column-width` is not `auto`, in a horizontal
  writing mode, is a multi-column container (CSS Multi-column Layout 1). Its content is laid out once, in a strip one
  column wide (its block or inline formatting context at the column width, as tall as its content: legacy WebKit's
  flow thread), and the strip is broken into columns. Every box keeps its one layout, in strip coordinates; each
  column is a fragment (`ColumnFragment`: the strip range it shows and where), stored in `LayoutData::columns` by
  the container's final layout and read through `Node::column_fragments` (which ignores them once the style makes the
  box no multi-column container). Column count and width follow §3.4 (`column-gap: normal` is 1em); intrinsic
  widths follow Chromium's `ColumnLayoutAlgorithm::ComputeMinMaxSizes`.
- Breaks: line boxes, replaced elements, scroll containers, tables, flex and grid containers, form controls and
  nested multi-column containers are monolithic (a break goes before them, or through one taller than a column, where
  the column ends); block containers break between their children and anywhere through their own empty space; the
  column after a break between blocks starts at the next one's border box (margins truncated). `orphans`, `widows` and
  `break-inside: avoid` keep lines and boxes together where a column can hold them.
- Heights: columns are balanced (the least height that fits the content in `column-count` columns, grown by the
  smallest shortage until it does, keeping together what `orphans`/`widows`/`break-inside` keep where the height
  allows) unless `column-fill: auto` and the height is definite; a definite height or `max-height` caps them, and
  content that does not fit continues in columns past the container's inline end, as in Chromium. A container of an
  auto height is as tall as its columns. Column offsets and breaks are snapped to device pixels.
- Column spanners: an in-flow block child with `column-span: all` is laid out in the strip as wide as the
  container's content box (`LayoutPassState::column_spanners`, read by `adjusted_child_inputs`) and is a row of its
  own: the content before it is balanced into a row of columns above it, the content after it into a row below.
- The container is its own block formatting context (its children's margins stay inside; its own do not collapse
  with them), and its parent keeps it clear of floats as it does a `flow-root` (Taffy's
  `block_child_establishes_formatting_context`, answered in `layout/mod.rs`). A `min-height` or `max-height` of the
  container clamps the strip Taffy lays out, so under either the content's end is taken from the laid-out content
  (its children's margin edges) instead. It is a paint layer root (`paint_tree.rs` `is_paint_layer_root`): its
  positioned descendants paint in its columns. A z-indexed descendant hoisted past it is placed through its columns
  (`hoisted_child_position`). Known deviation: its `z-index: auto` positioned descendants and its non-positioned
  stacking contexts paint at the container's place in in-flow order, not at the enclosing stacking context's
  positioned step, so a later in-flow sibling's background covers them where they reach past it, and an earlier
  positioned sibling covers them; an out-of-flow box whose containing block is the container is moved through its
  columns too (Chromium does not fragment those).
- Mapped through the columns: the scrollable overflow and hit bound (`resolve.rs`: the content's extent, cut at the
  breaks and moved to the columns, added to the padding box), hit testing (`Node::hit_inner`: the point the column
  under it shows), `absolute_position` and `physical_unrounded_geometry` (a box is placed in the column whose range
  holds its own origin; an origin in a margin truncated at a break, in the column after the break).
  `BaseDocument::has_multicol` tells pyro-render's geometry queries that columns exist (it is never cleared).
  pyro-render's `Geometry::columns_pieces` cuts a box at the breaks: `getBoundingClientRect` reports the union of the
  pieces, `getClientRects` one rect per piece, and pyro-dom hit-tests each piece.
- A container laid out to measure it (a `ComputeSize` run) writes its strip's layouts at that width, so it clears its
  own layout cache: its final layout runs again and puts them back. This relies on the container's final layout
  running after a measure in the same pass; an ancestor whose own final layout hits its cache after a measure missed
  would leave the strip at the measured width (no page doing so is known).
- Not supported: spanners deeper in the content (laid out in a column), forced breaks
  (`break-before: column`; Stylo has no `column` value), fragmenting monolithic boxes such as tables between rows,
  and vertical writing modes (laid out as one column). In blitz-dom's own geometry a box split by a break is placed
  at the column of its origin.

Tests: `layout::multicol` unit tests here (column count and width, balancing, breaks, the mapping);
`crates/pyro-render/tests/multicol.rs` (pixels) and `crates/pyro-layout/tests/multicol.rs` (geometry and hit
testing).

## Moving from 0.3.0-beta.4 to main

The local changes were merged with main's 124 commits. Upstream now has these, so the local versions were
dropped:

- Class tokens interned once per element: upstream #1240 (`ElementData::classes`, kept by `set_attribute`).
  This was the original `UPSTREAM.patch`; `class_selectors_match_as_before` still covers it.
- `font-optical-sizing: auto` as an `opsz` variation: #1360.
- Unresolvable URLs load nothing instead of panicking (`resolve_url` returns an `Option`): #1374.
- `:focus-within`: #1226 (the local `set_focus_within` went; upstream's ancestor-chain diff remains).
- Narrow restyles on child-list changes, `:empty` and `:has()` anchor parents: #1276
  (`restyle_for_child_list_change`). The local `:has()` invalidation stays: an inserted or removed element
  goes through Stylo's relative selector invalidator (`invalidate_relative_selectors_for_insertion`/`_removal`,
  below), a child-list change that flips an argument's `:empty` or sibling position through the stylist's
  tree-structural dependencies (`invalidate_relative_selectors_for_ts_state`), an attribute or state change
  through `invalidate_has_anchors`, which reaches anchors among ancestors and their earlier siblings; #1276's
  own anchor-parent hint (a `RESTYLE_SELF` of an anchor on every change of its child list) went with them.
  On top of #1276,
  `restyle_for_child_list_change` leaves the siblings alone when no element came or went (text nodes move
  no index or edge status) and restyles the styled children of the inserted range, which are moves within
  the same parent: `add_children_to_parent` no longer restyles their subtrees, their ancestors being unchanged.
- `::before`/`::after` boxes voting in the flow classification: #1280.
- Mid-line floats that do not fit placed below the line: #1378 and #1390 (with line retry), replacing the
  local pending-float queue in inline layout.
- Paint children kept in order without a key buffer: #1445 sorts precomputed keys (a sorted run is linear),
  replacing the local `is_sorted_by_key` check. The table paint-children bookkeeping is unchanged.

Kept, adapted to main:

- `layout/inline/build.rs`, `layout/inline/layout.rs`: main split `layout/inline.rs` (#1233); the local
  `<br>`, strut and text-node tracking moved with the build half. The skip of a line break with the same
  inputs (`TextLayout::last_break`) is keyed on main's `max_advance` (which carries #1449's rounding
  slack) instead of the width. Shrink-to-fit widths still round up to a layout unit (`ceil_layout_unit`);
  main dropped the whole-pixel ceil (#1375), so this keeps Chromium's 1/64 px instead of the raw width.
- `layout/writing_mode.rs`: incremental rounding with main's `ScrollableOverflow` (#1372). Upstream's
  `content_box_subpixel_offset` (#1382: text drawn from the unrounded content box) is left at zero:
  screenshots keep whole-pixel text as before and print moves text to its layout-unit x itself
  (`fractional_text_x` in blitz-paint). The scroll-container check `physical_layout` needs is only made
  under a vertical placer, the only case that reads it.
- `src/document.rs`: Stylo 0.23 renamed the prefs (`layout.css.*`); the local ones are set in main's
  `init_stylo_prefs`. Main shares parsed user-agent sheets through a read-only lock, which the thread's
  shared author lock can read.
- `src/net.rs`: main decodes linked sheets from bytes (#1388); `linked_page_css` keeps the text of one with
  `@page` rules from the same bytes.
- `paint_tree.rs`: the out-of-flow stacking-context fix (for upstream #807) and `table_paint_children_kept`
  sit on #1445's positioned-descendant lifting.

New, because main narrows attribute restyles to the element (#1276) where 0.3.0-beta.4 restyled the
parent's subtree and damaged the element:

- `src/mutator.rs` (`restyle_siblings_for_attribute_change`): an attribute change restyles the element's
  siblings when the parent carries `HAS_SLOW_SELECTOR_NTH_OF` (Stylo leaves `:nth-child(of S)` to the
  embedder, as Gecko's `RestyleSiblingsForNthOf`), and a change of pyro's state attribute
  (`urn:x-pyro:state`) is layout damage when its `natural=W,H` token changes. Its pseudo-classes
  are element states (`sync_lp_attribute_state`, "`:has()` invalidation for state and attribute changes"
  above), so the snapshot sees them change and Stylo's state invalidation reaches the siblings; nothing
  restyles them here. Covered by `sibling_and_fragment_invalidation_stays_exact`,
  `radio_group_changes_invalidate_selectively_*`, `form_state_writes_invalidate_selectively_*` and
  `loaded_image_natural_size_reaches_flow_through_the_dom` (pyro-layout).
- `src/stylo.rs` (`RecalcStyle::process_preorder`): a paint-only restyle of a table part in a
  `border-collapse: collapse` table that changes its borders or `color` is damage on it: the collapsed
  edges (`CollapsedEdges`) hold resolved colours, collected again with the part's row. Covered by
  `tables_lay_out_from_the_styles_written_at_restyle` (pyro-render).

## From be60597 to c27b4ca

Main's 18 commits merged with three conflicts:

- `src/document.rs`: `BaseDocument` keeps main's `flags: DocumentFlags` next to the local layout fields.
- `src/layout/resolve.rs`: main's `resolve_all` (every node resolved again after a scroll or viewport
  change) is threaded through the local incremental pass: `resolve_transforms(node_id, resolve_all)` skips
  an unchanged subtree only when it is false, and so does `table_cells_overflow`.
- blitz-paint `src/text.rs`: see its README.

## Stylo 0.24

Main pins Stylo 0.23.0; this copy builds against the vendored 0.24.0.

- `src/node/attributes.rs`, `src/stylo.rs`: Stylo 0.24 borrows the declaration blocks of presentational
  hints from the element for the length of the cascade
  (`TElement::synthesize_presentational_hints_for_legacy_attributes<'a>`), where 0.23 took a new `Arc` per
  declaration at every cascade. `Attributes` owns them (`PresentationalHints`, built by
  `Node::build_presentational_hints` at the first cascade that asks, dropped by every attribute write:
  `set`, `remove`, `DerefMut`; a clone builds its own). A table's `cellpadding` maps to four padding blocks
  the table owns (`PresentationalHints::cells`) and its cells borrow (`Node::cell_table`), so a cell's
  hints depend on its own attributes only and cells of one table push the same blocks.
- `src/stylo.rs`: `pseudo_style` passes its `AnimationDeclarations` by reference;
  `:-moz-topmost-modal` never matches.
- `src/has_invalidation.rs`: given snapshots, 0.24 drops a `:has()` dependency whose compound matches
  the changed element as it did per its snapshot (`RelativeSelectorDependencyCollector::add_dependency`).
  `run_relative_invalidation` passes them only when the element has a snapshot and the change is not one
  of custom states: an element never styled (below `display: none`) gets no snapshot
  (`snapshot_node_impl`), and Servo's snapshot keeps no custom states, so both read as unchanged and the
  anchor kept its style (WPT `css/selectors/invalidation/has-unstyled.html`, `state-in-has.html`). Test:
  `has_anchors_follow_changes_stylo_has_no_snapshot_for` in pyro-render.
- `src/stylo_device.rs`: `Device::new` takes the submit button's default label.
- `src/document.rs`: the pref is `layout.css.text-underline-position.enabled` now.

## Inline boxes split around blocks, `unicode-bidi`, relative inline boxes

- `src/layout/construct.rs` (`split_inline_sequence`, `split_inline_part`, `lays_out_as_block`, the split branch of
  `collect_complex_layout_children_in`), `src/node/node.rs` (`NodeFlags::IS_SPLIT_INLINE`, `ENDS_WITH_SPLIT_HEAD`,
  `STARTS_WITH_SPLIT_TAIL`, `Node::split_ref_at`), `src/layout/inline/build.rs` (`build_split_part`,
  `split_pieces`): an inline box that holds block-level boxes is split around them (CSS 2.1 § 9.2.1.1) where its
  parent lays out as a block container (a flow, flow-root or table cell box; a table part outside a table is laid
  out as a flex row and keeps one box). It was one block box with its margins, borders and padding around all it
  held. Its head, the inline content before its first block-level child, ends the anonymous block before it
  (`ENDS_WITH_SPLIT_HEAD`: the block's last layout child is the split inline, whose head its build lays out), and
  its tail, the content after its last block-level child, starts the anonymous block after it
  (`STARTS_WITH_SPLIT_TAIL`); both are laid out in the split inline's span, the start side in the head and the end
  side in the tail. The split inline stays a block-level box holding its middle (its first block-level child to its
  last, with the floats and absolutely positioned boxes right next to them, which float and paint beside the blocks
  as before), but as a box it has no margins, borders, padding or sizes (`stylo_taffy::StyleFlags::IS_SPLIT_INLINE`);
  its relative offset moves it. The anonymous blocks of its middle inherit from the block container around it (its
  strut, alignment and direction) and wrap their content in the spans of the split inlines around them, without
  their sides (`TextLayout::split_pieces`). A split inline at the edge of another's middle has its head (tail) in
  the outer one's. A split inline's own construction is damaged (`CONSTRUCT_BOX`) when its container's is.
- `src/layout/inline/layout.rs`: margins collapse through an inline root with no line (a div holding only an empty
  span), as through an empty block.
- `src/layout/inline/build.rs` (`bidi_controls`, `BidiScan`), `src/node/text.rs` (`TextLayout::bidi_controls`,
  `has_no_text`): `unicode-bidi` and `direction` of inline boxes and block containers apply (only `plaintext` on the
  root did): an inline box's `embed`, `bidi-override`, `isolate`, `isolate-override` or `plaintext` is the bidi
  formatting characters Blink inserts for it (CSS Writing Modes 3 § 2.4.2), pushed around its content
  (`TreeBuilder::push_bidi_control`); a block container's `bidi-override` starts its content with LRO or RLO. They
  are pushed only where they can change a level: some box has them and the paragraph has right-to-left content (a
  right-to-left root or box, or right-to-left text); the first build finds out and a second build pushes them. An
  inline box that holds nothing pushes a strut (U+2061, `TreeBuilder::push_empty_box_strut`), so that its line
  height counts, as an empty span's does in browsers. Text that is only such characters is no content. A
  right-to-left span's edges are marked (`InlineBox::rtl`) for parley to put them on their sides.
- `src/layout/inline/layout.rs` (`line_is_reordered`): the static position of an absolutely positioned box on a
  right-to-left or reordered line follows the inline content before it in logical order.
- `src/layout/inline/relative.rs` (new), `src/layout/inline/layout.rs`, `src/layout/inline/build.rs`:
  `position: relative` on an inline box that is not atomic (CSS 2.1 § 9.4.3) moves what it holds: the atomic inline
  boxes in it are laid out moved, and paint moves its text and its fragments
  (`BaseDocument::inline_span_relative_offset`, read only for an inline root with such a box,
  `TextLayout::relative_spans`). Decorations stay with the line.
- `src/layout/mod.rs`: an SVG image's (or inline `<svg>`'s) natural width and height are its root's absolute
  `width` and `height`, and its ratio theirs or else its `viewBox`'s; what it does not declare it does not have,
  and the default object size (300x150) stands in, as in browsers (usvg's resolved size stood in). The root `<svg>`
  of an SVG document keeps usvg's size: it fills the viewport in browsers, which is not done.
- `assets/default.css`: `bdo[dir]` stands for Gecko's `bdo:-moz-has-dir-attr`, and `[dir="auto" i]` for
  `:-moz-dir-attr-like-auto`: Stylo parses neither, which dropped both rules.

Covered by `block_in_inline.rs`, `unicode_bidi.rs`, `inline_relative_position.rs` and `line_paint_order.rs` in
`crates/pyro-render/tests`.

## Anonymous boxes in 3D rendering contexts

- `src/node/node.rs` (`set_transform`, `transform_context_for_children`), `src/resolve.rs`
  (`resolve_transforms`): an anonymous box (an anonymous table around a cell, an anonymous block) has no
  `transform-style` of its own: it is in its parent's 3D rendering context, if any, and passes it on, and it
  stands for the box it is in, whose DOM children are the boxes it lays out (`ContainingTransform::parent`), as
  Chromium's anonymous boxes do. Flat, an anonymous table ended a `preserve-3d` cell's context (WPT css-transforms
  `transform-table-011`). Covered by `anonymous_table_boxes_pass_a_3d_rendering_context_on` in
  `crates/pyro-render/tests/transform_3d.rs`.
## Floats painted in the phases of their stacking context; floats in inline formatting contexts (WPT reftests, lane P3)

- `src/layout/paint_tree.rs` (`Node::paints_in_ancestor_phases`, `Node::is_inline_level`, `is_lifted_float`,
  `NodeFlags::HAS_FLOAT_PAINT_CHILDREN`): a block box of the normal flow that is no layer root, no stacking context,
  no float, no flex or grid item, no multi-column container and no replaced element (anonymous blocks and split
  inlines included) paints in its painter's phases (CSS 2.1 Appendix E: block backgrounds, then floats, then inline
  content). Its floats are lifted out of it, as positioned boxes are, to the nearest box that paints its own phases,
  which lists them at its float level in tree order and is flagged. Floats inside an earlier block painted under a
  later block's background, and blocks in split inlines painted over floats (WPT `stack-floats-001..004`,
  `overhanging-float-paint-order`). An anonymous block's style keeps `display: inline`, which these checks see
  through. blitz-paint paints the phases (`FlowPhase`). Lifted floats are marked (`HoistedPaintChild::is_float`); a
  box that lifted floats (`NodeFlags::LIFTS_FLOATS`) or painted them is checked again before a build keeps its lists,
  as its parent can change whether it lifts them (a block that becomes a flex item) without damaging it.
- `src/layout/inline/layout.rs`: a float placed with nothing before it on its line leaves the line to be retried
  after it: content that doesn't fit beside the float moves down (WPT `floats-placement-004`, `c414-flt-fit-001`,
  `c5525-fltwidth-001`). A float left for below its line but met past the point where the line wrapped is dropped
  there and met again on the next line (`PendingFloat::item`, parley's `BreakerState::item_index`); a float met
  again after its line wrapped before it keeps its place (`placed_through`); before, it was placed twice (WPT
  `floats-line-wrap-shifted-001`, `float-no-content-beside-001`). Relatively positioned inline boxes around a float
  move it (`inline_span_offset`; WPT `block-in-inline-float-between-001`).

Covered by `crates/pyro-render/tests/floats.rs`.

## References to elements of other SVG documents, `mask-mode` on SVG elements (second pass, lane P6)

- `src/node/element.rs` (`ElementData::reference_images`), `src/node/svg.rs` (`reference_candidates`,
  `references_out_of_sync`, `svg_source_element`, `embeddable_svg`, `SvgImageData::reference_document_from_source`),
  `src/layout/damage.rs` (`flush_svg_references_from_style`), `src/stylo.rs` (`needs_style_image_flush`),
  `src/util.rs` (`ImageType::Reference`), `src/document.rs` (`apply_loaded_image`), `src/net.rs`
  (`ImageHandler::parse`): the SVG documents an element's `clip-path` and `filter` (and an SVG element's
  `clip-path`, `filter` and `mask` attributes) reference elements of by `url(document.svg#id)` are fetched as images
  are, one entry per reference, and kept with their source. A reference into the page gets an entry without an
  image (no fetch). A document that is no image usvg accepts (a root of no size, `<svg width="0" height="0">`) is
  parsed with its root filling a viewport and kept for its source. An SVG element's `clip-path` attribute is taken
  as written: the style engine resolves a presentation attribute's URL without the document's base.
- `src/node/svg_css.rs` (`Writer::import_document`, `external_reference_rewrites`, `css_mask_rewrite`): the
  documents an element of an inline `<svg>` has loaded are written into the source's `<defs>` once, and its
  `clip-path`, `filter` and `mask` references name their elements by fragment, which usvg resolves (it loads no
  other document). `mask-image: url(sprite.svg#m)` naming another document's `<mask>` masks by it; before, the
  element was left unmasked. WPT `clip-path-url-reference-external-svg-element`,
  `clip-path-url-reference-data-uri-svg-element`, `svg-external-filter-resource`, `svg-data-uri-filter-resource`.
- `src/node/svg_css.rs` (`Writer::mask_of_type`, `mask_rewrite`): `mask-mode: alpha` or `luminance` on an SVG
  element that references one `<mask>` is used in place of the mask's `mask-type` (CSS Masking 1 §6.2): the mask is
  written again into `<defs>` with that type and the element references the copy (WPT
  `mask-mode-to-mask-type-svg`).

- `src/node/svg_css.rs` (`attribute_boxes`, `Writer::boxes`): an element usvg's tree of the `<svg>` does not hold
  (the content of a `<mask>` or `<clipPath>` only a box of the page references) has the fill and stroke boxes of
  its geometry attributes when it is a `<rect>`, `<circle>` or `<ellipse>`: its `clip-path` basic shape or
  `transform-box` resolved against an empty box (WPT `mask-image-3h`).
- `src/document.rs` (`with_cross_svg_references`), `src/layout/construct.rs`: an inline `<svg>` that references a
  `<filter>`, `<clipPath>`, `<mask>`, gradient, `<pattern>` or `<marker>` of another inline `<svg>` of the page
  (`url(#id)`, a gradient's `href="#id"`) gets a copy of it (and of what that references) in a `<defs>` first in
  its source: ids are the document's, usvg reads each `<svg>` alone. A later change to the definitions does not
  rebuild the `<svg>`s that use them.

Covered by `crates/pyro-render/tests/svg_external_references.rs` and `svg_css_effects.rs`.
## A document whose root element is inline content

`src/layout/construct.rs` (`collect_layout_children_with_wrap`): only a box holds an inline layout. The document
node is the container of the root element, and when that element is inline content (a `<br>` appended to a document
whose element was removed) the document node was made an inline root: the inline layout queued for it had no element
to go to and `resolve_deferred_tasks` panicked (WPT css/css-text/white-space/
pre-line-br-with-whitespace-child-crash.html, css/css-contain/contain-style-remove-element-crash.html). Such a root
element is now the document's layout child, as every other root element. Covered by
`crates/pyro-render/tests/no_root_element.rs`.
## Inline layout leftovers (WPT reftests, lane P5)

- `src/layout/inline/layout.rs` (`inline_containing_block_of`, `span_padding_box`): an out-of-flow box whose
  containing block is a positioned inline box is laid out against the padding box that box's fragments bound (CSS
  2.1 § 10.1 4.1: top and left of its first fragment, bottom and right of its last; the other way round for a
  right-to-left box), moved by the box's relative offset, where the inline root's own padding box was used. The
  root sets it on the candidate (`OofCandidate::containing_block`, taffy) and lays the box out in its own pass. A
  box's static position moves with the relatively positioned inline boxes around it. On a line without content a
  span is at the line's top (a span holding only the out-of-flow box). WPT `abspos-inline-007`/`-008`,
  `toogle-abspos-on-relpos-inline-child`, `slice-inline-fragmentation-001..003`.
- `src/layout/inline/build.rs` (`span_edge_widths`, `span_edge_widths_in`): a side of an inline box is an edge when
  it has a margin, a border or a padding, also when they add up to nothing (a border and a negative margin as wide:
  it paints, its line is no empty line, a block-level out-of-flow box after it starts below; WPT `margin-right-114`,
  `static-inside-inline-002`), and when one is a percentage: each layout gives such an edge its width against the
  root's content width (zero while the root is measured; `TextLayout::percent_span_edges`; WPT `c547-indent-001`).
  A right-to-left span with one edge has the other too, empty: parley places a span's edges around the span's
  content on the line, which must be items of its own there (text is shaped across a span's boundary otherwise;
  WPT `bidi-box-model-011`, `-014`, `-015`, `-029`, `-030`, `-032`).
- `src/layout/inline/layout.rs`: `text-indent` does not indent an anonymous block that is not its parent's first
  box (CSS Text 3 § 8.1; WPT `text-indent-014`).
- `src/layout/mod.rs` (`block_container_passes_percentage_height`, `block_container_direction`): an inline box
  laid out as a block around its blocks is no containing block: percentage heights in it resolve against its
  containing block's height (WPT `block-in-inline-percents-001`, `percent-height-1`), and that block's direction
  decides which margin an over-constrained child drops (WPT `block-in-inline-margins-001b`, `-002b`).
- `src/node/node.rs` (`has_block_level_pseudo_element`): a block-level `::before` or `::after` makes its inline box
  one that contains a block (read from the pseudo-element's style: its box is made after the container asks), so
  the box is split around it (WPT `block-in-inline-insert-013`, `-014`, `-016a`, `-016b`). `src/document.rs`
  (`deallocate_anonymous_block`): the boxes of a split inline's head or tail keep no pointer to the freed anonymous
  block that laid them out (they are the split inline's children, not the block's).
- `src/layout/construct.rs`: an absolutely positioned box that would be inline-level joins the anonymous block of
  the inline content it follows, as a float does: its static position is on that line (WPT
  `remove-block-between-inline-and-abspos`).
- `src/layout/mod.rs`, `src/node/node.rs` (`NodeFlags::FLOW_BOXES_OVERLAP`, `FLOW_CONTENT_OVERFLOWS`): layout
  records where in-flow block boxes overlap in the block axis (a negative margin, or a box after one whose content
  is taller than it), for the box that paints them (taffy's `set_block_flow_overlap`; blitz-paint paints such
  children in phases).
- `src/stylo.rs` (`get_attr`): `attr(Title)` names the `title` attribute of an HTML element in an HTML document
  (WPT `content-attr-case-001`).

Covered by `crates/pyro-render/tests/inline_leftovers.rs`.

## `text-box-trim`, `<br>` clearance (WPT round 3, lane I)

- `src/layout/text_box.rs` (new), `src/layout/inline/layout.rs`: `text-box-trim` / `text-box-edge` on block
  containers. The inline root holding a block's first or last formatted line (found up the chain of first / last
  in-flow children, stopped by a child with a border or padding on that side, as in Chromium) moves its lines up
  by what the first line loses (`TextLayout::block_offset`) and is shorter by both trims; the metric is the
  `text-box-edge` of the line's own root inline box. `vertical-lr` lines are inverted: their under side is at
  the block start. On inline boxes the trim goes to parley (`TextStyle::text_box_trim`). Gated by the sticky
  `BaseDocument::text_box_trim_styles` (set by the style traversal), so other documents pay nothing.
  `layout.css.text-box.enabled` is set in `init_stylo_prefs` (and in pyro-dom's list).
- `src/layout/inline/build.rs`, `layout.rs`: a `<br>` with `clear` (`TextLayout::br_clears`) starts the next
  line below the floats it clears, and a block ending with it ends there; a trimmed last line does not end the
  block above the float beside it (WPT `text-box-trim-float-clear-br-*`).
- `src/layout/inline/build.rs`, `layout.rs`: an inline box with a border or padding over or under it
  (`TextLayout::span_block_edges`), and trimmed lines, set `FLOW_CONTENT_OVERFLOWS`: the box reaches over the
  next block, whose background is painted before it (Appendix E phases).
- `src/layout/inline/relative.rs` (`inline_span_flow_offset`): the relative offset of inline boxes in the axes of
  the layout, so static positions and span containing blocks move with them in vertical writing modes too (paint
  moves the text there since this change). Not in a multi-column container of a vertical writing mode, which is
  laid out as one column: `css-position/multicol/static-position/v*-in-multicol` would show what the offset moves
  out of a column.

Covered by `crates/pyro-render/tests/text_box_trim.rs`.
## Sticky offsets (WPT reftests, round 3 lane S)

- `src/node/element.rs`, `src/node/node.rs`: `LayoutData::sticky_offset` with `Node::sticky_offset` and
  `Node::set_sticky_offset`: how far a `position: sticky` box is from where it is laid out (CSS px). The layout
  places a sticky box as if static; the embedder knows the scroll positions, computes the offset (pyro-dom
  `Layout::sticky_offsets`) and sets it through pyro-render's mirror (`Snapshot::sticky_offsets`). blitz-paint
  moves the box by it (its README.pyro.md).

Covered by `crates/pyro-render/tests/sticky_paint.rs`.

## `aspect-ratio: auto <ratio>` on replaced elements (round 3 lane S)

- `src/layout/replaced.rs` (`has_natural_ratio`), `src/node/node.rs` (`layout_style`): a replaced element with a
  natural aspect ratio (a loaded raster image, an SVG image with a natural size or a `viewBox`, a canvas) and
  `aspect-ratio: auto <ratio>` is flagged `StyleFlags::HAS_NATURAL_RATIO`, and stylo_taffy then reports no
  preferred ratio: the natural one applies, as css-sizing-4 §5.1 has it (the `<ratio>` applied before, so a
  1x1 image with `width: 100px; aspect-ratio: auto 10/1` was 10px tall). Without a natural ratio (an image not
  loaded yet) the `<ratio>` still applies.

Covered by WPT `css/css-sizing/aspect-ratio/replaced-element-029`, `-030` and `grid-aspect-ratio-005`, `-006`.
- `src/layout/inline/build.rs` (`build_inline_layout_with`), `src/stylo_to_parley.rs`: preserved tabs go to tab stops
  (parley's README.pyro.md, "Tab stops"). Each style carries its `tab-size`; an inline root whose text has a tab sets
  the stops' unit, the advance of a space in the root's first available font plus the root's letter and word
  spacing, and the least tab advance, 0.5ch of that font, before its content widths are measured. Covered by
  `crates/pyro-render/tests/tab_stops.rs`.

- `src/stylo_to_parley.rs`, `src/document.rs` (`init_stylo_prefs`: `layout.css.hyphens.enabled`): `hyphens` and
  `hyphenate-character` are parsed and reach parley as each style's `SoftHyphen` (parley's README.pyro.md, "Soft
  hyphens"): `hyphens: none` lets no line end at a soft hyphen, `hyphenate-character` names what a line ending at one
  shows (the empty string nothing, one character that character, `auto` and longer strings the font's hyphen).
  `hyphens: auto` breaks only at soft hyphens, as `manual`. Covered by `crates/pyro-render/tests/soft_hyphens.rs`.

- `src/layout/inline/layout.rs` (`balanced_wrap_limit`), `src/document.rs` (the `text-wrap-style: balance` pref):
  `text-wrap-style: balance`. A block of two to six lines (Blink's limit; fewer when `line-clamp` cuts it) with no
  float beside it and no `max-height` (a block-size clamp, which nothing applies yet, would cut it) wraps at the least advance that keeps its line count, found by bisection over trial breaks; a
  trial with more emergency breaks (`overflow-wrap`) or more overflowing lines than the unbalanced break is no
  balance. The lines keep the container's width for alignment (parley's `BreakerState::set_wrap_limit`).
  Covered by `crates/pyro-render/tests/text_wrap_balance.rs`.


- `Cargo.toml` (parley's `complex-scripts` feature), `src/stylo_to_parley.rs`: lines break inside Thai, Lao, Khmer and
  Burmese text where ICU4X's dictionaries find word boundaries (`LineSegmenter::new_dictionary`; +3.8 MB of data in
  the binary); they broke only at spaces. `word-break: manual` (stylo parses it for this engine) turns that off for
  its text (parley's `TextStyle::dictionary_breaks`, parley_engine's `LineBreakConfig::no_dictionary`). WPT css-text
  `line-breaking-024..027`, `word-break-normal-{th,km,lo,my}-*`, `word-break-manual-001`.
## Size containment and `contain-intrinsic-size` (round 3 lane S)

- `src/layout/mod.rs` (`compute_child_layout_in_current_wm`, `size_contained_size`): a box with size containment
  (`contain: size` or `strict`; css-contain-2 §3.1) is sized as an empty box whose content is its
  `contain-intrinsic-size` (css-sizing-4 §4.1: `none` is zero, `auto <length>` the length, as no size is
  remembered): Taffy's leaf layout, with the box's own style, gives that size, and the box's algorithm then lays
  its content out with both dimensions known. A replaced element keeps its natural aspect ratio
  (`replaced::natural_ratio`): one size gives the other (WPT css-flexbox `canvas-contain-size`). `contain: size` did nothing before. Not for non-atomic inline
  boxes, tables and internal table boxes.
- `src/layout/mod.rs` (`inline_size_contained_width`): `contain: inline-size` alone (css-contain-2 §3.2): a box
  whose width its parent leaves to it (shrink-to-fit, or measured for its min-content or max-content contribution)
  is as wide as an empty box whose content is `contain-intrinsic-width` (Taffy's leaf layout with the box's own
  style); its content is then laid out at that width and gives its height. A box given its width (a block in flow,
  a stretched item) keeps it. Danex's list cells (Tailwind's `[contain:inline-size]` on a truncating line in a
  column `inline-flex`, Pyro 0.0.38 fault 5) took the line's whole text width (320 px, up to `max-width`) where
  Chrome takes none (264 px, the cell's `::after` minimum). Horizontal non-replaced boxes only: vertical writing
  modes and replaced elements are left as they were (WPT `contain-intrinsic-size-028.html`, testharness: 4 of 48,
  its horizontal boxes).

Covered by `crates/pyro-render/tests/size_containment.rs` and `inline_size_containment.rs`; WPT
`css/css-sizing/contain-intrinsic-size`: 23 more reftests pass.
## `image-set()`, images as `content`, font features and descriptors (WPT css-images and css-fonts, round 3 lane F)

- `src/util.rs` (`unfiltered_image`, `image_resolution`): an `image-set()` layer loads and draws the option
  Stylo selected (backgrounds, masks, border images, list markers, `content`), at that option's resolution.
- `src/layout/damage.rs` (`flush_content_image`), `src/layout/replaced.rs` (`is_content_image`),
  `src/node/element.rs` (`ElementData::content_image`): an element whose own `content` is one image is laid out
  as an `<img>` of it once the image has loaded (CSS Content 3 §2). A generated `img` box (`content: url()` of
  `::before`/`::after`) carries the resolution too (`Piece::Image`), and `layout/mod.rs` divides the raster's
  pixel size by it.
- `src/layout/mod.rs`: the `natural=W,H` token (pyro-dom's density-corrected natural size: a `srcset`
  candidate's `0.5x`) sizes a loaded raster `<img>` too; it was only used until Blitz had the image.
- `src/layout/list.rs`, `src/layout/inline/build.rs`, `src/layout/construct.rs`: an image marker inside the
  item (`list-style-position: inside`) has room at the start of the first line: a run of no size of its own, as
  tall as the image above the baseline and as wide as it plus Blink's 7px (blitz-paint draws the image). An
  item with nothing but that marker has the line.
- `src/stylo_to_parley.rs` (`font_kerning`, `font_variant_alternates`, `set_font_feature_values`),
  `src/resolve.rs` (`publish_font_feature_values`): `font-kerning: none | normal` is the `kern` feature;
  `font-variant-alternates` maps to `hist`, `salt`, `ssNN`, `cvNN`, `swsh`/`cswh`, `ornm` and `nalt` through the
  `@font-feature-values` rules for a family of the element's `font-family` (a thread-local the layout sets: the
  style conversions have no document). In `font_features` the last setting of a tag wins
  (`font-feature-settings` over `font-variant-*`); `dedup_by_key` kept the first.
- `src/net.rs` (`FontFaceOverrides::width`), `src/document.rs`: the `font-stretch` descriptor of `@font-face`
  reaches font matching (the faces of a family that differ in it only were all `normal`).
- `src/document.rs` (`init_stylo_prefs`): `light-dark()` of images and the `font-synthesis-*` longhands parse.

Covered by `crates/pyro-render/tests/gradient_stops_image_set.rs` and the WPT pages named in the commit.

- `src/util.rs` (`object_view_box`), `src/layout/mod.rs`: `object-view-box` (a rectangle of the content's natural
  size, `inset()`, `rect()` or `xywh()`): the view box is the content's natural size; one without an area is
  ignored.
- `src/net.rs`: a decoded raster image is turned as its EXIF orientation says (`image-orientation: from-image`,
  the initial value), in JPEG, WebP and PNG (`eXIf`); pyro-dom's header parser exchanges the sides to match.
- `src/mutator.rs`: an `<embed>` loads the image its `src` names, as an `<img>` does.

Covered by `crates/pyro-render/tests/object_view_box_embed.rs`.

## `::marker` (WPT round 3, lane I)

- `src/layout/list.rs`: a list item's marker takes its `::marker` style. The style (lazily cascaded, only when
  a rule matches) is kept on a node of its own (`ElementData::marker_style`, `ListItemLayout::style_node`), the
  brush of the marker's text, so paint reads its colour, font and shadows; inside markers are a span of it
  (`inline/build.rs`). `content`: `none` leaves no marker, strings, `counter(list-item)` and quotes replace it
  (other counters and images keep the `list-style` marker). The outside marker's line preserves its white space
  (the suffix space of `1. ` takes room, as `white-space: pre` in the UA sheet gives it).
- `src/stylo.rs`, `src/layout/list.rs` (`item_restyled`), `src/layout/counters.rs`: a restyled list item with
  a marker style node refreshes it (the marker inherits from the item); a changed `content` rebuilds the list's
  markers.
- `src/stylo.rs` (`implemented_pseudo_element`, `get_attr`): the box of a `::before` / `::after` is that
  pseudo-element for selector matching, so `::after::marker` rules reach the marker of a pseudo-element's list
  item, and `attr()` there reads the originating element.
- `src/animation.rs`, `src/node/element.rs`: `ElementEffects::marker`: the animations in effect on an element's
  `::marker` (`Element.animate` with `pseudoElement: '::marker'`) are sampled like the element's own and
  cascaded onto the marker's style (`ElementData::sampled_marker`, `Stylist::compute_for_declarations`).

Covered by `crates/pyro-render/tests/marker_pseudo.rs`.
## `font-size-adjust` (WPT css-fonts, round 3 lane F)

- `src/resolve.rs` (`publish_font_size_adjustments`), `src/stylo_to_parley.rs` (`font_size_adjustment`),
  `src/font_metrics.rs` (`metrics_for`): the used font size of a style with `font-size-adjust` is its font size
  times the value over the font's own ratio of the named metric (`ex-height`, `cap-height`, `ch-width`,
  `ic-width`, `ic-height`) to its size. The font is the one of the list that draws `x` (its `unicode-range` and
  character map), taken to draw the run; a fallback font used for other characters is not scaled on its own
  (parley sizes a run by its style, not by its font). The factors are computed before layout for every style
  with the property (one walk over the nodes, a pointer comparison each where the property is unset), by font
  style struct, into a thread-local the style conversions read: they have no document, and the layout holds
  the font context's lock. A `line-height` number keeps multiplying the unscaled size.

Covered by `crates/pyro-render/tests/font_size_adjust.rs`.

## `font-variant-caps` values, the `font-style` descriptor's range (WPT css-fonts, round 3 lane F)

- `src/document.rs` (`init_stylo_prefs`): `font-variant-caps: all-small-caps | petite-caps | all-petite-caps |
  unicase | titling-caps` parse (`stylo_to_parley::font_variant_caps` already mapped them to features).
- `src/net.rs` (`FontFaceOverrides::slant_range`, `stylo_to_fontique_style`): a `font-style` descriptor range
  that has 0deg in it (`oblique 0deg 15deg`, `oblique -15deg 15deg`) is an upright face whose `slnt` axis may
  go that far (vendored fontique clamps to it); `normal` forbids the axis.

## Phantom line boxes (WPT round 3, lane I)

- `src/layout/inline/layout.rs`: a line holding an inline box's inline-axis margin, border or padding (a span
  edge, also of zero width, as `span_edge_widths` makes them) is no phantom line box (CSS Inline 3 § 2.1,
  csswg-drafts#9344): margins do not collapse through its block, whatever its height. Block-axis sides alone
  leave the line phantom.

Covered by `crates/pyro-render/tests/phantom_line_boxes.rs`.

## Initial letters (WPT round 3, lane I)

- `src/layout/first_letter.rs`: an `initial-letter` on a `::first-letter` makes its style node the letter's box
  (`NodeFlags::IS_INITIAL_LETTER_BOX`): a copy of the letter in a text node of its own, the `::first-letter`
  style with the font size whose cap height spans `size` lines (the paragraph's line height and cap height,
  Blink's `ComputeInitialLetterFont`), the line height of that font, and a float to the start of the line.
  `FirstLetterSplit::initial` carries what the first line needs (`node::InitialLetter`: Blink's legacy sink,
  the paragraph's line height and descent under the baseline with its half-leading). A floated `::first-letter`
  with an `initial-letter` is one. `src/resolve.rs` builds the box's own inline layout and makes it the first of
  its root's layout children before the root's build; dropping the style node drops the text (`src/document.rs`).
- `src/layout/inline/build.rs`: the root's text leaves the letter out and holds the box in its place
  (`InlineBoxKind::CustomOutOfFlow`).
- `src/layout/inline/layout.rs`: `compute_initial_letter_box_layout`: the box is as wide as its text indent and
  the ink of its glyphs and as tall as the ink (`glyph_ink_bounds`: outline bounding boxes, rounded out to layout
  units), the ink's corner at its content box's. The first line places it (`initial_letter_offset`, Blink's
  `PostPlaceInitialLetterBox`): after the line's text indent (the line's text is not indented again), beside the
  floats along its whole height, its cap height on the line's (centred on its lines in vertical modes without
  sideways glyphs), an exclusion from the line's top to its margin box's bottom; raised and sunken letters move
  the line's text down (not under `text-box-trim` at the start). A block with an initial letter starts below the
  earlier ones.
- `src/font_metrics.rs`: `BlitzFontMetricsProvider::extents_for` (ascent, descent, line gap, cap height).

Covered by `crates/pyro-render/tests/initial_letter.rs`.

## Selection ends and declared highlight colours (WPT round 3, lane I)

- `src/dom_selection.rs` (`produced_offset`): the white space a text node's text starts with in its inline layout
  that the node before left to it (trailing collapsible white space belongs to the next text) has no counterpart in
  its content: offsets skip it, so a selection ending at the end of `<span>` text after a space keeps its last
  letter (and one starting at the span's text leaves the space out).
- `src/dom_selection.rs` (`BaseDocument::style_declares`): whether author or user rules declare some longhands of a
  style (the highest-priority declaration of each: `revert` declares nothing, `revert-layer` defers), read from
  the style's rule nodes; blitz-paint's highlight asks it of `::selection` styles.

Covered by `crates/pyro-render/tests/selection_highlight.rs` and the `dom_selection` unit tests.

## The `<body>` whose `overflow` the viewport takes (WPT round 3, lane G)

`src/resolve.rs` (`mark_viewport_body`, before each layout pass), `src/node/node.rs` (`NodeFlags::OVERFLOW_TO_VIEWPORT`,
`Node::layout_style`): the `<body>` whose `overflow` goes to the viewport (`body_overflow_goes_to_viewport`: the root's
is `visible`, neither has containment) is laid out with `overflow: visible` (stylo_taffy's
`StyleFlags::OVERFLOW_TO_VIEWPORT`). `body { overflow: hidden }` made it a scroll container: a block formatting context
root whose first child's top margin no longer collapsed with its own (8px lower than in browsers; WPT
`css/css-grid/layout-algorithm/grid-as-flex-item-should-not-shrink-to-fit-*`). A body whose flag changes is laid out
again with the root. Paint and scrolling already treated such a body's `overflow` as the viewport's. Covered by
`crates/pyro-render/tests/viewport_overflow.rs`.
Only the root's first `body` child (a `frameset` before it does not count) gives the viewport its `overflow`, and
only while it generates a box (`body_overflow_goes_to_viewport`, lane SM: a second `body` keeps its own; WPT
`css/css-overflow/overflow-body-propagation-014..016`).

## `::marker` text-transform and tabular digits (WPT round 3, lane I)

- `src/layout/list.rs`, `src/layout/text_transform.rs` (`TextTransformer::transform_alone`): a `::marker` rule's
  `text-transform` transforms the marker's string (inside and outside); the item's own does not reach it (Blink's UA
  sheet: `::marker { text-transform: none }`), read with `BaseDocument::style_declares`.
- `src/layout/list.rs`, `src/layout/inline/build.rs`, `src/node/element.rs` (`ListItemLayout::tabular_nums`),
  `src/stylo_to_parley.rs` (`with_tabular_nums`): marker digits are tabular (`tnum`, Blink's UA `::marker {
  font-variant-numeric: tabular-nums }`) unless a `::marker` rule declares `font-variant-numeric`.

Covered by `crates/pyro-render/tests/marker_pseudo.rs`.


## Subgrids (WPT round 3, lane G)

`src/document.rs`: Stylo's `layout.css.grid-template-subgrid-value.enabled` is on (and in pyro-dom's styles), so
`grid-template-*: subgrid` parses. `src/node/element.rs` (`LayoutData::subgrid`), `src/layout/mod.rs`
(`LayoutGridContainer::subgrid_context`/`set_subgrid_context`): the context a grid gives a subgrid child (vendor/taffy's
README.pyro.md, "Subgrids") is kept on the child; a new one drops the child's cached layouts, and one is read only while
the child's layout parent is a grid.
## `font-palette` (WPT css-fonts, round 3 lane F)

- `src/document.rs` (`init_stylo_prefs`): `layout.css.font-palette.enabled` (the `font-palette` property and the
  `@font-palette-values` rule). `BaseDocument::font_palette_values_rules` gives blitz-paint the rules in cascade
  order, `BaseDocument::font_face_families` the `@font-face` family a font file was registered under (recorded in
  `load_resource`).
- `src/util.rs` (`intern_font_blob`): a font file is one blob per `@font-face` family name it is loaded under, so a
  run's font tells which family drew it (two rules loading one file under two names: WPT `font-palette-31`).

## `@font-face` `size-adjust` and metrics overrides (WPT css-fonts, round 3 lane F)

- `src/net.rs` (`FontFaceOverrides::face_metrics`), `src/document.rs` (`load_resource`): the descriptors are
  registered with the face (fontique `FaceMetrics`); parley_engine draws and measures the face with them.
- `src/stylo_to_parley.rs`: `TextStyle::font_size_adjusted` where `font-size-adjust` is set (a face's
  `size-adjust` is then not applied). `src/font_metrics.rs` (`metrics_for`): `ex`, `ch`, `ic` and `cap` lengths
  measure the face at its `size-adjust`, with its ascent override, except under `font-size-adjust`.

Covered by `crates/pyro-render/tests/font_face_metrics.rs`.

## Container units (WPT css-images, round 3 lane F)

`src/resolve.rs` (`resolve`): the container sizes Stylo queried are kept, and a change of one after layout restyles
the document, also where no `@container` rule exists but styles use container units (`cqw`, ...;
`ComputedValueFlags::USES_CONTAINER_UNITS`): they were dropped as soon as the stylist had no container
condition. Covered by `crates/pyro-render/tests/container_queries.rs`
(`container_units_resolve_against_the_size_container`).

## `@font-face` `font-feature-settings` and `font-variation-settings`, `font-language-override` (WPT css-fonts, round 3 lane F)

- `src/net.rs` (`FontFaceOverrides::face_settings`), `src/document.rs`: the descriptors are interned
  (`fontique::FaceSettings`) and registered with the face.
- `src/stylo_to_parley.rs`: `TextStyle::auto_optical_size` (the automatic `opsz` leads the variations,
  `auto_optical_size`) and `TextStyle::language_override` from `font-language-override`.

Covered by `crates/pyro-render/tests/font_face_settings.rs`.
- `src/net.rs` (`FontFaceOverrides::weight_range`, `width_range`): the `font-weight` and `font-stretch`
  descriptors' ranges go to fontique (its README, "Font matching with `@font-face` ranges"); `weight` is the
  lower end of a reversed range too.

## `font-size-adjust` and font-relative units (WPT css-fonts, round 3 lane F)

`src/font_metrics.rs` (`font_size_adjust_scale`, `query_font_metrics`), `src/resolve.rs`
(`publish_font_size_adjustments`): under `font-size-adjust`, `ex`, `ch`, `ic` and `cap` measure the first available
font at the adjusted size (`em` stays the computed size); a font without the named metric takes the spec's
fallback (0.5em for `ex-height` and `ch-width`, 1em for `ic-*`, the ascent for `cap-height`): it was left
unscaled. WPT `font-size-adjust-units-001`, `-order-001`, `-011`.

## SVG images: a declared width or height is the natural one; zero sizes (WPT round 3, lane G)

`src/node/svg.rs`: an SVG image's natural width and height are its root's absolute `width` and `height` as declared
(`absolute_length_px`: px, in, cm, mm, pt, pc; em/ex and percentages as before). usvg's resolved size stood in, and
without a `height` usvg took the content's bounds: `width="20px"` with a circle sized in percentages was 46px wide (WPT
`grid-item-no-aspect-ratio-stretch-2/3`). A root `width` or `height` of zero is a natural size of zero, not an invalid
image (usvg refuses it: `InvalidSize`): the image keeps its declared dimensions and `viewBox` ratio, and its content is
parsed as of an SVG without a size, drawn in the box it is given, as in browsers (`grid-item-aspect-ratio-stretch-3`).
Covered by `crates/pyro-render/tests/svg_image_natural_size.rs`.

`src/node/node.rs` (`Node::layout_style`): a replaced element with a natural aspect ratio gives it to its layout style
(stylo_taffy's `natural_ratio`), for Taffy's flex algorithm (vendor/taffy/README.pyro.md, "Natural aspect ratios of
flex items").
`src/stylo_to_parley.rs` (`is_hidden_family`), `src/font_metrics.rs`: a family name that starts with a dot (macOS's
hidden UI fonts, `.AppleSystemUIFont`, `.SF NS Mono`) matches no font, as in Safari and Chrome (WPT
`standard-font-family`).
## `text-spacing-trim`, `text-autospace` (WPT round 3, lane TS)

- `src/stylo_to_parley.rs` (`text_spacing_trim`, `text_autospace`): the two CSS Text 4 properties become parley's
  `TextStyle::text_spacing_trim` and `text_autospace`. Font features that set fullwidth punctuation widths themselves
  (`halt`, `hwid`, `palt`, `pwid`, `qwid`, `twid`, `valt`, `vhal`, `vpal`, as Blink's `IsExclusiveFeature`;
  `font-variant-east-asian: proportional-width` is `pwid`) turn trimming off. Upright text in a vertical writing mode
  (`text-orientation: upright`, `text-combine-upright: all`) gets no autospace, as in Blink.
- `src/document.rs`: the `layout.css.text-autospace.enabled` and `layout.css.text-combine-upright.enabled` prefs (the
  layout ignores `text-combine-upright`; autospace reads it).
- `src/layout/inline/build.rs` (`span_edge_widths_in`, `span_edge_margins`), `src/layout/inline/layout.rs`: in a
  vertical writing mode an inline box's inline-axis margins (top and bottom, by the inline direction) are span edges;
  they were dropped with the borders and padding, which stay out (their sides are not painted in turned lines). The
  geometry and blitz-paint's fragments take the same logical margins. WPT `text-autospace-004` (`sideways-lr`)
  passed only while margins and autospace were both missing.

Covered by `crates/pyro-render/tests/text_spacing.rs`.
## Counter inheritance, `@counter-style` markers (WPT round 3, lane CB)

- `src/layout/counters.rs` (`Walk::reset`): counters are inherited as CSS Lists 3 § 4.4.1 has it (Chromium,
  WebKit): a `counter-reset` on an element whose parent already has a counter of that name makes a counter of the
  element and its descendants alone; the element's following siblings keep the parent's (WPT `counter-001`,
  `counters-001`). Without one on the parent, the following siblings inherit the new counter, as before. Counters a
  `::before` or `::after` makes for itself end with it. The `list-item` counter an HTML list (`ol`, `ul`, `menu`,
  `dir`) resets keeps the CSS 2.1 scope (items after a list nested in a list continue the nested one: WPT
  `counter-list-item-2`). The children of a `display: contents` element count in its parent's scope, as boxes of the
  parent (WPT `counter-order-display-contents`), and an increment past the integer range leaves the counter as it
  was (Blink's `CalculateCounterValue`).
- `src/layout/list.rs` (`custom_counter_marker`): the marker of a `list-style-type` naming an `@counter-style` rule
  with `system: extends <style>` is the item's number in that style with the rule's `prefix` and `suffix` (the
  extended style's when absent), as text in the item's font (WPT `marker-text-matches-*`). Other systems are not
  supported.

`src/stylo_to_parley.rs`: CSS `text-justify` becomes parley's `TextStyle::text_justify` (`distribute` parses as
`inter-character`).
  supported. A predefined style the marker code has no case of (`armenian`, `georgian`, `lower-greek`, ...) is the
  text `counter()` gives with a `. ` suffix (it was a `□`); an unknown name is `decimal`.

## Static positions, sticky inline boxes, positioned table parts (WPT css-position, round 3 lane P)

- `src/layout/inline/layout.rs`: an inline-level absolutely positioned box is put on the start side of its place in
  the direction of the text there (parley's `PositionedInlineBox::is_rtl`), a block-level one in the container's.
- `src/layout/mod.rs` (`static_position_is_containing_block`): an element in the top layer has the initial
  containing block as its static-position rectangle.
- `src/layout/inline/relative.rs`, `build.rs`: a sticky inline box (not atomic) is painted moved by its sticky offset
  (`Node::sticky_offset`), as a relatively positioned one is by its relative offset (`inline_span_relative_offset`).
- `src/layout/table.rs` (`table_cell_relative_offset`, `table_part_relative_offset`): relative positioning of a row
  and a row group moves their cells (rows and row groups have no boxes); a percentage `top`/`bottom` of a cell, row
  or row group resolves against the length height its parent is given, else is `auto` (Chromium; WPT
  `position-relative-010` to `-013`).
- `src/layout/paint_tree.rs`: a cell of a relatively positioned or sticky row or row group paints as a positioned
  box (CSS 2.1 Appendix E step 8), with the part's background under it (blitz-paint).
- `src/layout/mod.rs` (`table_part_containing_block`): an absolutely positioned box whose containing block is a
  positioned row or row group is laid out by the table against the part's box moved by its relative offset; a
  relatively positioned caption moves.
Covered by `crates/pyro-render/tests/static_position.rs` and `table_part_positioning.rs`.
## Scroll markers and scroll buttons (WPT round 3, lane SM)

CSS Overflow 5's `::scroll-marker-group`, `::scroll-marker`, `::scroll-button()` and `:target-current`, as
Chromium generates and lays them out (`src/layout/scroll_markers.rs`; the stylo side is in its README):

- The boxes are nodes outside the DOM (as `::before`), on `ElementData::scroll` (`ScrollPseudos`): a scroll
  container's group, an element's buttons and marker, and what pseudo-element a box node is with its originating
  element. `BaseDocument::resolve_scroll_markers` (after a style traversal or a tree change, before counters, on
  pages that use the feature) walks the document: a scroll container whose `scroll-marker-group` names a side has a
  group; the elements whose nearest scroll container through their containing blocks has a group, and whose
  `::scroll-marker` has `content`, have markers, its children in tree order; an element whose `::scroll-button()`
  has `content` has a button per direction. Styles come from `Stylist::compute_pseudo_node_style` on the box node:
  `implemented_pseudo_element`, `match_pseudo_element` (a button matches the arguments of its direction, physical
  ones through the element's writing mode), `pseudo_element_originating_element` and `get_attr` answer for the
  box node (`src/stylo.rs`). A button node is a `button` element (UA rules, `IS_BUTTON` layout).
- Box construction (`src/layout/construct.rs`, `src/layout/inline/build.rs`): children are iterated with the
  boxes generated beside them (`with_box_siblings`): a group before or after its scroll container's box, the
  buttons after it (before an `after` group; Chromium's order, WPT `scroll-buttons-layout-parent`); the root
  element's are its first and last boxes. Classification, anonymous blocks, `display: contents` hoisting and inline
  formatting contexts take them as children, once: an anonymous block wrapping a run of its container's boxes
  (`NodeFlags::WRAPS_BOX_RUN`) places none again (they were placed by its container's walk, in it or beside it; a
  block-level group beside an inline-block scroller was wrapped again without end). A non-replaced inline box is
  no scroll container (no group, no buttons: `overflow` does not apply to it). A marker's or button's box holds what its `content` generates
  (`flush_pseudo_elements`); counters see the markers in their group and the buttons, in the scope of their
  originating element (`src/layout/counters.rs`). The damage walks visit the boxes an element owns
  (`src/layout/damage.rs`); a dropped element drops them, and its marker leaves its group (`src/document.rs`). A
  dropped box drops only the children still its own (a marker can move to another group in the same pass) and
  leaves every list that holds it (its group's children, a wrapping anonymous block's children, layout and paint
  children: `detach_from_box_lists`); a moved marker leaves its old group's list. A dangling id there panicked
  the damage walk (WPT `scroll-marker-group-scroll-container-change-crash`, `-body-overflow-propagation-*`).
- `::column::scroll-marker`: a multi-column scroll container with a group has a `::column` node (for selector
  matching only) and a marker per column of its last layout; `update_target_current` builds them again when the
  count changes.
- After layout (`src/resolve.rs`), `update_target_current` picks each group's active marker (CSS Overflow 5
  § 3.1.8's example algorithm as Chromium selects: the nearest scroll container the targets share, then by block
  and inline axis, targets past the reachable range spread over the first and last eighth of a scrollport; a scroll
  container's current scroll target, `set_scroll_targets`, decides first) and sets `:target-current` on its marker
  (`:target-before` on the markers before it, `:target-after` on those after it),
  on the links of `scroll-target-group: auto` elements, and `:enabled`/`:disabled` on buttons (whether the element
  can scroll that way); a change runs another style and layout pass.
- `src/layout/paint_tree.rs` (`restore_out_of_flow_stacking_owners`): a group's or button's box belongs to the
  stacking contexts of its originating element's parent (`scroll_markers::box_parent`), not of the element (an
  absolutely positioned group of a fixed scroll container was painted at the container, WPT
  `scroll-marker-group-019`).
- `assets/default.css`: buttons (`button`, `input` of type `button`, `submit`, `reset`) size their border box, as
  Chromium's (`<button style="width:100px">` was 114px wide; a styled `::scroll-button()` too).
- `src/layout/multicol.rs`, `src/layout/mod.rs` (`multicol_percent_height`): a multi-column container's content
  resolves percentage heights against its column height (the strip was laid out with none: `height: 80%` items were
  auto; WPT `targeted-column-scroll-marker-selection-001`).

Covered by `crates/pyro-render/tests/scroll_markers.rs` and `incremental_construction.rs`
(`scroll_markers_moving_between_groups`).

## A table's `max-width` does not narrow it below its columns (WPT round 3, lane G)

`src/layout/table.rs` (`raise_max_width_to_columns`): the table grid's `max-width` is raised to its columns' min-content
widths (spacing, padding and borders in), as its `min-width` already was when its width was smaller: a width its flex or
grid parent gave it beyond `max-width` was clamped back to it, narrower than its columns (WPT
`table-as-item-auto-min-width`, `table-as-item-fixed-min-width`; vendor/taffy's README.pyro.md, "Tables").

`src/layout/mod.rs` (`compute_table_root_layout`): a height a flex or grid parent gives a table with captions is its
wrapper box's (css-flexbox-1 §4.1): the grid is laid out again with what the captions leave (the table's own `height`,
recognised by its value, is its grid's as in block flow). A stretched table with captions was taller than its line by
them (WPT `table-as-item-stretch-cross-size*`, `table-as-item-flex-cross-size`).

`src/layout/table.rs` (`is_inside_a_cell`): column percentages are not allowed while a table is measured (an available
width that is not definite) inside a flex or grid container either, not only inside a cell, as Blink's
`TableNode::AllowColumnPercentages`: a table with a 100% column measured there was `f32::MAX / 4` wide (WPT
`table-with-infinite-max-intrinsic-width`, `item-with-table-with-infinite-max-intrinsic-width`).

## A measuring run that leaves an inline root's lines unlike its final layout (Danex triage)

`src/layout/inline/layout.rs`, `src/layout/mod.rs` (`LayoutPassState::mark_final_layout_stale`, `cache_get`,
`cache_store`), `src/node/text.rs` (`TextLayout::final_lines`, `LaidOutLines`, `line_geometry`,
`inline_box_sizes`), `src/node/node.rs` (`NodeFlags::FINAL_LAYOUT_STALE`): an inline root has one parley layout, and
every run that is not a horizontal measurement breaks and aligns its lines again, a vertical measurement too. Taffy
measures a stretched column flex item at the available cross size and then lays it out at its container's, and a
table cell or flex item is measured before it is laid out. A later final layout of the item then came from the cache
and left the lines of the measurement: Danex's triage actions (a column `inline-flex` in a `text-align: right` cell)
were right-aligned to the cell's width inside a 51 px item, 93 px past the table.

Each final layout now notes where it left the lines (`LaidOutLines`: break key, `block_offset`, line box top, a hash
of the inline boxes' sizes). A measuring run that leaves them elsewhere marks the root `FINAL_LAYOUT_STALE`, and
with it the boxes around it that are being laid out (`IN_LAYOUT`, set while `compute_child_layout_internal` runs;
up the layout parents to one that is not): a table measures a cell for its columns after an earlier step laid the
cell's `inline-flex` out, and the cell's later final layout would get that `inline-flex` from the cache. `cache_get`
refuses a marked box a cached final layout, and the final layout that runs again (`cache_store`) clears the mark.
Boxes not being laid out are not marked: marking all layout parents made the ancestors of an absolutely positioned
box laid out alone run again in the next pass, which was then no longer free of layout for a paint-only restyle
(`mirror.rs` `paint_only_restyles_run_no_layout_and_match_fresh_builds`). Roots still marked when the root layout
returns mark the boxes they are in, and `resolve_layout` runs the root layout once more, which reaches only them
(`mark_stale_lines_up`). "Elsewhere": another `block_offset`
or line box top (a cell's `vertical-align` measured in another height), other inline box sizes, or lines broken with
other inputs whose geometry differs: their text, alignment offsets, starts, advances and block extents
(`line_geometry`; the line's end only in right-to-left and justified text). That hash is taken lazily, of the final
layout's lines when a measuring run is about to break them again, and Fx-mixed: hashing every final layout with
SipHash cost 1.5% of a relayout. Start-aligned text broken again at another width into the same lines marks nothing.
Lines broken beside floats are not compared (nor are the floats' places): a measuring run marks them. Cost, in
instructions of 41 relayouts (minimum of 2-4 runs, JSC tiering spreads runs of one binary by up to 2%): a 400-row
table whose 2,000 cells all hold the Danex cell +1.0%, 1,500 right-aligned column `inline-flex` flex items +1.2%, a
1,200-row table with one changing cell +0.7%. Covered by `crates/pyro-render/tests/text_align_after_measure.rs`.
## `visibility: collapse` on flex items relayouts (WPT round 3, lane G)

`src/stylo.rs`: `visibility: collapse` coming or going on a flex item (its layout parent a flex container) gives the
item `BOX_DAMAGE`, as on a table row or row group: a collapsed flex item leaves its line, which keeps its cross size
(vendor/taffy's README.pyro.md, "`visibility: collapse` on flex items"), so the restyle changes layout, not only paint
(`crates/pyro-render/tests/mirror.rs`, `review_moved_longhands_match_fresh_builds`).
- `src/layout/inline/layout.rs` (`split_inline_containing_block`), `src/layout/mod.rs`
  (`lay_out_out_of_flow_boxes`): the containing block of an absolutely positioned box in a positioned inline box split
  around block-level content is the bounding box of the inline's head (at the end of the anonymous block before its
  box) and tail (at the start of the one after), an empty one a point at its line's end or start (Chromium): the
  split box and the inline roots around it leave such boxes to their parent, which lays them out against it (WPT
  `position-absolute-in-inline-003`, `position-absolute-dynamic-relayout-005/006`). Covered by
  `crates/pyro-render/tests/split_inline_containing_block.rs`.
- `src/layout/multicol.rs`, `src/layout/mod.rs` (`multicol_percent_height`): percentage heights in a multi-column
  container's strip resolve against the container's definite height (taffy `block_container_percentage_height`).
Covered by `crates/pyro-render/tests/scroll_markers.rs`.

## `hyphens: auto` (WPT round 3, lane TS)

`src/layout/hyphenate.rs`, `src/layout/inline/build.rs` (text nodes), `Cargo.toml` (`hypher`, MIT OR Apache-2.0, TeX
patterns of 25 languages): text in a `hyphens: auto` element of a language with patterns gets a soft hyphen at each
hyphenation opportunity of its words, which parley shows as the font's hyphen (or `hyphenate-character`) where a line
ends there, as for authored soft hyphens. `hyphenate-limit-chars` bounds the word and the parts around the hyphen
(`auto`: 5, 2 and 2, as Blink); a word with an authored soft hyphen is left alone. Chromium's dictionaries derive from
the same TeX patterns. Covered by `hyphens_auto_breaks_words_of_a_known_language` in
`crates/pyro-render/tests/text_spacing.rs`; WPT `css-text/hyphens/hyphens-auto-*`.

`src/layout/table.rs` (`clear_spanned`): the collapsed border segments inside a cell that spans rows or columns are
cleared after conflict resolution (Blink resolves borders on cell edges only: a column's or row's border is not drawn
through a spanning cell; WPT `css-tables/tentative/border-collapse-spanning-cells-001/002`). The table's style lists
its collapsed rows in taffy's `Style::collapsed_rows`, so a collapsed row takes its border spacing with it.
`src/layout/paint_tree.rs` (`Node::paints_in_table_phases`): the row groups, rows and cells blitz-paint paints in a
collapsed table's phases (no layer root, stacking context, float or positioned box among their paint children).
**Replaced content keywords, percentage height quirk (lane S2).** `compute_replaced_layout` resolves `min-content`,
`max-content` and `fit-content` in min/max sizes (from `BlockItemStyle::min_size_keyword`/`max_size_keyword`; the
style's `min_size()` says `auto` for them) to the element's content size: in an axis, the size the ratio gives from
its definite size in the other axis (clamped there), else its natural size, as Chromium does (csswg-drafts#3973).
`DocumentConfig::quirks_mode` (from pyro-dom's parser) turns on the percentage height quirk: an `auto`-height block
container (flow or flow-root, in flow, not a table cell's child) hands its own percentage basis for heights to its
children (`BaseDocument::quirks_skips_for_percent_height`, used by `block_container_passes_percentage_height` and by
inline layout for its atomic inlines), Chromium's `UseParentPercentageResolutionBlockSizeForChildren`. Covered by
`crates/pyro-render/tests/intrinsic_percentages.rs`.

Round 3 lane C2T (CSS2), covered by `crates/pyro-render/tests/css2_intrinsic_and_hints.rs`:
- `src/stylo.rs`: the `<br clear>` presentational hint (`left`, `right`, `all`/`both` → `clear`), WPT
  `css/CSS2/floats-clear/floats-clear-multicol-*`.
- `src/layout/inline/layout.rs`: an atomic inline's percentage margins resolve against the line root's content
  width (zero while it is measured), not the root's parent (WPT `intrinsic-size-with-negative-margins`).
- `src/layout/table.rs`: `rowspan="0"` spans the rest of the row group, and a rowspan never reaches past its group in
  the column cursor (`ColumnCursor::end_group`); a table without rows has no border spacing (its caption alone is the
  table, as Chromium), and an incremental update that removes the last row builds the table again.
### `display: contents` content and fieldsets (WPT round 3, lane ID)

- `src/layout/construct.rs` (`push_hoisted_children_and_pseudos`, `collect_complex_layout_children_in`): collapsible
  white space between inline content hoisted out of `display: contents` elements stays in the open run (anonymous
  block or flex/grid item): `x <div style="display: contents">y</div>` is "x y"; white space that starts a run or makes
  a run alone still gives no box (WPT css-display `display-contents-flex-002/003`, `-list-001`, their dynamic
  variants).
- `src/layout/construct.rs` (`flush_contents_pseudos`), `find_inline_layout_embedded_boxes`, `src/layout/inline/build.rs`:
  a `display: contents` element's `::before` and `::after` are its parent's content. They are made before the parent's
  children are classified (on a first build an empty contents element's generated text came out as blocks), and an
  inline root walks them with the element's children (they were dropped when the parent was all inline). WPT
  `display-contents-dynamic-pseudo-insertion-001`, `-fieldset-002`, `-shadow-dom-1`.
- `src/node/node.rs` (`Node::display_style`, `Node::is_fieldset`, `StyleFlags::IS_FIELDSET`), `src/node/stylo_data.rs`
  (`LayoutStyles::derive`): a `<fieldset>` is laid out with `stylo_taffy::convert::fieldset_display` (an inline one is
  an inline-block, a table-type one a block) and has no table-part or cell layout style (a `table-cell` fieldset kept
  its `height` in none). Covered by `crates/pyro-render/tests/display_contents_boxes.rs`.
- `src/layout/inline/build.rs` (`has_block_edges`): in a vertical writing mode an inline box's block-axis border and
  padding are its left and right ones: a root with such boxes is painted in Appendix E phases
  (`FLOW_CONTENT_OVERFLOWS`), so the boxes reaching past its lines paint over the block backgrounds after it.
- `src/layout/text_box.rs` (`is_empty_anonymous_block`): an anonymous block holding only collapsible white space and
  the empty heads or tails of split inlines (no content, no inline-axis edges) is no line for `text-box-trim`: the
  container's trim reaches the first or last line of the block in the inline after or before it (WPT css-inline
  `text-box-trim-block-in-inline-*`). Covered by `a_trim_reaches_the_first_line_of_a_block_in_an_inline`.

## `text-fit` (WPT round 3, lane TS)

`src/stylo_to_parley.rs` (`text_fit`, `TextStyle::spacing_scales`), `src/layout/inline/layout.rs`: a block
container's `text-fit` goes to its parley layout before the final break (not beside floats or with an initial letter,
where Blink turns it off too, and part of the break key); `consistent` breaks once without it to measure the block's
factor (`Layout::text_fit_block_scale`), then again with it. Which letter and word spacing are percentages goes to
parley, which scales those on fitted lines. parley does the scaling (its README).
- `src/layout/inline/layout.rs`: an inline layout in a vertical writing mode with a central dominant baseline also
  exports its baselines from the block-start edge (`taffy::Baselines::dominant_first/last`: the middle of the line's
  content, turned for `vertical-lr`), which grid baseline alignment reads (vendor/taffy/README.pyro.md).
- `src/layout/inline/build.rs` (text nodes): a preserved newline ends the bidi paragraph like a `<br>`, so the
  embeddings and overrides still open (`unicode-bidi: bidi-override` of the block) are pushed again after it (WPT
  css-text `pre-wrap-align-*-003`). Covered by `bidi_override_holds_after_a_preserved_newline`.
**SVG image root background, replaced ratios, stretch in anonymous blocks, size containers (lane S2).** An SVG
image's root `background-color` (attribute or `style`, `SvgIntrinsicDimensions::root_background`) is painted over the
bands of its viewport outside the area usvg painted it in (its `viewBox`), even-odd, so letterboxed images show it as
browsers do. `ReplacedContext::ratio_in_box_sizing_box`: an `aspect-ratio: <ratio>` without `auto` is between
`box-sizing` boxes (a border box under `border-box`). An anonymous block hands its parent's height as the definite
available height to its atomic inlines (`height: stretch` on an iframe). `effective_size_containment`: a
`container-type: size` (`inline-size`) box has size (inline-size) containment for layout.

## Custom highlights and the target text (WPT round 3, lane PX)

`HighlightName` and `BaseDocument::set_dom_highlights`: the embedder's custom highlights (CSS Custom Highlight API)
and the target text of a fragment's text directive, bottom first, each a list of `DomSelection` ranges;
`highlight_layer_ranges` maps them to inline roots' byte ranges as the selection's. `highlight_pseudo_style` computes
an element's `::highlight(<name>)` / `::target-text` style lazily (`None` without a rule), and
`style_declares_current_color` tells a highlight's `color: currentcolor` (which computes to the inherited colour) from a
colour. `getComputedStyle(el, '::highlight(x)')` matches the name too (the lazy path's `matching_fn`).

## Floated `::first-letter` (WPT round 3, lane PX)

A `::first-letter` with `float` is a box as an initial letter's (`FirstLetterSplit::floated`, the `IS_INITIAL_LETTER_BOX`
style node holding a copy of the letter, `display: block`), but laid out as any inline root and placed as any float
of the first line (`compute_inline_layout` takes the initial-letter path only with `initial-letter`). Outside markers
whose text is a `::marker` `content` string keep the marker's font (not the bundled bullet font). Covered by
`a_floated_first_letter_is_a_float` in `crates/pyro-render/tests/initial_letter.rs`.
- `src/layout/construct.rs` (`indents_inline_level_abspos`), `src/layout/inline/layout.rs` (static position): a
  container with only out-of-flow children, a text indent and an inline-level absolutely positioned child is laid
  out inline, so the box's static position is after the indent; on a line without in-flow content the box takes the
  container's direction (WPT css-text `text-indent-with-absolute-pos-child`).
- `src/layout/first_line.rs` (`starts_a_block`), `src/layout/inline/layout.rs` (text indent): an anonymous flex or
  grid item's first line is a first formatted line and takes the text indent (WPT `anonymous-flex-item-001`).
- `src/layout/table.rs` (hinted column widths): a table that shrinks to fit its columns is no wider than its
  `max-width`, nor narrower than its columns' min-content widths (WPT css-text `overflow-wrap-min-content-size-001`,
  `word-break-min-content-001`). Covered by `a_shrinking_table_is_no_wider_than_its_max_width`.


A single-line text input's line is centred in its content box also when it is taller than the box (it overflows
both edges, as in Blink; WPT css-overflow `input-scrollable-region-001`).
- `src/layout/mod.rs` (`LayoutPassState::calc_size_inputs`, `has_calc_size`): a box whose `width` or `height` is
  `calc-size()` is laid out as its basis first (Taffy, through stylo_taffy) and then with a known size: the
  calculation of the size its parent gave it, else of its own measured size, in its `box-sizing` box, between
  `min-*` and `max-*`. Measured in definite space, an `auto`/`fit-content` basis gives the space between the
  calculations of its min-content and max-content widths (Blink's fit-content of the result). A size it measured
  and its parent hands back is not calculated again (`calc_size_results`, per pass). Horizontal writing modes only.
## List item numbers from the `list-item` counter (WPT round 3, lane IT)

A marker's number is its item's `list-item` counter value, computed by the counter walk (`layout::counters`, which
now runs for every document with list items: `generated_counters` is set by a `display: list-item` style) and kept
in `BaseDocument::list_item_numbers` (elements; `::before`/`::after` list items by their originating element);
`list::collect_list_item_children` reads it (its running index is only the fallback). The walk applies HTML's
presentational hints itself: `ol start` (reset to start − 1), `ol reversed` (a reversed `list-item` counter: list
items increment it by −1; without `start` its initial value is CSS Lists 3 § 4.4.2's, `reversed_initial`) and
`li value` (`counter-set`), each unless an author rule declares the property; `counter-reset: reversed(name)` works
for any counter. A changed number damages the item and its parent; `start`, `reversed` and `value` changes dirty
the counters. Covered by `crates/pyro-render/tests/list_numbering.rs`.
- `src/layout/inline/build.rs` (tab stops): the space a `tab-size` number counts is U+0020 of the first font that has
  one, measured between two digits (a font without spaces may map U+00A0; WPT css-text `tab-size-integer-005`).
- `src/document.rs` (`BaseDocument::svg_images`, `with_svg_images`, `damage_inline_svgs`), `src/layout/construct.rs`,
  `src/node/svg.rs`: SVG `<image href>` and `<feImage href>` of a resource that is no `data:` URL are drawn from the
  `data:` URLs pyro-render fetched for that href (usvg loads `data:` URLs only; a JPEG comes as PNGs turned by its
  EXIF orientation and not): each `<svg>` source (inline, or the reference document of an HTML box's
  `filter: url(#f)`) has the href replaced when it is built, unturned under the root's `image-orientation: none`;
  new entries build the inline `<svg>`s again.
- `src/layout/table.rs`: a table cell `width` that stays a `calc()` (a length and a percentage) is `auto` in table
  layout, as in Blink (WPT css-values `calc-width-table-auto-1`, `-fixed-1`).

Checkboxes and radios are 13x13 by nature (the leaf measure in `layout/mod.rs`), not by `width`/`height` in the UA
sheet, so a flex parent stretches them and percentages size them (WPT css-flexbox `stretch-flex-item-*-input`). A
fixed-layout table with a percentage width (empty ones, laid out as blocks, too) reports Blink's `kTableMaxWidth`
(1,000,000 px) as its max-content width (WPT css-flexbox `fixed-table-layout-with-percentage-width-in-flex-item`).
- `src/layout/construct.rs`: the children of an element its `content: url()` replaces (`is_content_image`) generate
  no boxes, as an `<object>` showing an image (the root's children were laid out and painted over its image).
- `src/layout/damage.rs` (`flush_content_image`), `src/document.rs` (`apply_loaded_image`): an `<img>` or `<video>`
  shows the image of its `content: url()` in place of its `src`/`poster`, whichever loads last (Blink; WPT css-content
  `element-replacement-image-src.tentative`). `src/node/node.rs` (`layout_style`) flags an element replaced by its
  content image (`StyleFlags::IS_CONTENT_REPLACED`).

More of lane IT (round 3):
- `src/layout/list.rs`: the list items of an inline box child (`ol { display: inline }`) are numbered with their
  container's (no construction of their own collects them; WPT css-lists `inline-block-list*`). An outside
  string marker's layout has the item's direction as its base direction; `ListItemLayout::isolate`: an inside
  string marker is bidi-isolated in the item's paragraph (`src/layout/inline/build.rs`) unless a `::marker` rule
  declares `unicode-bidi: normal` (WPT css-lists `list-style-type-string-005a/006`, css-pseudo
  `marker-unicode-bidi-*`).
- `src/layout/inline/build.rs`: the marker line of an empty item with an outside image marker is as tall as the
  image above the baseline (WPT css-lists `outside-marker-taller-than-content`).
- `src/layout/mod.rs` (table layout): a table without a known width whose `max-width` cuts below its min-content
  width (measured without `width` and `max-width`) takes its min-content width (WPT css-tables
  `table-intrinsic-size-002`; a fixed table with a larger `width` keeps to `max-width`, CSS2
  `max-width-applies-to-013`).
- `src/layout/table.rs`: `CollapsedEdge::is_double`.

A table's content-size measure (`SizingMode::ContentSize`, a flex base size) runs the grid as an inherent-size
layout, so the width its columns give survives, with the table's own `height` and `min-height` dropped (WPT
css-flexbox `table-as-item-min-height-1`, `table-as-item-percent-width-cell-001`).
A block-level absolutely positioned box in an inline formatting context (`layout/inline/layout.rs`) is aligned in a
static-position rectangle with no inline extent at the line start and the block extent of the line box, one line
down when content precedes it (Blink; WPT css-align `abspos/align-self-static-position-003/004/007`). Collapsed rows
keep their tracks (`row_tracks`); Taffy collapses them once sized.
- `src/util.rs` (`content_replaces`, `content_gradient`), `src/layout/construct.rs`, `src/layout/damage.rs`,
  `src/layout/replaced.rs`, `src/node/element.rs` (`ElementData::content_gradient`): an element whose `content` is
  one image builds no boxes for its children from the computed value on (while the image loads, and when it fails:
  WPT `element-replacement-alt`); a `content` gradient (also through `image-set()`) replaces an element or a
  `::before`/`::after` box as an image without a natural size (default object size, or its `width`/`height`),
  which blitz-paint fills with the gradient (`element-replacement-gradient`, `pseudo-element-gradient`,
  `pseudo-element-image-set`).
- `src/stylo_to_parley.rs` (`vertical_align`): `vertical-align`'s keywords (stylo's `VerticalAlign*` variants) keep
  parley's CSS 2.1 alignments; the `alignment-baseline` longhand's `text-top` / `middle` / `text-bottom`, and those of
  an inherited `dominant-baseline` under `alignment-baseline: baseline`, map to parley's `TextOver` / `XMiddle` /
  `TextUnder`; the pref `layout.css.dominant-baseline.enabled` is on (here and in pyro-dom). An `align=middle`
  hint is `VerticalAlignMiddle`. Covered by `crates/pyro-render/tests/alignment_baseline.rs`.

- `src/layout/table.rs` (lane IT, round 3): only a table's first header group goes before its rows and only its first
  footer group after them; the others stay in place as row groups (WPT css-tables `row-group-order`).
  `ColumnCursor::place` counts a row-spanning cell's rows as `start_row` counts them down, so the cells after it get
  their columns (`TableCell::column`: column widths, part backgrounds; WPT css-tables
  `paint/background-image-column`, `border-collapse-spanning-cells-003`).

- Lane SP: `BaseDocument::containing_block_below_table` (layout/table.rs): an absolutely positioned or fixed box whose
  containing block is a row or row group (positioned, transformed, any property that makes one) is laid out by the
  table against the part's area (`table_part_containing_block`, now for `fixed` too) (WPT css-transforms
  `transform-transformed-*-contains-fixed-position`, `-tr-percent-height-child`). `replaced.rs`: a keyword minimum
  wins over a length maximum (`replaced-aspect-ratio-stretch-fit-003`). `DocumentConfig::line_height_quirk`
  (quirks and limited-quirks mode): no strut for lines without text (inline/build.rs, parley
  `Layout::set_line_height_quirk`; css-sizing `intrinsic-percent-replaced-006`).
An inline formatting context whose text has anything but collapsible white space (a no-break space counts as text)
does not let margins collapse through it, even when its lines have no height (CSS 2.1 §9.4.2; WPT css-fonts
`font-size-zero-1`, css-color `t32-opacity-offscreen-multiple-boxes-2-c`, whose reference is `&nbsp;` text).
- SVG `<foreignObject>` (`layout/construct.rs` `svg_foreign_objects`, `document.rs` `svg_foreign_objects`,
  `resolve.rs` `foreign_object_roots`, `layout/paint_tree.rs`, `node/svg_css.rs` `foreign_object_href`, UA sheet):
  each `<foreignObject>` of an inline `<svg>` keeps its construction damage, is built, laid out (a root block of its
  `width`/`height`, mapped as presentation hints in `stylo.rs`, `overflow: hidden`), transformed and given a paint
  tree as a root of its own; the `<svg>` source writes it as an `<image>` whose data names it, so usvg applies its
  transform, clip path, mask, filter and opacity. Without a width or height it renders nothing.
- `stylo.rs` (`is_html_element_in_html_document`): only HTML elements of an HTML document match type selectors
  case-insensitively (`svg|foreignObject` matches, `foreignobject` does not match an SVG element).
- `layout/construct.rs` (`input_type_keyword`), `layout/mod.rs`: an `<input>`'s `type` is read case-insensitively
  and an empty or unknown value is the Text state (a text field), also after a change from `hidden`.
- `resolve.rs` (`refresh_container_query_sizes`): an element inside an `<svg>` (a `<foreignObject>`, now a CSS box)
  is no size query container.
- Lane SP: inline/layout.rs: `height: stretch` on atomic inlines (not replaced ones, `replaced.rs` has it) fills the
  inline root's definite content height less margins (`inline_box_inputs` `stretch_height`); an inline root's own
  `min-height`/`max-height` content keywords clamp its final height (css-sizing
  `hori-block-size-small-or-larger-than-container-with-min-or-max-content-1`).
- SVG images with a `<foreignObject>` (`net.rs` `foreign_source`, `document.rs` `svg_image_documents`,
  `update_svg_image_documents`, `html.rs` `HtmlParserProvider::parse_svg_document`): the image's source is parsed as
  an XML document of its own (no script runs in it), laid out at the image's natural size each resolve, and painted
  by blitz-paint in place of the usvg tree, which drops HTML content.
- `node/svg.rs` (`for_url`): an SVG image without a `viewBox` and with a zero `width` or `height` (no natural ratio)
  is laid out in the box it is drawn in, as Blink does.
- `node/svg_css.rs` (`attribute_boxes`, `boxes`): an `<image>`'s or `<foreignObject>`'s fill box is its geometry;
  a `<foreignObject>` without a size is an empty group (its filter region still paints).
- `layout/list.rs`, `layout/inline/build.rs`, `layout/construct.rs`: an inside symbol marker stays a bundled-font symbol
  under `::marker` rules unless one sets `font-family`; it is not spaced by `letter-spacing`/`word-spacing`, is
  bidi-isolated like a string marker, and gives an otherwise empty item its line. An inside marker's `::marker` style
  has its own emphasis marks (with their room in the line).
- `layout/first_letter.rs`: `::first-letter` takes any punctuation before the letter (dashes and connectors too) with
  combining marks and typographic spaces, and after it all but opening punctuation and dashes (a non-word-separator
  space bridges to more); a preserved line break before any letter means no first letter; a `::before`/`::after`
  box with an atomic display does not pass on its container's first line.
- `layout/mod.rs`: an `<input type=number>` is sized as a text field (it was an empty box); `size` does not apply.
- `net.rs`: `@font-face` rules inside `@media`, passing `@supports` and `@layer` blocks load too (media lists are not
  evaluated there); a `tech()` source needing a technology outside `SERVO_SUPPORTED` is skipped.
  `document.rs` sets `layout.css.font-tech.enabled`.
- `layout/inline/build.rs`, `stylo_to_parley.rs` (round 4 lane A): `<wbr>` is a U+200B soft wrap opportunity (it was
  nothing: no break at all, also under `keep-all`); a quirks-mode `<br>` carries its own `line-height` (parley's line
  height quirk sizes a line holding only it); `word-break: auto-phrase` maps to `keep-all` with parley's phrase
  breaks; a `hyphenate-character` of two to four characters is `SoftHyphen::Chars`.
- Round 4 lane B: `layout/mod.rs` `quirky_fill_height` (Quirks Mode §3.6/§3.7): in quirks mode the root html element and
  a body child of it, in flow with `auto` height (flow, flow-root, flex or grid), fill the viewport less margins (the
  body: less the html element's margins, border and padding too). Taffy block uses it through
  `block_container_fill_height`, inline layout (`compute_inline_layout_inner`) as a least height and a definite height
  for `stretch` and its children's available height, flex and grid through `quirky_fill_layout` (laid out again at the
  filled height). WPT css-sizing `stretch/stretch-quirk-001`, css-flexbox `flexbox_quirks_body`.
