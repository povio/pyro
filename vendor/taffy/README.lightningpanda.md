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
  depends on its placer's width) and absolute offset.

Debug builds round the whole tree again after each pass and assert that it matches
(`assert_rounding_is_current`), so every test checks the skip. `rounding_skips_what_a_change_did_not_touch`
in `crates/lp-render/tests/mirror.rs` fails if the skip stops firing; `LP.geometryStats` reports
`roundedNodes` and `roundingSkips`.

`compute/mod.rs`: `compute_root_layout_in` takes an initial containing block of another size than the root's available space (print: Blink's rounds down, the page area up). The `UPSTREAM.patch` hunk for it is at the end of that file.

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
copy of this directory with an empty `[workspace]` table, as the crate is not a workspace member). The
`UPSTREAM.patch` hunks for it are the two sections before the table placement ones.

`compute/grid/mod.rs` and `tree/traits.rs`: a tree can place a grid's items itself.
`LayoutGridContainer::track_counts` (default `None`) answers how many columns and rows the placed items reach,
and `resolved_placement` each in-flow item's area in origin-zero lines. The grid then skips named-line
resolution, the grid size estimate, auto-placement and its occupancy matrix, and sizes the explicit tracks
plus whatever the items reach past them (no negative implicit tracks). Empty `auto-fit` tracks are found from
the items' areas. blitz-dom's tables answer it from `TableContext::placement`, computed once as the table is
built (`place_cells` in `layout/table.rs`, the same `row dense` placement). In debug builds
`debug_assert_tree_placement` runs the grid's own placement as well and asserts the areas and track counts are
equal. The `UPSTREAM.patch` hunks for it are the two sections before the last one.
`compute/mod.rs`, the root positioning pass: a root whose own layout comes from its cache, and which claims no
out-of-flow box itself, records this pass's hoisted boxes again instead of keeping the last run's. blitz-dom
relays an out-of-flow box whose containing block is the initial containing block without relaying its
ancestors (`POS_CHILD_RELAYOUT` in `layout/damage.rs`), so the root's layout is cached while the box, relaid
here, can surface other candidates (a `position: fixed` box added inside it). A root that claims boxes is
always relaid when one of them changes, and keeps its list. The hunk is the second `compute/mod.rs` section of `UPSTREAM.patch`, after the table placement ones.
Covered by `crates/lp-render/tests/out_of_flow_boundary.rs`.
