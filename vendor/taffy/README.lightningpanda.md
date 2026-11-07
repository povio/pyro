# taffy 0.15.1, patched

The published `taffy` 0.15.1 crate (crates.io; upstream tag `v0.15.1`, commit `0064bd5`, in
[DioxusLabs/taffy](https://github.com/DioxusLabs/taffy)), used through `[patch.crates-io]` in the
workspace `Cargo.toml`. License: MIT, as upstream. `.cargo_vcs_info.json`, `Cargo.toml.orig`, `benches/`
and `tests/` were left out of the copy.

## The change

`UPSTREAM.patch` (applies with `git apply` from the root of an upstream checkout) is the whole diff.

`compute/flexbox.rs`, `determine_container_main_size`: an auto-width row flex container in definite
available space with a single line took the line's max-content length. CSS sizes it fit-content,
`min(max-content, max(min-content, available))`, so its shrinkable items shrink and their text wraps.
Hit by a `display:flex` row inside an `align-items:center` column (ToS invoice footers) and by
`width: fit-content`. Column containers are unchanged: an auto height is the content height. The available size is the
content box's; the clamp adds the container's padding and border to it before comparing outer
sizes (`a_padded_inline_flex_keeps_its_max_content_width`).

Upstream's full test suite (6605 generated tests) passes with the patch. The regression tests are
`a_centered_flex_item_is_clamped_to_its_container` and
`a_fit_content_flex_row_is_clamped_to_the_available_width` in `crates/lp-render/tests/render.rs`.

`compute/mod.rs` and `tree/traits.rs`: incremental rounding. `RoundTree` gains `rounded_unchanged` and
`record_rounded` (defaults: never skip, record nothing), and `round_layout` skips a subtree its tree reports
unchanged. See "Incremental rounding" below for how blitz-dom uses them.

`compute/block.rs` and `tree/traits.rs`: `LayoutBlockContainer::block_child_width_fits_content`
(default false) lets a tree keep a block-level child's `auto` width at its content size, as for tables and
replaced elements. blitz-dom answers true for form controls (`input`, `select`, `textarea`, `button`),
which Chromium does not stretch in block layout.

`compute/flexbox.rs`, `determine_container_main_size`: a column container's automatic block size is the
sum of its items' hypothetical main sizes (flex base size clamped by the resolved minimum and the max
size), as Chromium has it. An item's `height` counted on its own before: `flex: 1 1 0px; height: 10px`
made its container 10px tall (Chromium: 0), and a percentage basis of the indefinite container, which
is used as `content`, still counted the item's `height`. Covered by
`a_column_flex_container_is_as_tall_as_its_hypothetical_items` and
`flow_percentage_flex_basis_in_an_indefinite_column_is_content` in `crates/lp-layout/tests/flow.rs`.

`compute/grid/mod.rs`, `compute/grid/track_sizing.rs` and `tree/traits.rs`:
`LayoutGridContainer::grid_rows_are_table_rows` (default false) marks a grid whose rows are a table's;
blitz-dom's table answers true. There an item spanning several rows grows them as Chromium grows the rows
under a rowspan cell (`DistributeExcessBlockSizeToRows`, percentage rows aside) instead of by the grid
algorithm's even distribution: the excess goes to the rows where another rowspan starts, else to the
unconstrained rows with a size in proportion to it, else (all rows empty) to the last row, else to the
rows with a size in proportion to it. Covered by `table_rowspan_like_chromium` in
`crates/lp-layout/tests/flow.rs`.

`compute/grid/track_sizing.rs`, `expand_flexible_tracks`: flexible tracks are sized as Chromium's
`ExpandFlexibleTracks` sizes them. Each track's share of the flex fraction is truncated to 1/64 px (a
layout unit) in f32 raw units after an epsilon, and the fraction it loses carries into the next flexible
track, so `repeat(3, 1fr)` over 668px is 222.65625, 222.671875, 222.671875, as Chromium reports.
Chromium sizes a run of identical tracks (a "set") as one; this sizes every track on its own, which agrees
whenever grid items start in each track. The layout unit is 1/64 of a Taffy unit: a CSS px at scale 1.
Upstream's generated tests expect exact fractions and are not run against this change.
`grid_parity_tailwind_columns` and `grid_parity_calendar_fr_rows_with_min_height` in
`crates/lp-layout/tests/flow.rs` cover it.

## Incremental rounding

These hooks went unused for a while. blitz-dom's `writing-mode` feature (#199) replaced `round_layout` with
its own walk, `physicalise_and_round_layout` in `layout/writing_mode.rs`, which had no skip, and lp-render
enables that feature. The hooks kept compiling and nothing noticed: every layout query rounded the whole
tree, 87-91% of layout time on a paginating document.

The walk now calls `rounded_unchanged` and `record_rounded` itself, so `round_layout` only runs in a build
without the feature. Taffy's side is unchanged, as is `UPSTREAM.patch`. A subtree is skipped when:

- its root is not an anonymous box and has style data (damage flags are not kept on the others);
- no damage on the root or in its subtree. Every unrounded layout change marks damage up the DOM parent
  chain (`Node::update_unrounded_layout`, the only writer). Inline layout used to write these layouts
  directly, so an inline-block or float could move, beside a resizing float, inside a box with no damage;
- the same `LayoutData::rounded_from` key: the root's *physical* unrounded layout (a `vertical-rl` box's x
  depends on its placer's width) and absolute offset. A box that only moved is marked on its ancestors, not
  itself (`Node::update_unrounded_layout`): the key's location tells its rounding to follow the move.

Debug builds round the whole tree again after each pass and assert that it matches
(`assert_rounding_is_current`), so every test checks the skip. `rounding_skips_what_a_change_did_not_touch`
in `crates/lp-render/tests/mirror.rs` fails if the skip stops firing; `LP.geometryStats` reports
`roundedNodes` and `roundingSkips`.

`compute/mod.rs`: `compute_root_layout_in` takes an initial containing block of another size than the root's available space (print: Blink's rounds down, the page area up). The `UPSTREAM.patch` hunk for it is the second `src/compute/mod.rs` section, before the `src/lib.rs` one.

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
tables" in `vendor/blitz-dom/README.lightningpanda.md`). `crates/lp-render/tests/mirror.rs` checks replayed layouts
against fresh builds (`table_*_match_fresh_builds`) and that edits, appends, removals and inserts replay (`a_cell_edit_collects_only_its_row`).
The `UPSTREAM.patch` sections for it are the last ones (`src/compute/grid/alignment.rs` to `src/tree/traits.rs`), made
against the tree the sections before them give.
