# taffy main (d0345ba), patched

[DioxusLabs/taffy](https://github.com/DioxusLabs/taffy) main at commit
`d0345bac609a10b25278ecc0850e862be9504e74` (#1402, 42 commits past v0.15.1; its `Cargo.toml` still says
0.15.0), two commits past the one Blitz main pins (`4aca4f4`), used through `[patch.crates-io]` in the workspace `Cargo.toml`.
License: MIT, as upstream. `src/`, `examples/` (without `examples/parley`, a crate of its own), `README.md` and a `Cargo.toml` without the
workspace, dev-dependency, bench and example sections were copied.

## The change

`UPSTREAM.patch` (applies with `git apply` from the root of an upstream checkout at the commit above) is
the whole diff.

`compute/flexbox.rs`, `determine_container_main_size`: an auto-width row flex container in definite
available space with a single line took the line's max-content length. CSS sizes it fit-content,
`min(max-content, max(min-content, available))`, so its shrinkable items shrink and their text wraps.
Hit by a `display:flex` row inside an `align-items:center` column (ToS invoice footers) and by
`width: fit-content`. Column containers are unchanged: an auto height is the content height. The available size is the
content box's; the clamp adds the container's padding and border to it before comparing outer
sizes (`a_padded_inline_flex_keeps_its_max_content_width`).

Upstream's full test suite (6605 generated tests) passes with the patch. The regression tests are
`a_centered_flex_item_is_clamped_to_its_container` and
`a_fit_content_flex_row_is_clamped_to_the_available_width` in `crates/pyro-render/tests/render.rs`.

`compute/mod.rs` and `tree/traits.rs`: incremental rounding. `RoundTree` gains `rounded_unchanged` and
`record_rounded` (defaults: never skip, record nothing), and `round_layout` skips a subtree its tree reports
unchanged. See "Incremental rounding" below for how blitz-dom uses them.

`compute/block.rs` and `tree/traits.rs`: `LayoutBlockContainer::block_child_width_fits_content`
(default false) lets a tree keep a block-level child's `auto` width at its content size, as for tables and
replaced elements. blitz-dom answers true for form controls (`input`, `select`, `textarea`, `button`),
which Chromium does not stretch in block layout.

`compute/block.rs` and `tree/traits.rs`: `LayoutBlockContainer::block_child_establishes_formatting_context`
(default false) lets a tree make a block-level child that is a block container by its style a block
formatting context root all the same, so that its parent keeps it clear of floats as it does a `flow-root`
(`is_in_same_bfc`). blitz-dom answers true for multi-column containers (CSS Multi-column Layout 1 §2), which
Chromium places beside a float, narrowed. Covered by `a_block_multicol_container_avoids_floats` in
`crates/pyro-render/tests/multicol.rs`.

`compute/flexbox.rs`, `determine_container_main_size`: a column container's automatic block size is the
sum of its items' hypothetical main sizes (flex base size clamped by the resolved minimum and the max
size), as Chromium has it. An item's `height` counted on its own before: `flex: 1 1 0px; height: 10px`
made its container 10px tall (Chromium: 0), and a percentage basis of the indefinite container, which
is used as `content`, still counted the item's `height`. Covered by
`a_column_flex_container_is_as_tall_as_its_hypothetical_items` and
`flow_percentage_flex_basis_in_an_indefinite_column_is_content` in `crates/pyro-layout/tests/flow.rs`.

`compute/grid/mod.rs`, `compute/grid/track_sizing.rs` and `tree/traits.rs`:
`LayoutGridContainer::grid_rows_are_table_rows` (default false) marks a grid whose rows are a table's;
blitz-dom's table answers true. There an item spanning several rows grows them as Chromium grows the rows
under a rowspan cell (`DistributeExcessBlockSizeToRows`, percentage rows aside) instead of by the grid
algorithm's even distribution: the excess goes to the rows where another rowspan starts, else to the
unconstrained rows with a size in proportion to it, else (all rows empty) to the last row, else to the
rows with a size in proportion to it. Covered by `table_rowspan_like_chromium` in
`crates/pyro-layout/tests/flow.rs`.

`compute/grid/track_sizing.rs`, `expand_flexible_tracks`: flexible tracks are sized as Chromium's
`ExpandFlexibleTracks` sizes them. Each track's share of the flex fraction is truncated to 1/64 px (a
layout unit) in f32 raw units after an epsilon, and the fraction it loses carries into the next flexible
track, so `repeat(3, 1fr)` over 668px is 222.65625, 222.671875, 222.671875, as Chromium reports.
Chromium sizes a run of identical tracks (a "set") as one; this sizes every track on its own, which agrees
whenever grid items start in each track. The layout unit is 1/64 of a Taffy unit: a CSS px at scale 1.
Upstream's generated tests expect exact fractions and are not run against this change.
`grid_parity_tailwind_columns` and `grid_parity_calendar_fr_rows_with_min_height` in
`crates/pyro-layout/tests/flow.rs` cover it.

