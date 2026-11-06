# stylo_taffy 0.3.0-beta.4, patched

The published `stylo_taffy` 0.3.0-beta.4 crate (crates.io, upstream commit
`92b11f1bd8ab7975411328b0406a95a09445b4e7`, `packages/stylo_taffy` in
[DioxusLabs/blitz](https://github.com/dioxuslabs/blitz), the same commit as `vendor/blitz-dom`), used through
`[patch.crates-io]` in the workspace `Cargo.toml`. License: MIT OR Apache-2.0 OR MPL-2.0, as upstream.
`.cargo_vcs_info.json` and `Cargo.toml.orig` were left out of the copy.

## The change

`UPSTREAM.patch` (applies with `git apply` from the root of an upstream checkout) is the whole diff.

`src/wrapper.rs`: `TaffyStyloStyle` gains a `cell: CellOverride` field (default: no override). When set,
`CoreStyle::size` reads the physical `height` as `auto` and `CoreStyle::border` reads the given physical
widths in px instead of the style's widths and border styles; the `taffy::Style` conversion applies the same.
Every other getter reads the style as before.

blitz-dom lays a table cell out with its authored `height` as a minimum rather than a definite size, and a
collapsed-border cell with its share of the borders (or none, when the table's gutters hold them). It did
that by boxing a clone of the cell's `ComputedValues` and copying its position and border structs on every
`Node::layout_style` call, which Taffy makes for every child-style read. Paginating the ToS `cmr-overflow`
document spent about 1.3% of its samples there. The override is a few bytes read from the primary style and
`LayoutData::collapsed_border` per call; nothing is copied.

`a_table_cell_override_reads_as_the_copied_style_did` in `vendor/blitz-dom/src/node/stylo_data.rs` checks
the getters and the conversion against the copied style, for separate and collapsed tables, uniform and
per-cell collapsed borders, fixed, percentage and auto heights and a vertical cell. The rendered ToS PDFs are
byte-identical.

## Updating or dropping the patch

- To move to a newer stylo_taffy: copy the published crate from `~/.cargo/registry/src/*/stylo_taffy-<version>`
  (without `.cargo_vcs_info.json` and `Cargo.toml.orig`), apply `UPSTREAM.patch`, and keep it in step with
  `vendor/blitz-dom`.
- Dropping it means going back to a copied `ComputedValues` in blitz-dom's `Node::layout_style`.
