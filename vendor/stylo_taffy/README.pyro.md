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
- `src/wrapper.rs` (`replaced_overflow`): the `overflow` of a replaced element (`StyleFlags::IS_REPLACED`) is
  `clip` for Taffy whenever it is not `visible`: on replaced elements the used value of every other value is
  `clip` (CSS Overflow 3), so a replaced box is no scroll container. Its automatic minimum size as a flex or grid
  item stays its content size and it starts no formatting context, as in Chromium; only paint clips it (blitz-paint
  `draw_svg`). The UA sheet's `svg:not(:root) { overflow: hidden }` (blitz-dom) let SVG icons in a flex row
  shrink without it. Covered by `an_svg_flex_item_does_not_shrink_below_its_size` in
  `crates/pyro-render/tests/svg_viewport_clip.rs`.
