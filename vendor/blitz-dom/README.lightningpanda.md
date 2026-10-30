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
