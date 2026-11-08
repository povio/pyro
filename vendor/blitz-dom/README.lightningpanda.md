# blitz-dom main (be60597), patched

`packages/blitz-dom` of [DioxusLabs/blitz](https://github.com/dioxuslabs/blitz) main at commit
`be60597f67b3cece0456d26e8d7ff5a88600bbc5` (#1456, 124 commits past 0.3.0-beta.4 = `92b11f1b`; its version
is still 0.3.0-beta.4), used through `[patch.crates-io]` in the workspace `Cargo.toml`. License: MIT OR
Apache-2.0, as upstream. `src/` and `assets/` are copied; `Cargo.toml` is the package manifest with the
workspace's dependency versions written in, git dependencies replaced by their crates.io releases (Taffy and
Stylo come from `vendor/`, anyrender 0.14 from crates.io: main's anyrender API is unchanged). blitz-traits,
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
against the upstream commit above); they are covered by lp-render's rendering tests.

## `<br>` as inline text

- `node/node.rs` (`is_line_break`), `layout/construct.rs`: a `<br>` is a piece of inline text in every container, as
  in Chrome: as a flex or grid child it joins neighbouring text and `<br>`s in one anonymous item, one line each,
  instead of being blockified into an empty box. All of its CSS is ignored except `display: none` and
  `display: contents`, which remove it. In flex and grid, white space after text or a `<br>` stays when it's preserved.
  A `display: none` child no longer ends a run of inline content. Covered by `crates/lp-render/tests/br_flex_item.rs`
  and `bench/pdf/fixtures/generators/fx-br.mjs`.

## Incremental passes for layout queries

- `src/resolve.rs` (`resolve_layout_children`): an undamaged subtree without damaged descendants is not
  walked (its layout children are those of the last pass); anonymous boxes are always visited.
- `src/resolve.rs` (`resolve_transforms`): a node's cached scrollable overflow is reused only when it and
  its subtree are undamaged (a descendant's relayout reaches ancestors as damage, not as
  `RECALCULATE_OVERFLOW`).
- `src/layout/writing_mode.rs`, `src/layout/mod.rs`, `src/node/element.rs`: incremental rounding, keyed by
  `LayoutData::rounded_from` (see "Incremental rounding" in `vendor/taffy/README.lightningpanda.md`).
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
  overflow uses unrounded sizes, and paint (radii, backgrounds), sticky offsets and lp-render's geometry derive
  positions from the layouts when they are used.
- `src/resolve.rs` (`resolve_layout`): no Taffy pass or rounding with nothing damaged and the same viewport.
  `BaseDocument::needs_resolve` tells a caller whether a `resolve` could change anything.
- `src/node/node.rs`, `src/resolve.rs` (`resolve_layout_children`): `Node::layout_child_of` names the node whose
  `layout_children` list holds a node, entered and left where those lists are rebuilt (a child another container
  collected in the same pass keeps pointing there). A node is a box of the current layout when it equals its
  `layout_parent`, which inline content of an inline root also has without being listed, or when it is a hoisted
  (out-of-flow) box whose `oof_containing_block` is its `layout_parent`: lp-render answers that in O(1) instead of
  indexing the parent's list (a table's cells) after every sync. The incremental table update keeps it too: when it
  lists a table's cells again (`update_rows`) and when a former box becomes a table part (`drop_part_box`), the
  boxes that left a list leave it (`leave_layout_children`) and the new ones enter it.
- `src/layout/paint_tree.rs` (`build_paint_tree_impl`): a rebuilt paint list already in paint order (tree order with
  no positioned, floated or z-indexed children: a table's cells) is not sorted again.
- `src/stylo.rs`, `src/document.rs` (`BaseDocument::has_sticky`): the style traversal records the elements it styles
  `position: sticky`, and drops the entries whose node is gone or no longer sticky after each traversal, so whether a
  document has sticky boxes is not a scan of every node per sync. Pseudo-elements are not recorded (lightningpanda
  reports sticky offsets for elements only).

lp-render's mirror tests (incremental documents against fresh builds after random mutations) cover them.
- `assets/default.css`: text inputs take Chromium's UA box (`border: 2px inset`, `padding: 1px 2px`; a
  40x30 input is 48x36), textareas keep `1px solid` / `2px` and wrap like Chromium's
  (`white-space: pre-wrap; overflow-wrap: break-word`: a word longer than the box breaks).
