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

## Incremental passes for layout queries

- `src/resolve.rs` (`resolve_layout_children`): an undamaged subtree without damaged descendants is not
  walked (its layout children are those of the last pass); anonymous boxes are always visited.
- `src/resolve.rs` (`resolve_transforms`): a node's cached scrollable overflow is reused only when it and
  its subtree are undamaged (a descendant's relayout reaches ancestors as damage, not as
  `RECALCULATE_OVERFLOW`).
- `src/layout/mod.rs`, `src/node/element.rs`: incremental rounding (taffy's `RoundTree::rounded_unchanged`,
  vendored taffy), keyed by `LayoutData::rounded_from`.

lp-render's mirror tests (incremental documents against fresh builds after random mutations) cover both.
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

`BaseDocument::set_layout_size` lays out at a fractional CSS size instead of the viewport's whole pixels (`make_device`); printing uses it for the paper's content box.
