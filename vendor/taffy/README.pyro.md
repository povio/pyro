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

Not done: the browsers' rule that an auto width's negative margins do not widen the band it must fit in (WPT
`floats-wrap-bfc-with-margin-001`, which every browser passes) contradicts `floats-wrap-bfc-with-margin-006/007.tentative`
(every browser fails them, pyro passed them), so the band still widens by them. Nor `max-height` separating a
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
  the subgrid lies at an edge, the items at the edge-most occupied track are also given stretched to it with the edge's
  margin, border and padding; an empty subgrid gives an item of no size spanning its tracks with them at both edges (as
  Blink, WPT `subgrid-no-items-on-edges-001`). The virtual items are dropped before the items are positioned;
- stretches the subgrid in its subgridded axes, whatever its alignment, size, minimum and maximum size and aspect ratio.

The subgrid's own layout places its items as the grid placed them for sizing (`place_subgrid_items`): the spanned
track count is its explicit grid in a subgridded axis, with no implicit tracks (items are clamped to it), its line
names are its `<line-name-list>` (`subgrid_line_names`, `repeat(auto-fill)` repeated to fit) plus the inherited ones,
and a subgrid child's automatic span is the number of tracks its own list names. Its tracks there are fixed at the
given sizes, `justify-content`/`align-content` do not apply, and after alignment they are placed at the given spans.
A subgrid in its rows only records its own columns (`SubgridContext::own_columns`) and its parent, before sizing rows,
lays it out at its width if needed (`resolve_standalone_widths`): its items' heights depend on its columns. The resolved
value of a subgridded axis is `subgrid` with each line's own names (`DetailedGridTracksInfo::subgrid`).

Not done: orthogonal subgrids (laid out as `none`), baseline alignment across a subgrid, the subgrid's own rows for a
subgrid in its columns only (its items' column contributions see an indefinite height). Covered by
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
- a definite cross size (a stretched one) gives the flex base size (§9.2 B) and bounds the content size suggestion, as
  Blink measures a replaced element's min-content size with a definite block size (`relayout-intrinsic-block-size`).

The property's ratio and every other use of `aspect_ratio` are unchanged. Covered by
`crates/pyro-render/tests/flex_natural_ratio.rs`.

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
