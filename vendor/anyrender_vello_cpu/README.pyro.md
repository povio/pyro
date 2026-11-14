# anyrender_vello_cpu 0.18.0, patched

The published `anyrender_vello_cpu` 0.18.0 crate (crates.io; part of
[dioxuslabs/anyrender](https://github.com/dioxuslabs/anyrender)), used through `[patch.crates-io]` in the
workspace `Cargo.toml`. License: MIT OR Apache-2.0, as upstream. `.cargo_vcs_info.json` and
`Cargo.toml.orig` were left out of the copy.

## The change

`UPSTREAM.patch` is the whole diff: three changes.

## A raster thread count per renderer

With the `multithreading` feature, vello_cpu can rasterize on worker threads of its own
(`RenderSettings::num_threads`), but `VelloCpuImageRenderer::new` always takes vello_cpu's default
settings: every renderer then gets a thread pool of one less than the cores (at most 8), and a 1280x720
capture cost twice the CPU time for the hand-offs. `VelloCpuImageRenderer::with_threads(width, height,
threads)` (and `VelloCpuScenePainter::with_settings`) make a renderer with the given count, 0 being the
calling thread; `resize` keeps it (upstream made a default context). pyro-render keeps one renderer of each
kind and rasterizes frames of 4 M device px or more on the threaded one (`THREADED_RASTER_PIXELS`,
`raster_threads` and `PYRO_RASTER_THREADS` in `crates/pyro-render/src/lib.rs`). The pixels are the same.

## Glyph emboldening in the units of glifo's outline cache

`draw_glyphs` takes its `embolden` amount at the scale of the font size, and passed it to glifo 0.4 as it was.
glifo expands the outline it caches, which is in font design units when a run is not hinted (drawn scaled by
size / unitsPerEm) and at the drawn size only when it is hinted. An unhinted run (all text on macOS since
`CORE_TEXT_GLYPHS`, and any run under a rotation or a skew) therefore got its amount times size / unitsPerEm:
blitz-paint's synthetic bold of 0.6 px on 16px Lato (2000 units per em) came out as 0.005 px, no bold at all.
`embolden_in_cache_units` in `scene.rs` converts the amount (and the expansion tolerance) to the cache's units:
times unitsPerEm / size for an unhinted run (`units_per_em` reads the `head` table), times the transform's
scale for a hinted one, by the conditions glifo hints under. glifo's main branch converts in `FontEmbolden`
itself (`embolden_in_cache_units`); drop this when the workspace moves to a glifo with it. Covered by
pyro-render's `a_missing_bold_face_is_synthesized`.

## Filter layers (`blur()`, `drop-shadow()`)

vello_cpu 0.3 filters a layer only when it renders on one thread (its multi-threaded dispatcher panics on a
filter layer, and pyro rasterizes frames of 4 M device px or more on threads), takes a single primitive per
layer and panics on most of them (colour matrices). So the `filters` feature, which passed a layer's filter to
vello_cpu, is not used (it does nothing now), and `VelloCpuScenePainter::append_scene` replays a recorded scene
itself: a layer whose filter has a `blur()` or a `drop-shadow()` (`src/filters.rs`, `FilterPlan`; a chain of
them is applied in order, other effects are left out as before) is rendered into a single-threaded context of
its own over the pixels the filter reads to produce what the layer's clip shows of the frame, filtered, and
drawn into the frame as images inside a layer with its clip, blend mode and opacity; a drop shadow's content is
then drawn as it is, over its shadow. An opaque layer of the normal blend mode whose clip is a rectangle of whole
device pixels holding everything it draws gets no layer of its own (the images are drawn inside the clip): that
layer cost a 4K title card's shadow 3 to 5 ms of CPU time. A layer pushed with `push_layer` directly is drawn
unfiltered, as before.

The blur is Skia's on the CPU, which Chromium 147 uses for filters and text shadows: three box blurs with SVG's
`feGaussianBlur` windows (a deviation of 0.94 times the one asked for), a Gaussian kernel below a deviation of 2;
measured within a level of Chromium. A single blur or drop shadow of a deviation of 8 device px or more renders
the content at a resolution reduced by a power of two that leaves 4 to 8 there (as Skia does on the GPU),
blurs with boxes of fractional widths (Gwosdek et al.) of the full resolution's variance less what the reduction
and the bilinear scaling back up spread, and lets the frame's raster scale the result up: within two levels of
the full-resolution blur, at a cost that does not grow with the deviation (a 4K title card's 60 px shadow:
0.5 ms). A blur of content in one solid colour (a text shadow's glyphs) blurs the alpha alone. Covered by the
unit tests in `src/filters.rs` and by pyro-render's `filter_drop_shadow.rs` and `text_shadow.rs`.

Regenerate the patch from a pristine copy of the published crate:
`git diff --no-index --no-prefix a b` with the published crate in `a` and this directory, without this file
and the patch, in `b`; `git apply UPSTREAM.patch` in a pristine copy reproduces `src`.
