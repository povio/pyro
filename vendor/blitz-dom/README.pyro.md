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
- `layout/inline/build.rs` (`ends_lines_with_ellipsis`), `node/text.rs` (`TextLayout::ellipsis`): an inline root
  that clips its inline overflow and has `text-overflow: ellipsis` on its end side (Stylo keeps a single value as
  `second`) gets "…" shaped in its own style; blitz-paint ends each overflowing line with it. Repro
  `bench/fidelity/repros/text-overflow-ellipsis.html`.
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
- `filter` is written as the attribute, not in the `style` attribute: usvg's `style` tokenizer ends a value at its
  first `)`, which cut `drop-shadow(rgb(0, 128, 0) 50px 0px 0px)` short and left the element unfiltered. Drop shadows
  are written with their colour resolved to `rgba()` (usvg knows neither `currentcolor` nor `color-mix()`).
- A `<text>`'s `text-shadow` (one shadow) is a copy of the text written before it, in a group moved by the
  offset (and blurred by half the blur radius), whose fills and strokes that are not `none` are the shadow's
  colour, their opacities times its alpha: Chromium draws a text shadow as the text painted again, which a
  reference drawing the text twice matches to the level (a drop shadow of the text was 2 levels off at its
  edges; WPT `css-text-decor/text-shadow/svg-*`).
- `clip-path` and `mask` references (attributes or CSS) to an element of the `<svg>` that is no `<clipPath>` or
  `<mask>` are written as `none`: such a reference is ignored (CSS Masking), where usvg left the element out (WPT
  `clip-path-invalid-reference`, `mask-invalid-reference`). A `transform`, `patternTransform` or
  `gradientTransform` attribute that is no transform list in SVG's grammar (a comma that separates nothing:
  `rotate(90,)`, which svgtypes reads) is ignored, as in Chromium (WPT `svg-external-styles-014`).
- CSS mask layers on an SVG element (other than one `url(#mask)` of the `<svg>`, which stays usvg's `mask`) become
  a `<mask>` made for it: SVG images (embedded as `data:` URLs from the document they were loaded from,
  `SvgImageData::source`, which `net.rs` keeps) and linear gradients (as `<linearGradient>`s) as tiles placed by
  `mask-size`, `mask-position`, `mask-repeat` (`repeat`, `space`, `round`, `no-repeat`) and `mask-origin`, clipped
  to `mask-clip`, in SVG's boxes (`content-box` and `padding-box` are the fill box, `border-box` the stroke box,
  `view-box`, `no-clip`); a reference to a `<mask>` as a white rectangle over the fill box under it; a luminance
  layer as the alpha of a white rectangle under a luminance `<mask>` of its tiles; each layer above the bottom one
  in a group whose id names its `mask-composite` operator (`__pyro_compose_srcout_1`), which anyrender_svg
  composites with it. An image that failed to load is a transparent layer (as the only layer it leaves the element
  unmasked, as in Chromium: WPT `bad-mask-image-svg-2`). Raster images and other gradients leave
  the element unmasked, as before. `document.rs` (`apply_loaded_image`): a mask image that loads for an SVG
  element damages its outermost `<svg>`, whose source is written again with it (WPT `css-masking/mask-image`
  `mask-clip-*`, `mask-repeat-*-svg`, `mask-position-*`, `mask-composite-2d`, `mask-mode-e`, `bad-mask-image-svg`).
- `mix-blend-mode: plus-lighter`, which usvg does not know, wraps the element in a group whose id names the
  operator (`__pyro_compose_pluslighter_1`).
- A 3D transform (`stylo_to_kurbo::resolve_transform`'s `Spatial`) about its origin, and for the root's children
  the root's `perspective`, is flattened into the plane (`flatten_transform`): exactly for an element that stays
  parallel to it, as a depth under a perspective is a scale about its origin (WPT `perspective-svg-child-*`).

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
    box's corners (least squares) stands in until a projective warp of a layer draws the trapezoid. It is
    close for a box parallel-ish to the screen, and off by tens of pixels inside a large, steeply turned box.
  - A box at or behind the viewer, or whose 4x4 matrix has no inverse (`scale: 1 1 0`), is not drawn.
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
    element itself is not hidden: its children are planes of their own, which paint would hide with it.
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
  content (WPT `mix-blend-mode-root-element-group`). `src/node/node.rs` (`hit`): an out-of-flow child placed so
  is hit where it is painted (`lifted_paint_offset`). Covered by `crates/pyro-render/tests/opacity_group.rs`.

Covered by `crates/pyro-render/tests/transform_3d.rs` (paint, a scrolled perspective, the card flip, a
composed 3D rendering context, grouping properties, a filtered inline's positioned boxes, layout boxes), `crates/pyro-layout/tests/transform_3d.rs` (`getBoundingClientRect`, `elementFromPoint`) and the unit
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
out by an ancestor: `display: inline` boxes (to the inline root, replaced ones included since `is_or_contains_block`
looks through them), `display: contents`, table rows, row groups, columns and column groups (to the table) and list
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
revalidation selector, so style sharing stays correct. The HTTP `Content-Language` header is not seen (the
document's response headers do not reach the renderer). Covered by `crates/pyro-render/tests/lang_pseudo_class.rs`.

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

## SVG reference cycles usvg does not break

`src/svg_refs.rs` (new), `src/util.rs` (`parse_svg_image`): usvg sets a clip path, mask or filter reference that
closes a cycle of one or two elements to `none` (`fix_recursive_links`), but follows a longer cycle without end and
overflows the stack, which ended the browser (WPT css/css-masking/clip-path-svg-content/clip-path-recursion-001.svg:
`clip0`, `clip`, `clip2`). Before usvg parses a source that has `url(` and one of those properties, it is read with
roxmltree (usvg's own), and every reference that closes a cycle (a back edge of a depth-first search over the
elements of one kind, through the references their content makes, as attributes or `style` declarations) is set to
`none`, as usvg treats a short cycle. Sources without a cycle are passed on as they are. Covered by
`crates/pyro-render/tests/svg_reference_cycles.rs`.

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
  quote depth (`quotes: auto` is English's). A counter is visible to its element, its descendants and its
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

Tests: `generated_content` in pyro-render; `first_letter` and `counters` unit tests here.

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
