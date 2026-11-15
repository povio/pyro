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