- `src/document.rs`: `set_hover_node` puts `:hover` on a node and its ancestors (an embedder that tracks
  the pointer itself: lightningpanda mirrors lp-dom's hovered element).
- `src/document.rs`: Stylo's prefs for `:nth-child(An+B of S)`, container and style queries,
  `@starting-style`, `@scope`, `line-clamp`, `fit-content()` and `@supports at-rule()` are on, as in
  lp-dom's cascade. The prefs are process-wide and read while sheets are parsed, so Blitz used to drop
  those rules unless lp-dom's cascade had been created first.
- `src/document.rs`, `src/mutator.rs`, `src/form.rs`: a URL that cannot be resolved against the
  document's (`src="x"` in a `data:` document) loads nothing instead of panicking, as in browsers.
- `src/resolve.rs`: `resolve_style` runs the style pass of `resolve` alone (messages, device changes,
  the traversal), leaving damage, box construction and layout to the next `resolve`: lightningpanda
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
  again. Sheets are not edited through Blitz's CSSOM in lightningpanda (its CSSOM edits the text).
- `src/net.rs`: decoded raster images are kept by their encoded bytes for every document of the process
  (64 images, 64 MB of pixels): a template's logo is decoded once, not once per document.
- `assets/default.css`, `src/layout/mod.rs`: a drop-down `<select>` is an inline-block box with a 1px
  border and `line-height: normal`, sized from its longest option label (about half an em per character)
  plus the arrow, a line of its font tall (Chromium's menu list). It had no box at all: its options are
  hidden because Blitz paints the control. Its baseline is its text line's, centred in the content box
  (as for text inputs), not its bottom edge.
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
rebuilds, tables, rounding) read by lp-render's profiling. The phase timers run only while
`phase_profile::set_timing(true)` (lp-render's `profiling::enable`, turned on by `LP.geometryStats`,
`Performance.enable` and `LP_GEOMETRY_PROFILE`) is on: `mark` is a no-op otherwise. The counters always count.

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
printing. The servo build of Stylo parses neither `size` nor `page`, so the parsed rule does not have them. lp-render's
`page_box` reads them.

## Collapsed tables without uniform borders

`src/layout/table.rs` (`CollapsedEdges`), `src/node/stylo_data.rs`, `src/node/element.rs`: upstream has one gutter, from the
first cell's borders. A table whose collapsed borders are not one uniform grid now resolves them by the spec's conflict
rules: each cell keeps half of the borders around it (`LayoutData::collapsed_border`, applied to the cell's
`LayoutStyles::cell`) and the table half of its outer ones, as Chromium's boxes do. Uniform tables keep the gutter.
Values in `bench/pdf/fixtures/chrome-156/table-collapse.json`, tests in `crates/lp-render/tests/table_collapsed_borders.rs`.

`src/layout/table.rs`: the table places its cells once, as it is built (`place_cells`, stored as
`TableContext::placement`), and hands the areas to the grid through taffy's `LayoutGridContainer::track_counts`
and `resolved_placement` (see `vendor/taffy/README.lightningpanda.md`). It runs the grid's `row dense`
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
- `src/node/element.rs` (`sync_lp_attribute_state`), `src/stylo.rs`: the tokens of lp-dom's
  `urn:x-lightningpanda:state` `state` attribute are `ElementState` bits (`VALID`, `INVALID`,
  `PLACEHOLDER_SHOWN`, `INRANGE`, `USER_INVALID`, ...; `READONLY` unless `read-write`), set when the
  attribute is written, and the pseudo-classes match the bits. Stylo's snapshot invalidation and style
  sharing compare element states, so they follow these like `:checked`. A selected `<option>` (lp-dom
  writes `selected` for the current selectedness) is `CHECKED`. `each_custom_state` lists the
  `custom-states` attribute instead of panicking.

Covered by `crates/lp-layout/tests/has_state.rs` and `crates/lp-browser/tests/has_state.rs`.

## Top layer

- `src/layout/paint_tree.rs` (`note_lp_state`, `top_layer_nodes`, `top_layer_position`), `src/mutator.rs`
  (`create_element`, `set_attribute`, `clear_attribute`, node drops), `src/document.rs` (`top_layer`): elements whose
  `urn:x-lightningpanda:state` `state` attribute has a `top-layer=N` token (lp-dom's render tree) are kept by `N`
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
(`assert_layout_styles_are_current`). lp-render's mirror tests `tables_lay_out_from_the_styles_written_at_restyle`
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
  `tables_keep_their_layout_when_only_the_available_width_changes` in `crates/lp-render/tests/mirror.rs`.
- The grid is laid out again from its last layout when the columns are the same (see "Table grids laid out again
  from their last layout" in `vendor/taffy/README.lightningpanda.md`): `take_table_grid_state` gives back the cells
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
  lost their boxes all the same. The context is dropped with them (the box is no table; lp-render reads a table's
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
  `display: none` and never visits the children of one). The moved element kept its styles, and lp-render reported
  the box its old inline layout gave it. Its pseudo-elements and layout tree (`leave_layout_tree`) go too. Every
  insertion looks up the ancestors, not only one of a styled element: lp-dom builds a new wrapper by moving styled
  children into the new, unstyled element before inserting it, and the paragraph so moved under a hidden element was
  never laid out once shown.
- `src/document.rs` (`leave_layout_tree`, `left_layout_lists`), `src/resolve.rs`: a box that left its container's list
  in a pass (`resolve_layout_children`, `update_rows`, `drop_part_box`) and that no container collected again by the
  pass's end is out of the layout tree, with what its subtree built: its layout children leave its list in turn, and
  its anonymous blocks are freed. A fresh build has none there; the stale lists kept boxes under a `display: none`
  element (and in a cell made a block) in lp-render's layout tree, and tripped `assert_layout_parents_consistent`
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

`crates/lp-render/tests/mirror.rs` compares the incremental document with fresh builds after cell edits, row appends,
inserts and removals, spans, collapsed borders, nested and vertical tables, stacking contexts in cells and `display`
changes (`table_*_match_fresh_builds`), and counts what an edit, an append, a removal and an insert collect and lay
out (`a_cell_edit_collects_only_its_row`). `blitz_tables` and `blitz_table_grids` (lp-render) report tables built, cells
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
Covered by `crates/lp-render/tests/incremental_construction.rs` (each change compared with a fresh build, boxes and
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
`vendor/taffy/README.lightningpanda.md`). `phase_profile::performed_layouts` counts layout algorithm runs that place
children. Covered by `crates/lp-render/tests/out_of_flow_boundary.rs`.

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
build of every node, tables visited as any box. Covered by lp-render's `settle_resolution_tests`
(`resolve_leaves_the_paint_tree_to_the_first_hit_test`, `element_from_point_sees_every_resolve_since_the_last_paint`).

## Line breaking resumes after the lines whose text did not change

`src/layout/construct.rs`, `src/resolve.rs`, `src/layout/inline/build.rs`, `src/layout/inline/layout.rs`: an inline
root built again keeps its parley layout (the rest of its `TextLayout` is reset), builds with
`build_into_keeping_lines` (parley keeps the lines its last break decided over the shaped content that is the same,
see `vendor/parley/README.lightningpanda.md`) and, with no floats involved and the text not justified, resumes the
break after them (`BreakLines::resume` with main's `max_advance` for the lines after the first; the float
bookkeeping of #1378/#1390 starts at the kept lines). `phase_profile::line_breaks` counts breaks, lines kept and
the text bytes the breaker went over. Covered by `crates/lp-render/tests/line_break_resume.rs`.
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
`crates/lp-render/tests/mirror.rs`.

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
  embedder, as Gecko's `RestyleSiblingsForNthOf`), and a change of lightningpanda's state attribute
  (`urn:x-lightningpanda:state`) is layout damage when its `natural=W,H` token changes. Its pseudo-classes
  are element states (`sync_lp_attribute_state`, "`:has()` invalidation for state and attribute changes"
  above), so the snapshot sees them change and Stylo's state invalidation reaches the siblings; nothing
  restyles them here. Covered by `sibling_and_fragment_invalidation_stays_exact`,
  `radio_group_changes_invalidate_selectively_*`, `form_state_writes_invalidate_selectively_*` and
  `loaded_image_natural_size_reaches_flow_through_the_dom` (lp-layout).
- `src/stylo.rs` (`RecalcStyle::process_preorder`): a paint-only restyle of a table part in a
  `border-collapse: collapse` table that changes its borders or `color` is damage on it: the collapsed
  edges (`CollapsedEdges`) hold resolved colours, collected again with the part's row. Covered by
  `tables_lay_out_from_the_styles_written_at_restyle` (lp-render).
