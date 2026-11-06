# blitz-dom 0.3.0-beta.4, patched

This is the published `blitz-dom` 0.3.0-beta.4 crate (crates.io, upstream commit
`92b11f1bd8ab7975411328b0406a95a09445b4e7`, `packages/blitz-dom` in
[DioxusLabs/blitz](https://github.com/dioxuslabs/blitz)), used through `[patch.crates-io]` in the
workspace `Cargo.toml`. License: MIT OR Apache-2.0, as upstream. Only `.cargo_vcs_info.json` and
`Cargo.toml.orig` were left out of the copy.

## The change

`UPSTREAM.patch` (applies with `git apply` from the root of an upstream checkout) is the whole diff:

- `src/node/attributes.rs`: `Attributes` keeps the `class` attribute's tokens as interned atoms
  (`class_atoms()`), computed on first use and dropped by every mutation (`set`, `remove`,
  `DerefMut`).
- `src/stylo.rs`: `has_class` and `each_class` read those atoms instead of splitting the attribute
  and interning every token on every call.

Stylo calls `has_class` once per class selector it tests and `each_class` for the bloom filter, rule
lookup and the style-sharing cache. Upstream interns each token every time (a global lock and a
hash), which was about 18% of the main thread rendering a Tailwind page (25 classes per element):
style resolution on the TOS positions list went from 24 to 9 ms. Matching is unchanged, and so are
the rendered display lists. lp-render's tests cover it (`class_selectors_match_as_before`,
`class_changes_invalidate_the_cached_tokens`).

## Updating or dropping the patch

- When upstream has the change: delete this directory and the `[patch.crates-io]` entry, then
  `cargo update -p blitz-dom`.
- To move to a newer blitz-dom: copy the published crate from `~/.cargo/registry/src/*/blitz-dom-<version>`
  over this directory, apply `UPSTREAM.patch` (the cached atoms field is the only structural
  change), bump the `=` versions of the blitz crates in `crates/lp-render/Cargo.toml`, and update the
  version and commit above.

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

The original `UPSTREAM.patch` covers only the class-atom optimization above;
these additional local changes are covered by lp-render's rendering tests.

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
  `Node::update_unrounded_layout` is the only writer of unrounded layouts, and marks changes as damage.
- `src/resolve.rs` (`resolve_layout`): no Taffy pass or rounding with nothing damaged and the same viewport.
  `BaseDocument::needs_resolve` tells a caller whether a `resolve` could change anything.

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
- `assets/default.css`: checkboxes and radios are Chromium's 13x13 border boxes (radios with Chromium's
  `3px 3px 0 5px` margin); Blitz had 14x14.
- `src/layout/mod.rs`: a textarea or select with a percentage width contributes zero to min-content sizing,
  as text inputs already did (CSS Sizing 3 §5.2.2, compressible): in a flex-wrap row it shrinks into the
  space left on the line instead of wrapping below.

`src/resolve.rs`: scrollable overflow as in Blink: an empty rect adds nothing, a box fixed to the viewport is left out, and `overflow` on `body` is the viewport's (`BaseDocument::body_overflow_goes_to_viewport`). `src/layout/list.rs`: `decimal-leading-zero`, `lower-roman`, `upper-roman`, `lower-latin` and `upper-latin` markers.

`src/layout/table.rs`: auto tables with percentage cells or `<col>`s distribute widths as css-tables and Blink do (`TableTreeWrapper::percent_column_widths`).

`BaseDocument::set_layout_size` lays out at a fractional CSS size instead of the viewport's whole pixels (`make_device`); printing uses it for the paper's content box.

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
rules: each cell keeps half of the borders around it (`LayoutData::collapsed_border`, read through
`ComputedStyleRef::table_cell_override`) and the table half of its outer ones, as Chromium's boxes do. Uniform tables keep the gutter.
Values in `bench/pdf/fixtures/chrome-156/table-collapse.json`, tests in `crates/lp-render/tests/table_collapsed_borders.rs`.

A cell's layout style does not copy its `ComputedValues`: `Node::layout_style` hands `TaffyStyloStyle` a
`stylo_taffy::CellOverride` (height read as `auto`, collapsed border widths in px), which its `size` and `border`
getters read in place of the style's (see `vendor/stylo_taffy/README.lightningpanda.md`). Upstream's
`for_table_cell_layout` boxed a clone and copied the position and border structs on every child-style read of a
fixed-height or collapsed-border cell. `a_table_cell_override_reads_as_the_copied_style_did` checks every getter
against that copy.

`src/layout/table.rs`: the table places its cells once, as it is built (`place_cells`, stored as
`TableContext::placement`), and hands the areas to the grid through taffy's `LayoutGridContainer::track_counts`
and `resolved_placement` (see `vendor/taffy/README.lightningpanda.md`). It runs the grid's `row dense`
placement over each row's occupied column ranges (`OccupiedColumns`: sorted, adjacent ranges merged, so a
row of contiguous cells is one range and a wide row is not rescanned per cell), which is not always the column `ColumnCursor` gave the cell
(`TableCell::column`): a colspan cell that does not fit beside a rowspan hole moves past it and a later cell
backfills the hole. Upstream ran taffy's placement on every layout of the table. A cell whose `grid-row` is
not a positive line and a span (or whose `colspan` is 0) leaves the placement to the grid.
`table_cell_placement_is_the_grids` compares the grid info and every box's layout with the grid placing the cells.

Time inputs use the existing text-editor construction and intrinsic measurement paths so captured values are painted.