`style/block.rs` and `compute/block.rs`: `BlockContainerStyle::margin_trim` (default: neither edge) is
`margin-trim` in the block axis (css-box-4 §3.3; stylo_taffy answers from the computed value). With `start`,
the top margins of the in-flow children at the container's top edge are truncated to zero together with every
margin that collapses with them: the margins their descendants collapse through them, and both margins of a
child that is collapsed through, after which the next child is at the edge. With `end`, the same for the
bottom margin of the last in-flow child and the collapsed-through children after the last one with a height
(they move to its bottom edge); when every child is collapsed through, all their margins go. Floats and
out-of-flow boxes keep their margins. The container's own margins are not trimmed. The trimmed margins are
reported as zero in the children's `Layout::margin`. Covered by `crates/pyro-render/tests/margin_trim.rs` and
WPT `css/css-box/margin-trim` (42 of 42 reftests).
The `end` trim reaches into the last child that is not collapsed through when the margins of its own last
children collapse through its bottom edge: that child is laid out again with `BlockContext::trims_block_end`
(stored as `BlockItem::trim_relayout`), which trims them as its own `end` would, and so on down. Its size is
unchanged; its self-collapsing last children move up to the bottom of its last child with a height. Such a
layout is not cached (`compute_block_child_layout` here and in blitz-dom clear the node's cache around it), and
it is skipped when the formatting context has floats, which it would place again. WPT
`block-container-block-end-self-collapsing-children-nested-*`, `-offsets-nested-*` and
`-nested-last-child-with-border` (`block_end_trims_self_collapsing_boxes_nested_at_the_bottom`).

`compute/block.rs`, `compute/grid/alignment.rs`, `style/mod.rs`, `style/block.rs`: `aspect-ratio` as
css-sizing-4 §5 has it, in block layout (`RatioSizes`, `resolve_sizes_with_aspect_ratio`). Upstream applied
the ratio to `size`, `min-size` and `max-size` each on its own, so a limit in one axis overrode a size given in
the other (`width: 100px; aspect-ratio: 2/1; min-height: 100px` was 200px wide), and a box was never taller or
wider than its ratio made it.

- The axis the ratio gives follows the other one after that one's own minimum and maximum. Limits are
  transferred through the ratio only when both sizes are `auto` (block axis to inline axis: the minimum no
  larger than `max-width`, the maximum no smaller than `min-width`).
- The automatic minimum size. A block container whose height follows its ratio (`height_follows_ratio`:
  the height is then not handed down as known; `compute_inner` derives it from the used width, so a float's
  or an inline-block's shrink-to-fit width counts) is as tall as its content when that is taller, up to
  `max-height`, unless `min-height` is set or it is a scroll container. A flex or grid container gets the same
  from its block parent, which measures its content height (`BlockItem::content_min_height_ratio`); not a
  wrapping column flex container, whose lines break at the ratio's height
  (`BlockItemStyle::breaks_lines_in_block_axis`, default false). A box whose width follows its ratio is as
  wide as its min-content width, up to `max-width` (`BlockItem::min_width_is_content`, measured in
  `SizingMode::ContentSize`).
- `CoreStyle::aspect_ratio_of_content_box` (default false): `aspect-ratio: auto <ratio>` on a non-replaced
  box is the ratio of its content box whatever `box-sizing` says (`aspect_ratio_inset`).
- Grid items (`align_and_position_item`): with a ratio, an explicit `stretch` in an axis with an `auto` size
  fills the grid area whatever the ratio gives that axis; the other axis, when `auto` and not stretched,
  follows through the ratio.

Not done: the flex algorithm's own rules for items with a ratio (content size suggestion, flex base size),
absolutely positioned boxes, tables. Covered by `crates/pyro-render/tests/aspect_ratio.rs`; WPT
`css/css-sizing`: 50 more reftests pass.

`compute/block.rs`, `style/block.rs`: sizing keywords in `min-width`, `max-width`, `min-height` and
`max-height` of block-level items. `CoreStyle::min_size`/`max_size` are lengths, percentages or `auto`, so a
keyword there was `auto`; `BlockItemStyle::min_size_keyword`/`max_size_keyword` (default: none) name it, and the
item's block parent applies it: in the inline axis the item's content width under the keyword's constraint
(`clamp_width_by_keywords`, measured in `SizingMode::ContentSize` with the item's height kept, so percentage
heights in it resolve as they will; `stretch` is the stretch width; `fit-content` is the max-content width, at
most the available width but at least the min-content width, from two measures: one measure under the definite
width gave a grid container with `fr` columns its max-content width), also where the parent sizes itself by its
content; in the block axis the content height at the item's width, or the stretch height. A width the preferred
aspect ratio gives from a height counts as an intrinsic width. Out-of-flow boxes, floats, flex and grid items
are not done. Covered by `crates/pyro-render/tests/sizing_keywords.rs`.

`compute/block.rs`, `determine_content_based_container_width`: a percentage in an item's margin resolves against
zero in an intrinsic size, also inside `calc()`: `margin-left: calc(10% + 100px)` counts 100px (the whole
margin was dropped). Same test file.

## Incremental rounding

These hooks went unused for a while. blitz-dom's `writing-mode` feature (#199) replaced `round_layout` with
its own walk, `physicalise_and_round_layout` in `layout/writing_mode.rs`, which had no skip, and pyro-render
enables that feature. The hooks kept compiling and nothing noticed: every layout query rounded the whole
tree, 87-91% of layout time on a paginating document.

The walk now calls `rounded_unchanged` and `record_rounded` itself, so `round_layout` only runs in a build
without the feature. Taffy's side is unchanged, as is `UPSTREAM.patch`. A subtree is skipped when:

- its root is not an anonymous box and has style data (damage flags are not kept on the others);
- no damage on the root or in its subtree. Every unrounded layout change marks damage up the DOM parent
  chain (`Node::update_unrounded_layout`, the only writer). Inline layout used to write these layouts
  directly, so an inline-block or float could move, beside a resizing float, inside a box with no damage;
- the same `LayoutData::rounded_from` key: the root's *physical* unrounded layout (a `vertical-rl` box's x
  depends on its placer's width) and the fractional part of its parent's absolute position. A box that only
  moved is marked on its ancestors, not itself (`Node::update_unrounded_layout`): the key's location tells
  its rounding to follow the move, and a subtree moved by whole pixels (the rows below a table cell that
  grew by a line) is skipped whole: `writing_mode::rounded_layout` rounds positions relative to the pixel
  grid (`parent_frac + location`, rounded half up, less the parent's `parent_frac` rounded), never seeing
  the whole pixels, so moving a parent by them changes nothing it computes. Half up (`-2.5` to `-2`, as
  Blink's `LayoutUnit::Round`) is the rounding that moves with the grid; `f32::round` (half away from
  zero) is not across zero. Taffy's `round_layout` still rounds the cumulative positions and passes them to
  the hooks, whose arguments blitz-dom reads as the fractional parts.

Debug builds round the whole tree again after each pass and assert that it matches
(`assert_rounding_is_current`), so every test checks the skip. `rounding_skips_what_a_change_did_not_touch`
and `rounding_skips_subtrees_moved_by_whole_pixels` in `crates/pyro-render/tests/mirror.rs` fail if the skips
stop firing; `Pyro.geometryStats` reports `roundedNodes` and `roundingSkips`.

`compute/mod.rs`: `compute_root_layout_in` takes an initial containing block of another size than the root's available space (print: Blink's rounds down, the page area up).

`compute/mod.rs`, the root positioning pass: a root whose own layout comes from its cache, and which claims no
out-of-flow box itself, records this pass's hoisted boxes again instead of keeping the last run's. blitz-dom
relays an out-of-flow box whose containing block is the initial containing block without relaying its
ancestors (`POS_CHILD_RELAYOUT` in `layout/damage.rs`), so the root's layout is cached while the box, relaid
here, can surface other candidates (a `position: fixed` box added inside it). A root that claims boxes is
always relaid when one of them changes, and keeps its list. The hunk is in the `src/compute/mod.rs` section of
`UPSTREAM.patch`. Covered by `crates/pyro-render/tests/out_of_flow_boundary.rs`.

## Table grids laid out again from their last layout

`compute/grid/table_reuse.rs` (new), `compute/grid/mod.rs`, `compute/grid/track_sizing.rs`, `compute/grid/alignment.rs`
and `tree/traits.rs`: a grid whose rows are a table's (`grid_rows_are_table_rows`) can be laid out again from its last
layout, sizing again only the rows of the items that changed. Two `LayoutGridContainer` methods, both defaulting to
the whole layout:

- `store_table_grid_state` is offered a `TableGridState` after a `PerformLayout` that can be replayed: its inputs and
  container constants, the columns as sized, the rows as their items sized them (11.5) and as finally sized, the
  items, and where each item was positioned (`alignment::PositionedItem`).
- `take_table_grid_state` hands it back with the indices of the items that changed since (their subtree or their
  style) and the rows replaced since, if any (`TableGridSplice`: rows inserted or removed with their items; rows
  appended at the end need none). The inserted items are placed as the whole grid places them.

With the same inputs, constants and columns, `table_reuse::prepare` keeps the old items and the rows' item-sized
tracks (those after the replaced rows moved to their new indices), makes the changed items again from their style,
and marks their rows (and inserted or re-specified rows) as affected. The inline axis is not sized again: every column is fixed, so the columns depend on no item. The block axis
sizes only the affected rows from their items (`track_sizing_algorithm`'s `only_tracks`), then runs the rest of track
sizing on every row as before. An item that did not change keeps its layout when its grid area is the same, and is
moved (`PositionedItem::moved_to`, its block offset and overflow contribution with it) when only its block position
changed; the others are positioned again.

A state is recorded only where nothing else depends on the items: no intrinsic, percentage or (in an indefinite
width) flexible column, no percentage row, no content-alignment gutter, no collapsed track, no implicit track, no
baseline-aligned item, no item spanning rows or with out-of-flow descendants bubbling up, and no re-run of track
sizing. Otherwise, or with other inputs, the grid is laid out whole. blitz-dom's tables use it (see "Incremental
tables" in `vendor/blitz-dom/README.pyro.md`). `crates/pyro-render/tests/mirror.rs` checks replayed layouts
against fresh builds (`table_*_match_fresh_builds`) and that edits, appends, removals and inserts replay (`a_cell_edit_collects_only_its_row`).

The inputs may differ in the available width and the parent's width (`same_but_for_available_width`, on the
`LayoutInput` and on the constants but for `available_grid_space.width`): the columns a state is recorded with are
fixed tracks, which neither sizes, and whatever of the grid's own style resolves against the parent's width is in
the constants, which must agree. With nothing changed besides (no changed item, no splice, no item appended, the
same tracks), the last layout stands (`table_reuse::stands`): `compute_grid_layout` gives back its `LayoutOutput`,
kept in the state, and stores the state again marked `stood` (blitz-dom counts no layout for it). A shrink-to-fit
table in a container that got wider or narrower, but not past the table's own width, was laid out whole, cell by
cell, for the same result (`tables_keep_their_layout_when_only_the_available_width_changes` in `mirror.rs`).

## The grid's min-content re-check

`compute/grid/mod.rs` and `compute/grid/types/grid_item.rs`: the re-check of the items' min-content widths
after row sizing skips an item the first column pass read nothing from that depends on its content or its
grid area (`GridItem::inline_contribution_consumed`, set when a min-content or max-content contribution is
computed in the inline axis, or a minimum contribution that resolves a percentage or uses an aspect ratio).
Upstream measured every item crossing an intrinsic column and compared it with a cache the first pass never
filled for such items, so an `fr` column holding a `min-width: 0` item re-ran column sizing on every layout.
The columns of the first pass do not depend on a skipped item. The skip is limited to grids of a definite
width, so the inline percentage basis and `fr` expansion space are the same in both passes. The re-run is
not a no-op, though: it estimates the rows from their base sizes and re-reads every other item's max-content
and minimum contributions. So when a skipped item was the only reason for the re-run, the result can change.
That happens when another item's max-content width changes with its row but its min-content width does not.
Before, the re-run picked up the new max-content width. Now the first pass's value stays, the same result
the grid gives without the skipped item, and the same as css-grid-1 §12.1 step 3, which re-runs only on a
changed min-content contribution. `an_unread_item_does_not_change_the_other_columns` pins this. Covered by the `rerun_tests` module at the end of `compute/grid/mod.rs` (run them from a
copy of this directory with an empty `[workspace]` table, as the crate is not a workspace member).

## A tree placing its own grid items

`compute/grid/mod.rs` and `tree/traits.rs`: a tree can place a grid's items itself.
`LayoutGridContainer::track_counts` (default `None`) answers how many columns and rows the placed items reach,
and `resolved_placement` each in-flow item's area in origin-zero lines. The grid then skips named-line
resolution, the grid size estimate, auto-placement and its occupancy matrix, and sizes the explicit tracks
plus whatever the items reach past them (no negative implicit tracks). Empty `auto-fit` tracks are found from
the items' areas. blitz-dom's tables answer it from `TableContext::placement`, computed once as the table is
built (`place_cells` in `layout/table.rs`, the same `row dense` placement). A table grid laid out again from
its last layout (`table_reuse::prepare`) takes the inserted rows' items from it as well, instead of placing them
over an occupancy matrix of the whole grid. In debug builds `debug_assert_tree_placement` runs the grid's own
placement as well and asserts the areas and track counts are equal, and the replay asserts the same for the
inserted items.

## Small stack frames per level of block layout

`compute/block.rs`: block layout recurses through `compute_block_layout` -> `compute_inner` ->
`perform_final_layout_on_in_flow_children` -> the child's layout, once per level of a nested tree.
`generate_item_list` and `determine_content_based_container_width` are no longer inlined into `compute_inner`
(their locals are dead by the time it recurses), and a box that establishes a block formatting context
creates it in `compute_inner_in_new_bfc` (not inlined) instead of in the frame of `compute_block_layout`
that every block pays for. About 0.3 KB less per level (blitz-dom's README has the whole picture). No
behaviour change.

## A block container's last baseline

`compute/block.rs` (`perform_final_layout_on_in_flow_children`): a block container's last baseline is the last
baseline of its last in-flow child that has one, moved with `align-content` as the first is; only the first was
set, so an inline-block (whose baseline is its last line box's, CSS 2.1 § 10.8.1) sat on its first line. Covered by
`crates/pyro-render/tests/block_in_inline.rs`.

## Table cells aligned on their row's baseline

`compute/grid/table_baseline.rs` (new), `compute/grid/mod.rs`, `compute/grid/types/grid_item.rs` and `tree/traits.rs`
(`table_has_baseline_cells`, `table_cell_is_baseline_aligned`, `table_cell_baseline`,
`set_table_cell_baseline_offset`, all defaulting to none): in a table's grid, the cells aligned on their row's baseline
(`vertical-align: baseline`, CSS 2.1 §17.5.3; spanning one row) are laid out at their columns' width before the rows
are sized (again after a re-run of column sizing). In a row with two or more of them, each one's baseline (the tree's
`table_cell_baseline`: its content's first baseline, else the bottom of its content box when it has content); the
row's is the lowest, and each cell's
`baseline_shim` the row's less its own (a cell with neither takes no part). The shim counts in the row's height as a
margin does, but the cell is positioned as any other, filling its row: the tree moves its content down by the shim
inside it (`set_table_cell_baseline_offset`, called before each cell's final layout, 0 for the cells without a
shim; never in a table without such cells, where the tree drops stale offsets itself), as Chromium's intrinsic
padding does. blitz-dom hands a table with such cells its `TableGridState` back
only while nothing in it changed (a change may move other cells' baselines).
Covered by `baseline_cells_share_their_rows_baseline` in `crates/pyro-render/tests/table_parts.rs`.

## Fixed-layout cells narrower than their padding

`compute/grid/mod.rs`, `compute/grid/alignment.rs` (`align_and_position_item`'s `width_unfloored`),
`compute/grid/types/grid_item.rs` and `tree/traits.rs` (`table_cells_narrower_than_padding`, default false): the
cells of a table that answers true (blitz-dom: the fixed table layout algorithm) are as wide as their columns, even
where those are narrower than the cells' horizontal padding and borders: their width is not floored at them (as in
Chromium; `fixed_table_cells_can_be_narrower_than_their_padding` in `table_parts.rs`).

## Moving from v0.15.1 to main

The local commit was cherry-picked onto main (base `v0.15.1`, a release branch). Upstream has none of the
changes above, so all are kept. Three places needed more than a merge:

- `compute/block.rs` (`generate_item_list`): main caps the min-content contribution of compressible
  replaced children (#1401, `min_content_width_cap`). `block_child_width_fits_content` still makes a
  child's auto width its content size, but the cap stays with `is_compressible_replaced` as upstream
  defines it, so form controls' contributions are what they were.
- `compute/grid/alignment.rs` (`PositionedItem`): main measures an item's scrollable overflow contribution
  from the container's padding-box inset and a `ScrollableOverflowContainer` (#1372, reversed scroll
  origins). The kept item stores both instead of the old RTL x and top border, so `moved_to` measures as
  `align_and_position_item` does. `ScrollableOverflowContainer` derives `Clone, Copy, Debug` for it.
- `compute/grid/mod.rs`: the replayed table grid passes the new `align_and_position_item` arguments.

Upstream's float and block-formatting-context fixes since v0.15.1 (#1384, #1385, #1386, #1387, #1389, #1392,
#1394, #1395, #1397, #1401) and `ScrollableOverflow` (#1372) come with the update; blitz-dom main uses them.

## Floats and the boxes that avoid them (WPT reftests, lane P3)

- `compute/block.rs` (`perform_final_layout_on_in_flow_children`): a box that avoids floats (a table, a new
  formatting context, a block-level replaced element) goes to the first band where its border box fits. Its
  narrowest border box in a band is what it needs: a table, a replaced element or a box of an intrinsic width is
  laid out in the band (the inputs its layout gets if the band stands, so that layout is cached), any other auto width
  shrinks to the band, down to its padding and border, its `min-width` and what its negative margins prop it up to. A box laid out in a band that is taller than the band stays as wide is
  placed again beside the floats of its whole height (`find_bfc_slot` with that height, at most 8 times): upstream
  checked only the styled height, so a box beside a short float overlapped the wider float below it (WPT
  `floats-wrap-bfc-004`, `floats-bfc-003`, `table-among-floats-001`, `floats-wrap-bfc-with-margin-003`).
- `compute/block.rs` (`generate_item_list`): a block-level replaced element or form control
  (`is_compressible_replaced`, the tree's `block_child_width_fits_content` included) is not in its parent's block
  formatting context: it keeps clear of floats as a table does (CSS 2.1 § 9.5; WPT `floats-039`).
- `compute/float.rs` (`place_floated_box_inner`, `find_content_slot`, `find_bfc_slot`): clearance searched from the
  segment after the cleared floats' last one (`last_placed_floats` ends are exclusive; `+ 1` skipped one more), so a
  float clearing one side was pushed below the next float on the other side too (WPT `floats-placement-005`). The
  `y` threshold (`clear_bottoms`) still does the clearing.
- `compute/float.rs` (`find_bfc_slot`, `Segment::has_wide_float`): zero-width floats alone beside a box leave its
  margins to apply as if there were none (WPT `zero-width-floats`); beside a float of some width a zero-width float
  absorbs its side's margin, as before (WPT `zero-width-floats-positioning.tentative`).

Beside a float, negative margins on a side with no float no longer widen the band (`find_bfc_slot`): the box is no
wider than the band, and one that they make wider moves down (Chromium, measured; WPT `floats-wrap-bfc-with-margin-001`,
`-001a`, `-002`, which every browser passes). This loses `floats-wrap-bfc-with-margin-006/007.tentative`, which assert the
opposite and every browser fails (`crates/pyro-render/tests/floats.rs`). The side with no float is bounded by the
containing block alone: its segment inset (a float that ended, in another container) is no edge (pyro-layout
`flow::floats_like_chromium`, `#c6`).

Not done: `max-height` separating a
last child's bottom margin (WPT `max-height-separates-margin`, Chromium and Gecko): `margin-collapse-038` (Safari only,
which pyro passes) expects the margins to collapse. Covered by `crates/pyro-render/tests/floats.rs`.

## Inline layout leftovers (WPT reftests, lane P5)

- `src/tree/layout.rs` (`OofCandidate::containing_block`), `src/compute/oof.rs` (`perform_oof_layout`): a candidate
  can carry its containing block, when that is no layout node's (the padding box an inline box's fragments bound):
  the first positioning pass that meets it lays it out against that area, not against the node's (nor a grid area).
- `src/tree/traits.rs`, `src/compute/block.rs`: three hooks of `LayoutBlockContainer`.
  `block_container_passes_percentage_height`: a container that is no containing block (an inline box laid out
  around its blocks) hands its own containing block's height on as the percentage basis.
  `block_container_direction`: such a container lays its children out in its containing block's direction.
  `block_child_flow_overlap` / `set_block_flow_overlap`: the final layout records whether in-flow children overlap
  in the block axis (one starts above the end of an earlier one, or follows one whose content is taller than its
  box, or a child recorded an overlap) and whether the container's own content is taller than its box.
- `src/compute/block.rs`: the last baseline a child that is a scroll container gives its container is its bottom
  margin edge, whatever lines it holds (as browsers place an inline-block on it; WPT
  `baseline-block-with-overflow-001`).

Covered by `crates/pyro-render/tests/inline_leftovers.rs`.

## Percentage heights in a multi-column container's strip (WPT round 3, lane SM)

`src/tree/traits.rs` (`LayoutBlockContainer::block_container_percentage_height`), `src/compute/block.rs`: a
container whose own height is not known can name the border-box height its children resolve percentage heights
against. blitz-dom names its multi-column container's column height while it lays out the strip.

## Initial letters (WPT round 3, lane I)

- `src/compute/float.rs` (`FloatContext::place_initial_letter`, `initial_letter_clearance`), `src/compute/block.rs`
  (`BlockContext::place_initial_letter`, `initial_letter_clearance`): the margin box of an initial letter
  (`initial-letter`, CSS Inline 3 § 7) is an exclusion at the place its line gives it: content keeps out of it as
  of a float, but the float placement rules (no float above an earlier one, `clear`) do not apply to it. Floats
  are placed below the initial letters on their side, and the first line of a block that starts with an initial
  letter below every earlier one (Blink's `InitialLetterClearanceOffset`). Segments inserted for it keep the
  segment ranges of the last placed floats in step (`shift_last_placed_floats`).

Covered by `crates/pyro-render/tests/initial_letter.rs`.

## Subgrids (WPT round 3, lane G)

`compute/grid/subgrid.rs` (new), `compute/grid/mod.rs`, `compute/grid/alignment.rs`, `compute/grid/track_sizing.rs`,
`compute/grid/types/grid_item.rs`, `compute/grid/types/named.rs`, `style/grid.rs`, `tree/traits.rs`: css-grid-2 §9.
A grid item is a subgrid in an axis when `GridItemStyle::is_subgrid` says so (its template there is `subgrid`, it is a
grid container in flow with neither layout nor paint containment, in its parent's orientation). The grid then:

- gives it a `SubgridContext` through the tree (`LayoutGridContainer::set_subgrid_context`; the default keeps nothing,
  and a `subgrid` template lays out as `none`): per subgridded axis the spanned track count, the names the grid's lines
  (and its named areas the subgrid overlaps) give the subgrid's lines, the subgrid's gap (its own, or the grid's for
  `normal`), whether its direction is the other one, and once sized the subgrid's tracks: the spanned tracks less the
  subgrid's margin, border and padding at its edges and half its gap's difference to the grid's gutter between them, as
  sizes and as spans from its content box (a track can end before it starts when the gap is wider; the next track still
  starts on the grid's line);
- sizes its tracks with the subgrid's items in the subgrid's place ("virtual" items, `GridItem::is_virtual`, through
  nested subgrids too), each with that margin, border and padding at the subgrid's edges and that half gap difference
  elsewhere as an extra margin (`GridItem::extra_margin`). The subgrid itself takes no part in sizing in its subgridded
  axes (`GridItem::excluded`); a virtual item takes none in the axes its subgrid has tracks of its own. Where no item of
  the subgrid lies at an edge (or it has none), an item of no size in its edge track alone carries the edge's margin,
  border and padding, so the empty tracks between it and the edge-most item stay empty (as Blink, measured: WPT
  `subgrid-no-items-on-edges-001`, `subgrid/grid-gap-002`, `orthogonal-writing-mode-002`, `-003`;
  `crates/pyro-render/tests/subgrid_edge_tracks.rs`). A percentage gap of the subgrid counts as zero for its parent's
  track sizing and resolves against the subgrid's content box once the parent's tracks give it
  (`update_subgrid_track_sizes`; Blink, measured: items may overflow; WPT `subgrid/grid-gap-008`). The virtual items
  are dropped before the items are positioned;
- stretches the subgrid in its subgridded axes, whatever its alignment, size, minimum and maximum size and aspect ratio.

The subgrid's own layout places its items as the grid placed them for sizing (`place_subgrid_items`): the spanned
track count is its explicit grid in a subgridded axis, with no implicit tracks (items' definite lines are clamped to it before auto-placement, `clamp_placements`, and the rest after it), its line
names are its `<line-name-list>` (`subgrid_line_names`, `repeat(auto-fill)` repeated to fit) plus the inherited ones,
and a subgrid child's automatic span is the number of tracks its own list names. Its tracks there are fixed at the
given sizes, `justify-content`/`align-content` do not apply, and after alignment they are placed at the given spans.
A subgrid in its rows only records its own columns (`SubgridContext::own_columns`) and its parent, before sizing rows,
lays it out at its width if needed (`resolve_standalone_widths`): its items' heights depend on its columns. The resolved
value of a subgridded axis is `subgrid` with each line's own names (`DetailedGridTracksInfo::subgrid`).

A subgrid in an orthogonal flow matches its parent's axes swapped: `GridItemStyle::subgrid_axis` names its own axis for
each of the parent's, `subgrid_reversed` whether its tracks there run the other way (stylo_taffy compares the writing
modes' track progressions), and the context, the virtual items and the track sizes are mapped through them (subgrids
nested in an orthogonal one are left to it).

Not done: baseline alignment across a subgrid, the subgrid's own rows for a subgrid in its columns only (its items'
column contributions see an indefinite height). Covered by
`crates/pyro-render/tests/subgrid.rs`; WPT `css/css-grid/subgrid`: 36 → 63 of 102 reftests.

## Natural aspect ratios of flex items (WPT round 3, lane G)

`style/mod.rs` (`CoreStyle::natural_aspect_ratio`, default none), `compute/flexbox.rs`: a replaced element's natural
ratio is its preferred aspect ratio when `aspect-ratio` is `auto` (css-sizing-4 §5.1), between content boxes. The flex
algorithm knew only the property's ratio (`aspect_ratio`, in the box-sizing box), and a replaced item's ratio was left
to its measure function, which a min-content measurement does not transfer through. Now (`FlexItem::natural_ratio`):

- a definite preferred cross size, clamped by the item's minimum and maximum cross sizes, gives its main size through
  the ratio, so also its flex base size and its transferred size suggestion (the automatic minimum is the smaller of
  it and the content size suggestion; css-flexbox-1 §4.5): an image with `height: 100px; max-width: 150px` and a
  natural 200x200 size is 100px wide, not 150px (WPT `flex-aspect-ratio-img-row-012`, `flex-minimum-width-flex-items-011`);
- a definite cross size (a stretched one) gives the flex base size (§9.2 B), and a definite cross size (preferred or
  stretched) gives the content size suggestion through the ratio, smaller or larger than the natural size, as Blink
  measures a replaced element's min-content size with a definite block size (`relayout-intrinsic-block-size`,
  `flexbox-min-width-auto-002a`, `-002c`).

The property's ratio and every other use of `aspect_ratio` are unchanged. Covered by
`crates/pyro-render/tests/flex_natural_ratio.rs`.

A stretched item with either ratio and no definite cross size (its container's cross size is not definite, e.g. a
column container sized to its content) is measured for its flex base size at its fit-content cross size, the available
space only its limit (§9.2 E): it was given the available space as its cross size, so a float or `inline-flex` column
around an image or an `aspect-ratio` box was as wide as its containing block and the item as tall
(WPT `flex-item-transferred-sizes-padding-*`, `inline-flex-column-image-load`).

## Static positions and positioned table parts (WPT css-position, round 3 lane P)

- `compute/block.rs`: an inline-level out-of-flow child of a block container (`BlockItemStyle::
  original_display_is_inline`) has the static position of the start of a line at its place: beside the floats
  there (`BlockContext::find_content_slot`), where `text-align` puts the line's content (`BlockContainerStyle::
  text_align_edge`), as Blink's `HandleOutOfFlowPositioned` (WPT `inline-level-absolute-in-block-level-context-*`,
  `position-absolute-dynamic-static-position-floats-*`).
- `tree/traits.rs` (`LayoutContainingBlock::static_position_is_containing_block`), `compute/oof.rs`
  (`layout_oof_box`): a box whose static-position rectangle is its whole containing block (an element in the top
  layer, css-position-4) is aligned in it by its self-alignment (WPT `top-layer-box-uses-icb-*`).
- `compute/block.rs`: a table child gives a block container no last baseline (an inline-block holding only tables
  sits on its bottom margin edge, as in Chromium; WPT `position-relative-table-*`).
## Tables: a height is a minimum; tables as flex items (WPT round 3, lane G)

`compute/grid/mod.rs`: a table grid (`grid_rows_are_table_rows`) is at least as tall as its rows, whatever height it
was given (CSS 2.1 §17.5.3): its `height`, or the height a flex or grid parent gave it. The rows were laid out taller and
the table box kept the smaller height: `<table style="height:20px">` around a 50px row was 20px tall, and the next block
overlapped the row.

`compute/flexbox.rs`, `style/flex.rs` (`FlexboxItemStyle::is_table`, default false): a table flex item is never smaller
than its min-content size in the main axis, as in Blink: its resolved minimum main size is at least its min-content
width (row) or its rows' height at its width (column), whatever its `min-width`/`min-height`, and its maximum main size
is raised to it (WPT `table-as-item-fixed-min-width-3`, `table-as-item-min-content-height-1/2`, `ortho-table-item-001`).
Covered by `crates/pyro-render/tests/table_flex_items.rs`.

## Sizing keywords in flex items' minimum and maximum sizes (WPT round 3, lane G)

`compute/flexbox.rs` (`resolve_size_keyword`), `style/flex.rs` (`FlexboxItemStyle::min_size_keyword`/`max_size_keyword`,
default `auto`): `min-content`, `max-content`, `fit-content` and `stretch` in a flex item's `min-width`, `min-height`,
`max-width` or `max-height` (which `min_size()`/`max_size()` give as `auto`) resolve by measuring the item in that axis
under the keyword's constraint (`fit-content` from the min-content and max-content sizes, clamped by the available size,
as block items'), or to the stretch size: in the cross axis before the item's sizes are transferred, in the main axis
before its minimum and hypothetical main sizes; a stretched item is clamped by its resolved maximum cross size (WPT
`flex-item-max-width-min-content`, `flex-item-max-height-min-content`, `flex-item-min-width-min-content-max-width`,
`flex-item-min-height-min-content-overflow`). Block items had them (lane S); grid items and out-of-flow boxes still do
not. Covered by `crates/pyro-render/tests/flex_size_keywords.rs`.

## `last baseline` in flex rows (WPT round 3, lane G)

`style/alignment.rs` (`AlignItemsKeyword::LastBaseline`, `AlignItems::LAST_BASELINE`), `compute/flexbox.rs`: the items
of a flex row with `align-self: last baseline` (no `auto` cross margin) are aligned on their last baselines from the
line's cross end, as the first-baseline group is from its start (css-align-3 §9.3): each item's distance from its last
baseline (synthesized at the bottom border edge without one, or for a scroll container) to its outer cross end, the
group's largest makes room in the line's cross size, and each item sits that much above the end less its own.
stylo_taffy mapped `last baseline` to `end` before. In columns, wrap-reverse lines, grids and for absolutely positioned
boxes it still behaves as its fallback, `end`. Covered by `crates/pyro-render/tests/flex_last_baseline.rs`.

## `visibility: collapse` on flex items (WPT round 3, lane G)

`compute/flexbox.rs` (`measure_collapsed_item_struts`, `exclude_collapsed_items`), `style/flex.rs`
(`FlexboxItemStyle::is_collapsed`, default false): step 10 of the flex layout algorithm (css-flexbox-1 §4.4, §9.4),
a TODO before. A container with collapsed items first runs steps 5 to 9 with every item in, each collapsed item's strut
the cross size of its line then. The layout then collects the collapsed items into lines with no main size and no gaps
around them, takes them out of their lines (each moves to its line's end, out of the line's slice), keeps each line at
least as tall as its struts after step 8, and lays the collapsed items out hidden as `display: none` children. Covered by
`crates/pyro-render/tests/flex_collapse.rs`.

## Percentage gaps of a column with an indefinite height (WPT round 3, lane G)

`compute/flexbox.rs` (`size_container_main_axis`): a column container's percentage gaps are not resolved again against
the height its content gives it; they stay as they resolved against its indefinite height, zero, as in Blink. Resolved
again, they pushed the items past the container's end (WPT `css/css-flexbox/gap-percentage-wrap`). A row's are still
resolved again against its main size, which a row's later layout knows anyway. Covered by
`crates/pyro-render/tests/flex_gaps.rs`.

## An `auto` grid height through a preferred aspect ratio (WPT round 3, lane G)

`compute/grid/mod.rs`: a grid container with `aspect-ratio` and an `auto` height (none known or preferred) is as tall as
its used width makes it through the ratio (between content boxes for `box-sizing: content-box`), and at least as tall as
its rows unless `min-height` is set or it is a scroll container (the automatic minimum size, css-sizing-4 §5.1). It was
as tall as its rows (WPT `css/css-grid/grid-definition/grid-auto-repeat-aspect-ratio-001`).
- `compute/oof.rs` (`layout_oof_box`), `style/mod.rs` (`OofItemStyle::min_size_keyword`, `max_size_keyword`): sizing
  keywords in the minimum and maximum sizes of an absolutely positioned box (`max-height: fit-content`, ...) bound it
  as the keyword would size it, the widths first, the heights at the width (WPT `position-absolute-fit-content`).
- `tree/traits.rs` (`LayoutBlockContainer::block_container_percentage_height`), `compute/block.rs`: a height
  percentage heights of a container's children resolve against when the container's own is unknown as it is laid
  out (blitz-dom's multi-column strip: the container's definite height; WPT `position-sticky-child-multicolumn`).
- `style/mod.rs` (`Style::collapsed_rows`), `style/grid.rs` (`GridContainerStyle::collapsed_rows`),
  `compute/grid/explicit_grid.rs` (`initialize_grid_tracks`): explicit row tracks the container marks collapsed (a
  table's `visibility: collapse` rows, from blitz-dom) are collapsed tracks with one gutter each, the one after them
  (before them when no open row follows), so a collapsed row takes its border spacing with it (WPT
  `css-tables/visibility-collapse-border-spacing-001/002`).
- `compute/block.rs` (`determine_content_based_container_width`): a table with a `width` or `max-width` contributes at
  least its min-content width (its content measured without them): a table is never narrower than its content (WPT
  `css-tables/table-intrinsic-size-001`).
**Percentages in intrinsic widths and against a content width (lane S2).** Block `compute_inner` hands its definite
percentage basis for heights (own height, the ratio's, or the passed-through one) to the children it measures for
its content width (`determine_content_based_container_width`'s `percentage_height`): a float of `height: 100px`
around a `height: 100%` canvas is as wide as the canvas at that height. When the container's width came from its
content (no known width: floats, inline-blocks), the item list is generated again with that width, so the children's
percentage sizes, min/max sizes and padding resolve against it, as at a given width. Covered by
`crates/pyro-render/tests/intrinsic_percentages.rs`.

**Flex items with a preferred aspect ratio (lane S2).** A preferred cross size gives the main size through the ratio
at item generation, but a preferred main size gives no cross size: the cross size follows the flexed main size.
A non-replaced ratio item's content size suggestion is the larger of its measure with the definite cross size (the
ratio's) and its content's min-content size measured without it (the cross size as available space); the specified
size suggestion is the preferred main size only (no transferred one for non-replaced items), and min cross sizes
bound it through the ratio. A replaced item's `aspect-ratio` converts a definite cross size like a natural ratio.
`FlexItem::ratio_inset` makes transfers between the boxes the ratio is between (content box for `content-box` or
`auto <ratio>`). Block layout follows the ratio in content-size measures too (`height_follows_ratio` without the
sizing-mode condition: a max-content flex base size of `min-width: 100px; aspect-ratio: 1` is 100px tall), and
derives a known axis through the ratio's box. Covered by `crates/pyro-render/tests/aspect_ratio.rs`.
- `src/compute/block.rs` (`set_block_flow_overlap`): a container whose last in-flow child's content overflows it
  records that its own content overflows too, so the overflow reaches past nested blocks to the container that paints
  them in Appendix E phases (WPT css-inline `text-box-trim-accumulation-002/003/004`: trimmed glyphs over the next
  block). Covered by `a_spans_border_nested_in_blocks_is_painted_over_the_next_blocks_background` in
  `crates/pyro-render/tests/text_box_trim.rs`.

**Grid baseline alignment in both axes (lane GB).** `compute/grid/baseline.rs` (new) replaces
`track_sizing::resolve_item_baselines`: items with `align-self` or `justify-self` `baseline` / `last baseline` form
baseline-sharing groups per track and side (css-align-3 §9.1): the start side for a first baseline, the end side for
a last one, swapped when the item's block flow runs the other way (`GridItem::baseline_flow`, from
`GridItemStyle::subgrid_axis`/`subgrid_reversed`: `vertical-rl` in `vertical-lr`, or an orthogonal item whose
block-start is the grid's inline-end). Items whose block axis is perpendicular to the alignment axis, or without a
baseline, get one synthesized at `GridContainerStyle::baseline_synthesis` (line-under edge, middle for central
baselines). Orthogonal items' baselines survive `LayoutOutput::transpose` as `Baselines::orthogonal_first/last`.
Shims sit on the group's side in both axes (`baseline_shim`, `baseline_shim_block_end`, `baseline_shim_inline`), count
in contributions and are computed per axis as it is sized (with the other axis' sizes when known), then again with
the final sizes before `align_and_position_item`, which aligns the item to its group's side
(`ItemBaselineAlignment`). A grid container reports a last baseline (its last row: a last-baseline group, else its
last item).
**Stretch beside floats and for absolutely positioned boxes (lane S2).** Tables and compressible replaced elements
resolve `stretch` in `width`, `min-width` or `max-width` through the block parent (`block_item_known_dimensions`),
and the float-slot search clamps a given width by the keyword limits (`clamp_width_by_keywords`), so `stretch` is
the slot's width. `resolve_absolute_sizing_keywords` takes the stretch-fit size of the inset-modified containing
block, which an auto inset bounds at the static position.
Subgrids (css-grid-2 §9): a subgrid's items (virtual items) join the parent's baseline groups, orthogonal subgrids'
with their alignment axes swapped (nested, reversed: fallback alignment). `subgrid::record_subgrid_baselines` gives
the subgrid each line's group baseline and the parent's `baseline_synthesis` (`SubgridAxisContext::baselines`,
`baseline_synthesis`), which its own final shims take.
`compute/grid/types/named.rs` (`add_subgrid_line_names`): in a subgridded axis, the edges of the subgrid's own named
areas past its last line are on its last line, and the parent's area names reach a subgrid that spans the area in
that axis whatever rows (columns) it is in (`area_names_for_subgrid`), as Blink (WPT `subgrid/line-names-*`).
Column-only subgrids (css-grid-2 §9 standalone axis): a subgrid in its columns only records its own rows
(`SubgridContext::own_rows`, `subgrid::record_own_rows`); before sizing its columns the parent lays out its
column-only subgrid children at their definite (else max-content) height and gives their items the rows they span
(`resolve_standalone_heights`, `GridItem::standalone_size.height`), so an orthogonal item's width follows its row
(WPT `subgrid/standalone-axis-size-010`). A subgrid stretched over rows of fixed sizes is measured at their height,
so a nested one sizes its own rows in what it gets (`-011`). The items of a nested subgrid in both its axes (no tracks
of its own) belong to its parent subgrid's tracks, their owner lines shifted to where it lies there
(`collect_virtual_items`), so they and chains of such subgrids reach the subgrid with rows of its own (`-012`, `-013`).
`crates/pyro-render/tests/subgrid_edge_tracks.rs`.


**Flex content sizes (lane FO).** A block's content size (`SizingMode::ContentSize`) ignores its own `min-width`,
`max-width`, `min-height` and `max-height`, as inline layout's does, unless the box has a ratio (its height follows
the width they give): a flex base size is the content's. Keyword `min-width`/`max-width` (`clamp_width_by_keywords`)
bound an automatic width's intrinsic contribution too. In flex: a column's automatic size counts each item's
hypothetical main size with the minimum winning over a smaller maximum; a column's min-content size under no max
size is one line's (`collect_flex_lines`); `fit-content` in a min/max with nothing to fit into is max-content;
percentage row gaps resolve only against a definite height; an overflowing `space-between` (not explicitly safe)
falls back to plain `flex-start` (csswg-drafts#11937). Covered by `crates/pyro-render/tests/flex_content_sizes.rs`.

`style/mod.rs` (`CalcSizeFunction`, `CoreStyle::calc_size_min`), `compute/flexbox.rs` (automatic minimum main size),
`compute/block.rs` (`ratio_min_content_width`, `BlockItem::calc_size_min_width`, the content minimum of a box whose
height follows its ratio): `min-width`/`min-height: calc-size(auto, <calculation>)` is the calculation of the
automatic minimum size those compute (WPT css-values `calc-size-flex-*`, `calc-size-aspect-ratio-003..005`).

`util/resolve.rs` (`maybe_resolve_min_size`), `compute/block.rs`: a `min-width`/`min-height` `calc()` against an
indefinite percentage basis resolves its percentages as zero, as in Blink (`calc(25px + 50%)` is 25px; a plain
percentage stays none): WPT css-values `calc-min-height-block-1`, `calc-min-width-block-intrinsic-1`.
`compute/mod.rs` (`compute_in_flow_root_layout`, `CoreStyle::is_content_replaced`): a root replaced by its `content: url()` is not stretched to
the viewport's width; it takes its natural size (WPT css-content `element-replacement-root`).
In a row's intrinsic size each item's share is clamped by its max main size and floored by its resolved minimum
(an inflexible basis above `max-width` counts the max). A table in a row bases on its layout in the container's
width (fit-content, percentage columns taking their share), whatever its `flex-basis` keyword, as in Blink.

Forced clearance (a float adjoining the strut a cleared box's top margin would collapse into) is read before the
box is laid out: the box's own floats, merged into that strut by its layout, forced clearance on it and moved it up
to their bottom, over the box before it (`clear: both` blocks of a fixed height holding floats overlapped).

A wrapping column's unstretched item with an automatic cross size shrinks to fit its line's cross size once that is
known (fit-content, its main size kept; `determine_used_cross_size`), and a wrapping column without a cross size of
its own in definite space fits its lines into that space, at least its widest line (`determine_container_cross_size`).
`compute/grid/types/grid_track.rs` (`collapse_after_sizing`, `collapse_sized`), `compute/grid/explicit_grid.rs`,
`compute/grid/mod.rs`: a collapsed table row (`Style::collapsed_rows`) is sized as any other row, so a cell spanning it
gives it its share, and loses its size after each row sizing pass (CSS 2.1 §17.5.5, Blink; WPT css-align
`blocks/align-content-table-cell-002..005`). Its gutter still collapses up front. Tables with one are not reused
(`table_reuse`).

`style/mod.rs` (`CoreStyle::root_float_is_right`), `compute/mod.rs` (`compute_in_flow_root_layout`): a floated root
element shrinks to fit (min/max-content against the initial containing block) and sits on its side (WPT CSS2
`floats/float-root`). `compute/block.rs`: a flex or grid scroll container child gives its block parent its contents'
last baseline; only a block one gives its bottom margin edge (WPT css-align `baseline-of-scrollable-1b`).

`util/resolve.rs` (lane SP): `LengthPercentageAuto::resolve_or_zero` resolves a `calc()` against an indefinite basis
with its percentages as zero (Blink's `MinimumValueForLength`): a `calc(10% + 100px)` margin is 100px in an
intrinsic size (WPT css-sizing `calc-margins-flex`). Padding keeps zero (css-flexbox `dynamic-isize-change-004`).
`style/block.rs` (`BlockContainerStyle::own_height_keywords`), `compute/block.rs` (`compute_inner`, lane SP): a block
container's own `min-height`/`max-height` content keywords clamp its height to its content (atomic inlines, floats,
out-of-flow boxes; block items get them from their parent too).
- `compute/grid/track_sizing.rs` (`initialize_track_sizes`): a `calc()` gutter against an indefinite size keeps its
  lengths, its percentages resolving to 0 (`gap: calc(20px + 5%)` of an inline grid; WPT css-grid `grid-gutters-009`).
- Round 4 lane B: `compute/block.rs` `determine_content_based_container_width` counts percentage margins of the items as
  zero in the contribution also under a definite available width (shrink-to-fit abspos/floats/inline-blocks; WPT
  css-sizing `abspos-auto-sizing-fit-content-percentage-*`); a width the ratio gives from a definite height is the
  item's `min-content`/`max-content`/`fit-content` width (`clamp_width_by_keywords`), and a height the ratio gives from
  the width ignores the box's own content keyword min/max heights; a `min-width`/`min-height` keyword (which reads as
  `auto` in `min_size`) turns off the ratio's content-based automatic minimum (`aspect-ratio/intrinsic-size-010`).
  `tree/traits.rs` `LayoutBlockContainer::block_container_fill_height`: the least border-box height of an `auto`-height
  block container that its children's `stretch` and percentage heights resolve against (the quirky html/body).
- Round 4 lane B (2): `compute/grid/mod.rs` re-runs column/row sizing at the definite container size when the flexible
  tracks' factors sum to less than one (the first pass counted the sum as one against the indefinite size; WPT css-grid
  `grid-template-flexible-rerun-track-sizing`, `flex-sizing-rows-indefinite-height*`); `track_sizing.rs`: an item
  spanning several tracks, a flexible one among them, gives them only its minimum contribution under a content
  constraint (`grid-flex-spanning-items-001`). `compute/flexbox.rs`: `stretch` as the cross size makes the item's cross
  size definite for its flex base and automatic minimum whatever `align-self` (css-sizing
  `stretch/flexbox-auto-minimum-002`); a replaced item with a natural ratio in a row measures with the container's
  width as its main parent size, so a ratio-only one stretch-fits it (css-flexbox `svg-root-as-flex-item-003`).
- Round 4 lane B (3): `compute/flexbox.rs` `determine_container_main_size`, single-line row under a definite available
  width (shrink-to-fit): an item's min-content contribution is its min-content size, at least its flex base size under a
  min-content constraint (a content basis is the min-content size, §9.2.3 C) when it cannot shrink, at most that basis
  when it cannot grow, then at least its resolved minimum (Blink's row algorithm; WPT css-flexbox
  `intrinsic-size/row-001`, `row-004`). Before, inflexible items counted their max-content basis and shrinkable ones only
  their resolved minimum.

Round 4 lane E: `compute/block.rs`: a known height that is no percentage basis (`known_dimensions_are_definite`
false: a table cell's stretched height) leaves the box's own clamped `height` as the basis
(`block_container_restricts_percent_scrollers` boxes only); the intrinsic-width pass reads
`block_container_percentage_height` too; a percentage-height scroll container whose percentage does not resolve in
a container that `block_container_restricts_percent_scrollers` (a cell of a table of non-`auto` height) is sized as
of height 0, then clamped (css-tables `percentage-sizing-of-table-cell-007`). `tree/traits.rs`: the new hook.
- Round 4 lane B (4): `compute/grid/mod.rs`: a preferred height the ratio gives from the width is from the width
  between `min-width` and `max-width`, its final value the automatic one (at least the rows), and `max-width` is not
  transferred to a maximum height (WPT css-grid `grid-content-distribution-029`). `compute/oof.rs`: an absolutely
  positioned box with a preferred aspect ratio and automatic sizes takes the shrink-to-fit width between its
  (transferred) limits and the ratio's height from it; a size the ratio gives from the other axis is at least the
  content's when `min-*` is `auto` (css-sizing `aspect-ratio/abspos-008/012/013`; `abspos-017`'s `min-width: 0` keeps
  the ratio).
- Round 4 lane F: `compute/block.rs` `perform_final_layout_on_in_flow_children`: a block container's
  `Baselines::dominant_first`/`dominant_last` come from the children that give its first and last baselines (none from a
  scroll container or a child without one), so an inline-block whose lines sit in a nested block still has a central
  baseline in a vertical line. `compute/flexbox.rs` `calculate_children_base_lines`: in a container on a central baseline
  (`FlexboxContainerStyle::block_baseline_synthesis` 0.5: vertical writing modes, text not sideways) an item's baseline is
  its central one (`dominant_first`), else its baseline, else synthesized in its middle (WPT css-flexbox
  `baseline-synthesis-004` with vertical lines that centre their atomic inlines). Such a row container exports the same
  item's central baseline as its `dominant_first` (`FlexItem::dominant_baseline`; WPT `synthesize-vrl-baseline`).

Round 4 lane G: flexbox transfers min/max sizes through an item's ratio via its ratio inset (`transfer_through_ratio`;
WPT css-sizing `flex-aspect-ratio-025/026`), and a column container of indefinite width measures a non-replaced ratio
item without its content-based height (no definite basis or height, no growing) for its hypothetical width
(`flex-aspect-ratio-026`).
- Round 4 lane G: grid items honour `min-width: min-content | max-content` (`GridItemStyle::grid_min_size_keyword`,
  `grid_item::keyword_min_width`): in their known size and final clamp, their min-/max-content contributions and their
  minimum contribution (WPT css-grid `grid-items/grid-intrinsic-maximums`).
- Round 4 lane G (baselines): flexbox synthesizes a baseline at `height * block_baseline_synthesis` (0 in sideways
  vertical-lr, the line-under side); a column table item's flex base size from its `height` is measured at that
  height (captions added); grid `baseline.rs` synthesizes an orthogonal item's missing inline-axis baseline at its
  physical left (line-under).
- Flexbox: `baseline` items in an rtl column share a baseline at their line-left edges, their group at the cross
  start (`max_baseline_cross` in `align_flex_items_along_cross_axis`; WPT `flexbox-align-self-vert-rtl-001..003,005`).
- `compute/grid/track_sizing.rs` (`track_sizing_algorithm`, round 4 lane G): a percentage gutter resolves against the
  container's size alone; track percentages keep the minimum size as a fallback basis. While an inline grid with
  only a `min-width` sizes itself its percentage gap counts as zero, then overflows (Chromium, measured; WPT css-grid
  `subgrid/grid-gap-003`; `crates/pyro-render/tests/subgrid_edge_tracks.rs`).
- Lane F1 (CSS Shapes), `compute/float.rs` + `compute/block.rs` + `tree/traits.rs`: a float's `shape-outside` float area. `Segment::shape` holds the insets line boxes take where a float's area leaves them more room than its margin box. `FloatContext::shape_float` / `BlockContext::place_shaped_floated_box` set them from rows of the margin box with the area's horizontal extent (`LayoutBlockContainer::float_shape_bands`, implemented by blitz-dom). `find_content_slot` reads them; floats are still placed by margin boxes (CSS Shapes 1 § 3).

## Content-derived abspos heights are not definite (WPT round 4, lane S1)

`src/compute/oof.rs` (`height_is_definite`): an absolutely positioned box is laid out with its final size as known
dimensions, but its height is definite (`LayoutInput::known_dimensions_are_definite`) only when it was known before
its content was measured (its `height`, both insets, a ratio); one its content gives leaves its children's percentage
heights `auto` (CSS 2.1 § 10.5; WPT CSS2 `normal-flow/blocks-025`). Covered by
`crates/pyro-render/tests/abspos_percentage_heights.rs`.

## Boxes empty in one axis count for scrollable overflow (WPT round 7, css-overflow)

`src/compute/common/scrollable_overflow.rs` (`compute_scrollable_overflow_contribution`): upstream dropped a box
empty in either axis from its parent's scrollable overflow. Only a box empty in both axes is dropped now: one empty
in a single axis contributes its extent in the other axis and, in a scroll container, its end margin and the
parent's end padding, as in Chromium (WPT `scrollable-overflow-zero-one-axis`, a flex item shrunk to zero width
keeping its `margin-inline-end` in `scrollable-overflow-*-flex-item-margin-inline-end*`). Such a box sitting exactly
on the scroll origin is not wholly unreachable (the start edge is compared strictly). Covered by pyro-layout
`flow::a_box_empty_in_one_axis_still_extends_its_scrollers_overflow_like_chromium`.
