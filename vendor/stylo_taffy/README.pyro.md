# stylo_taffy main (c27b4ca), patched

`packages/stylo_taffy` of [DioxusLabs/blitz](https://github.com/dioxuslabs/blitz) main at commit
`c27b4ca14158dc457f35c78eb32b8c4fec906b7e`, as `vendor/blitz-dom` (see its README.pyro.md), which
needs this version. `Cargo.toml` is the package manifest with the workspace's dependency versions written in.
`UPSTREAM.patch` (applies with `git apply` from the root of an upstream checkout at that commit) is the whole
diff of `src/`; `scripts/vendor/regen-patch stylo_taffy` rewrites it. Drop the crate (and its `[patch.crates-io]`
entry) once a Blitz release carries these commits and changes.

- `src/wrapper.rs`: `StyleFlags::IS_SPLIT_INLINE`, set by blitz-dom on an inline box split around
  block-level content (CSS 2.1 § 9.2.1.1). Such a box is laid out as a block box wrapping its pieces
  (blitz-dom's README.pyro.md, "block-in-inline"); its margins, borders, padding and sizes belong to
  its inline head and tail, so as a box it has none: `size`, `min_size` and `max_size` are `auto`,
  `margin`, `padding` and `border` zero.
- `src/wrapper.rs`: `BlockContainerStyle::margin_trim` answers from the computed `margin-trim`
  (`block-start`, `block-end`), for Taffy's block layout (vendor/taffy/README.pyro.md).
- `src/wrapper.rs`: for Taffy's aspect-ratio rules (vendor/taffy/README.pyro.md):
  `aspect_ratio_of_content_box` (`auto <ratio>` on a non-replaced box), `breaks_lines_in_block_axis` (a wrapping
  column flex container), and `StyleFlags::HAS_NATURAL_RATIO`, set by blitz-dom on a replaced element with a
  natural aspect ratio: `aspect_ratio()` then answers `None` for `auto <ratio>`, so the natural ratio applies
  (css-sizing-4 §5.1).

- `src/wrapper.rs` (`float`, `clear`): `inline-start` and `inline-end` are sides of the inline axis: with a
  right-to-left direction (the box's inherited one stands in for its containing block's) `inline-start` is the right
  side. They were always left and right (WPT `tab-stop-with-float-001.html?class=rtl`;
  `crates/pyro-render/tests/tab_stops.rs`).
- `src/convert.rs`, `src/wrapper.rs`: `BlockItemStyle::min_size_keyword`/`max_size_keyword` answer the sizing
  keywords of `min-width`, `max-width`, `min-height` and `max-height` (`min_size`/`max_size` still say `auto` for
  them), for Taffy's block layout (vendor/taffy/README.pyro.md).
- `src/wrapper.rs`: `StyleFlags::OVERFLOW_TO_VIEWPORT`, set by blitz-dom on the `<body>` whose `overflow` the viewport
  takes (CSS Overflow 3 §3.3): `overflow()` answers `visible` in both axes, so `body { overflow: hidden }` makes no
  scroll container (no block formatting context; its children's margins collapse through it).
- `src/wrapper.rs`: subgrids for Taffy (vendor/taffy/README.pyro.md, "Subgrids"): `GridContainerStyle::subgrid_line_names`
  (the `<line-name-list>` of `grid-template-*: subgrid`, `repeat(<integer>)` expanded, `repeat(auto-fill)` marked),
  `gap_is_normal`, and `GridItemStyle::is_subgrid` (a grid container in flow, in its parent's orientation, with no
  layout or paint containment and no size container type, whose template in the axis is `subgrid`).
- `src/wrapper.rs`: `TaffyStyloStyle::natural_ratio`, set by blitz-dom on a replaced element with a natural aspect ratio;
  `CoreStyle::natural_aspect_ratio` answers it (in the layout's axes) while `aspect-ratio` is `auto`, for Taffy's flex
  algorithm (vendor/taffy/README.pyro.md, "Natural aspect ratios of flex items").
- `src/wrapper.rs`: `BlockContainerStyle::text_align_edge` (`start`/`end` by the direction in Taffy's axes,
  `left`/`right` by the line-left side) and `BlockItemStyle::original_display_is_inline`, for the static positions of
  inline-level out-of-flow boxes in block containers (vendor/taffy/README.pyro.md, "Static positions and positioned
  table parts").
- `src/wrapper.rs`: `FlexboxItemStyle::is_table` (a table wrapper box), for Taffy's flex algorithm (vendor/taffy/README.pyro.md,
  "Tables: a height is a minimum; tables as flex items").
- `src/wrapper.rs`: `FlexboxItemStyle::min_size_keyword`/`max_size_keyword`, the block items' (vendor/taffy/README.pyro.md,
  "Sizing keywords in flex items' minimum and maximum sizes").
- `src/convert.rs` (`item_alignment`): `last baseline` is Taffy's `AlignItems::LAST_BASELINE` (it was `end`, its fallback),
  for flex rows (vendor/taffy/README.pyro.md, "`last baseline` in flex rows").
- `src/wrapper.rs`: `GridItemStyle::subgrid_axis`/`subgrid_reversed` for orthogonal subgrids: the parent's inline axis is
  the child's block axis when one of them is vertical, and whether the tracks run opposite ways is found from both
  writing modes (inline: `direction` or `is_inline_tb`; block: top to bottom, or `vertical-lr`/`-rl`).
- `src/wrapper.rs`: `FlexboxItemStyle::is_collapsed` (`visibility: collapse`) for collapsed flex items
  (vendor/taffy/README.pyro.md, "`visibility: collapse` on flex items").
- `src/convert.rs` (`content_alignment`, `item_alignment`): `flow-start`/`flow-end` (vendor/stylo/README.pyro.md) are
  Taffy's `FLEX_START`/`FLEX_END`, which are `start`/`end` outside flex layout.
- `src/wrapper.rs`: `OofItemStyle::min_size_keyword`/`max_size_keyword` (the block item's), for the sizing
  keywords in the minimum and maximum sizes of absolutely positioned boxes (vendor/taffy/README.pyro.md).
- `src/convert.rs`: `collapsed_rows` (vendor/taffy/README.pyro.md) is empty; blitz-dom's table layout sets it.
- `src/convert.rs` (`fieldset_display`), `src/wrapper.rs` (`StyleFlags::IS_FIELDSET`, now 16 bits): a `<fieldset>`
  lays out as HTML's rendering section says whatever its `display`: an inline-level one is an inline-block
  (`inline-flex`/`inline-grid` stay), a table or table-internal one a block (WPT css-display `table-display-on-fieldset`, `display-inline-on-block-container`).
