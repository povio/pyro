# vello_cpu 0.3.0, patched

The published `vello_cpu` 0.3.0 crate (crates.io; part of [linebender/vello](https://github.com/linebender/vello),
`sparse_strips/vello_cpu`), used through `[patch.crates-io]` in the workspace `Cargo.toml` by pyro-render (through
`vendor/anyrender_vello_cpu`), anyrender_svg and pyro-canvas. License: Apache-2.0 OR MIT, as upstream.
`.cargo_vcs_info.json`, `.cargo-ok` and `Cargo.toml.orig` were left out of the copy.

## The change

`UPSTREAM.patch` is the whole diff: one change.

## Skia's rounding in 8-bit compositing

`src/util.rs` (`Div255Ext`, `normalized_mul_u8`, `scalar::div_255`) and the imports of `src/fine/lowp/{mod,compose,
blend}.rs`. The u8 pipeline (the one pyro uses: `u8_pipeline`, `RenderMode::OptimizeSpeed`) divides each product of
two 8-bit values by 255 rounding to the nearest integer, `(t + (t >> 8)) >> 8` with `t = v + 128`, as Skia's 8-bit
raster pipeline divides on NEON (lowp `div255` in `SkRasterPipeline_opts.h`, `vrshrq_n_u16(vrsraq_n_u16(v, v, 8),
8)`) and as a GPU's float blend rounds (Chromium on macOS); Skia's lowp `div255` on other CPUs is upstream's
`(v + 255) / 256`. Upstream used vello_common's `Div255Ext`, `(v + 255) >> 8`, which is `ceil(v / 256)`: a level off
for about 29% of all products (high for 26.4%, low for 2.3%). Every composite of a translucent colour over another
came out a level off what Chromium paints, and off the references that spell those colours out as one opaque colour: WPT
box-shadow-multiple-001 (`#BF7F7F` came out `#BF7F80`), t32-opacity-zorder-c (white seen through three
`opacity: .99` reds never faded: 1 x 3 / 255 rounded up to 1), backdrop-filter-isolation-isolate/-fixed (`#ff08`
over a backdrop), box-shadow-041/-042 and clip-path-animation-inherit (a masked composite's covered pixels, which
round once, against an unmasked composite's two roundings: equal once both round to nearest). Both divisions are
exact for multiples of 255 (`div_255(v * 255) == v`), so fully covered opaque pixels composite as before. An
antialiased edge of opaque content over another colour divides `bg * (255 - m) + src * m`, rarely a multiple of
255: about a fifth of those pixels move by a level.

What uses it: source-over and the other Porter-Duff composites, masked and unmasked (`fill`, `alpha_fill`,
`compose`), a layer's opacity and its mask (`apply_mask`), tints, the separable blend modes (`blend.rs`), and the
product of a fill's coverage with a mask layer's. Left with upstream's division: the bilinear image painter
(`lowp/image.rs`), vello_common's clip-path intersections and its premultiplication of images (no reftest was
found to depend on them).

Cost (pyro PR #373, 4K = 1920x1080 CSS px at DSF 2, single-threaded raster): a screenshot of an effects-heavy page
(a drop-shadow card, an opacity card with a 40 px box shadow, a 6000 px spread ring at opacity .95, text shadow, a
rotated translucent panel, a multiply blend: several full-frame layers) runs about 3% more instructions (1596-1609 M
per frame on main, 1651-1652 M with this), CPU time within run-to-run noise; an ordinary page's (a dashboard: cards
and a table) is unchanged (1031 M on main, 1028-1034 M with this).

Covered by the unit tests in `src/util.rs` (`div_255_rounds_to_nearest`) and `src/fine/lowp/mod.rs`
(`translucent_colours_over_translucent_colours_round_to_nearest`, `a_translucent_layer_hides_a_level_of_what_lies_under_it`),
which cargo does not run in the workspace (a path dependency with dev-dependencies; run them from a copy outside it
with `cargo test --lib --no-default-features --features std,text,u8_pipeline`). The same values are checked through
the public API by pyro-canvas's `tests/vello_cpu_rounding.rs` (every byte product, the composites above), which
`cargo test -p pyro-canvas` runs, and through pages by pyro-render's `compositing_rounding.rs`.

Regenerate the patch from a pristine copy of the published crate: `git diff --no-index --no-prefix a b` with the
published crate (without `.cargo_vcs_info.json`, `.cargo-ok` and `Cargo.toml.orig`) in `a` and this directory, without
this file and the patch, in `b` (`scripts/vendor/regen-patch vello_cpu`); `git apply UPSTREAM.patch` in a pristine
copy reproduces this directory.
