# parley_engine 0.12.0, patched

The published `parley_engine` 0.12.0 crate (crates.io; part of
[linebender/parley](https://github.com/linebender/parley)), used through `[patch.crates-io]` in the
workspace `Cargo.toml`. License: Apache-2.0 OR MIT, as upstream. `.cargo_vcs_info.json` and
`Cargo.toml.orig` were left out of the copy.

## The change

`UPSTREAM.patch` is the whole diff.

`shape/shaped_text.rs`, `FontMetrics::from_skrifa_metrics`: Times, Helvetica and Courier get
WebKit's and Chromium's (macOS) extra ascent, 15% of their rounded ascent plus descent, rounded.
These are macOS's default `serif`, `sans-serif` and `monospace` fonts, so every page without an
explicit font family had shorter lines than in Chrome (`line-height: normal` at 16px: 16px instead
of 18px), and taller documents paginated differently.
