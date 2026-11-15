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
