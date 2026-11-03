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
unchanged. blitz-dom implements them: an undamaged, non-anonymous subtree reached with the same unrounded
layout at the same offset is rounded as before. Without it, every layout query rounded the whole tree.

`compute/block.rs` and `tree/traits.rs`: `LayoutBlockContainer::block_child_width_fits_content`
(default false) lets a tree keep a block-level child's `auto` width at its content size, as for tables and
replaced elements. blitz-dom answers true for form controls (`input`, `select`, `textarea`, `button`),
which Chromium does not stretch in block layout.

`compute/grid/track_sizing.rs`, `expand_flexible_tracks`: flexible tracks are sized as Chromium's
`ExpandFlexibleTracks` sizes them. Each track's share of the flex fraction is truncated to 1/64 px (a
layout unit) in f32 raw units after an epsilon, and the fraction it loses carries into the next flexible
track, so `repeat(3, 1fr)` over 668px is 222.65625, 222.671875, 222.671875, as Chromium reports.
Chromium sizes a run of identical tracks (a "set") as one; this sizes every track on its own, which agrees
whenever grid items start in each track. The layout unit is 1/64 of a Taffy unit: a CSS px at scale 1.
Upstream's generated tests expect exact fractions and are not run against this change.
`grid_parity_tailwind_columns` and `grid_parity_calendar_fr_rows_with_min_height` in
`crates/lp-layout/tests/flow.rs` cover it.
