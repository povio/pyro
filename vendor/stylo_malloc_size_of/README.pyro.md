# stylo_malloc_size_of 0.24.0, patched

The published `stylo_malloc_size_of` 0.24.0 crate (crates.io; part of [servo/stylo](https://github.com/servo/stylo)),
used through `[patch.crates-io]` in the workspace `Cargo.toml`. License: MIT OR Apache-2.0, as upstream.
`.cargo_vcs_info.json`, `.cargo-ok` and `Cargo.toml.orig` were left out of the copy. `UPSTREAM.patch` is the whole
diff (`git diff --no-index --no-prefix` of the published crate and this copy).

## The change

`lib.rs`: the size of a selector `Component` counts the selector of `Component::HostContext` (`:host-context()`,
which pyro's `selectors` adds: `vendor/selectors/README.pyro.md`) as it counts `:host()`'s. The match is exhaustive,
so without it the crate does not build against that `selectors`.
