# anyrender_vello_cpu 0.18.0, patched

The published `anyrender_vello_cpu` 0.18.0 crate (crates.io; part of
[dioxuslabs/anyrender](https://github.com/dioxuslabs/anyrender)), used through `[patch.crates-io]` in the
workspace `Cargo.toml`. License: MIT OR Apache-2.0, as upstream. `.cargo_vcs_info.json` and
`Cargo.toml.orig` were left out of the copy.

## The change

`UPSTREAM.patch` is the whole diff: a raster thread count per renderer.

With the `multithreading` feature, vello_cpu can rasterize on worker threads of its own
(`RenderSettings::num_threads`), but `VelloCpuImageRenderer::new` always takes vello_cpu's default
settings: every renderer then gets a thread pool of one less than the cores (at most 8), and a 1280x720
capture cost twice the CPU time for the hand-offs. `VelloCpuImageRenderer::with_threads(width, height,
threads)` (and `VelloCpuScenePainter::with_settings`) make a renderer with the given count, 0 being the
calling thread; `resize` keeps it (upstream made a default context). pyro-render keeps one renderer of each
kind and rasterizes frames of 4 M device px or more on the threaded one (`THREADED_RASTER_PIXELS`,
`raster_threads` and `PYRO_RASTER_THREADS` in `crates/pyro-render/src/lib.rs`). The pixels are the same.

Regenerate the patch from a pristine copy of the published crate:
`git diff --no-index --no-prefix a b` with the published crate in `a` and this directory, without this file
and the patch, in `b`; `git apply UPSTREAM.patch` in a pristine copy reproduces `src`.
