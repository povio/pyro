# anyrender_vello_cpu 0.18.0, patched

The published `anyrender_vello_cpu` 0.18.0 crate (crates.io; part of
[dioxuslabs/anyrender](https://github.com/dioxuslabs/anyrender)), used through `[patch.crates-io]` in the
workspace `Cargo.toml`. License: MIT OR Apache-2.0, as upstream. `.cargo_vcs_info.json` and
`Cargo.toml.orig` were left out of the copy.

## The change

`UPSTREAM.patch` is the whole diff: seven changes.

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

## Filter layers (`blur()`, `drop-shadow()`, the colour functions)

vello_cpu 0.3 filters a layer only when it renders on one thread (its multi-threaded dispatcher panics on a
filter layer, and pyro rasterizes frames of 4 M device px or more on threads), takes a single primitive per
layer and panics on most of them (colour matrices). So the `filters` feature, which passed a layer's filter to
vello_cpu, is not used (it does nothing now), and `VelloCpuScenePainter::append_scene` replays a recorded scene
itself: a layer whose filter has a `blur()`, a `drop-shadow()` or a colour function (`src/filters.rs`, `FilterPlan`;
a chain of them is applied in order) is rendered into a single-threaded context of
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
0.5 ms). A blur of glyph runs in one solid colour (a text shadow's) blurs the alpha alone, as the blur mask filter
Chromium draws text shadows with does; other content is blurred in all four channels, as Chromium's blur filter does
(a box blurred beside an invisible command in another colour came out a level off the box blurred alone). Covered by the
unit tests in `src/filters.rs` and by pyro-render's `filter_drop_shadow.rs` and `text_shadow.rs`.

The colour functions (`grayscale()`, `sepia()`, `saturate()`, `hue-rotate()`, `invert()`, `opacity()`,
`brightness()`, `contrast()`) arrive as `feColorMatrix` matrices (and `url()` filters' `feComponentTransfer`
functions, with linearRGB as conversion tables around them) (blitz-paint builds Chromium's) and are applied as
Skia's matrix colour filter applies them, which Chromium builds them into: to each pixel's unpremultiplied sRGB
values (not linearRGB), each matrix's result clamped to 0..1, functions in a row without rounding to bytes between
them, premultiplied and rounded at the end (`color_matrices`; a run of equal pixels is computed once). A matrix that
changes no byte (a neutral amount) is left out, trailing `opacity()` becomes the layer's opacity (`opacity()` alone
is a faded layer with no pixels of its own), and when the functions keep the alpha and the layer's content is in one
solid colour (a box, a run of text), that content is drawn in the mapped colour (`map_color`) instead of being
rendered apart. Covered by the unit tests in `src/filters.rs` and `src/scene.rs` and by pyro-render's
`filter_color.rs`.

## SVG filter graphs

`src/graph.rs`. A filter layer whose filter is no chain of blurs, drop shadows and colour functions (an SVG
`<filter>`, from anyrender_svg) is a graph: `feFlood`, `feOffset`, `feGaussianBlur`, `feDropShadow`,
`feColorMatrix`, `feComponentTransfer`, `feComposite` (arithmetic included), `feBlend` (every mode),
`feDisplacementMap`, `feTile`, `feMorphology`, `feConvolveMatrix`, `feTurbulence`, the two lighting primitives,
`feImage` and crops, reading the source graphic, its alpha and each other's results. Each node is computed over one
region of device pixels, the part of the filter region (the layer's clip) the output needs: as SVG asks, nothing
outside the filter region is read or made, the source graphic included (a chain, CSS's, reads the content past the
clip). Colour nodes in a row are applied to each pixel in one go, as Skia composes colour filters. A graph with a node
this backend does not do (a background image input) is drawn unfiltered, as before.

`src/primitives.rs` (new) does the pixel work of the newer primitives, as Chromium's (Skia's) filters do it where that
differs from the spec's reference code: `feDisplacementMap` samples the nearest pixel to the displaced centre (the
map unpremultiplied, the displacement mapped from the filter's user space to device pixels, so a mirrored or scaled
space moves as it is drawn), `feTile` repeats its input's subregion from its own position, `feMorphology` takes the
minimum or maximum over a window cut to the region (radii mapped to device pixels and rounded),
`feConvolveMatrix` turns the kernel half a turn and computes in floats with the bias times 255, rounded once (with
`preserveAlpha`, unpremultiplied colours), the lighting primitives take the spec's Sobel normals (edge kernels at the
region's edges) and Skia's spot light cone (smoothed over 0.016 of the cosine), angles reduced to whole turns
first. `feTurbulence` and the lighting normals are resvg's ports of the spec's code (resvg 0.48, `filter/`;
Apache-2.0 OR MIT, as this crate; attributed in the file). Nodes whose output may come from anywhere in their input
(`feTile`) read the whole filter region, up to 4096 px past what shows.

Conventions with the producer: the graph's output is its last node (`Filter::add` leaves a graph's output at its
first node and the setter is private), and a primitive's subregion is an `Image` node of the image id
`FILTER_CROP_IMAGE` (`u32::MAX`, exported) whose transform holds the rectangle (`[x0, y0, x1, y1, ..]`, the filter's
space) and whose input is the node it crops. What anyrender's types cannot say: `FilterEffect::Tile` tiles the
rectangle of the crop node it reads; `ConvolveMatrix`'s `size` is the number of columns and its `values` the kernel
followed by the rows, the target's x and y and the edge mode, read first (`order="4 3"` makes 16 values, as many as
a square 4 x 4 kernel; without those four, a square kernel centred, edges duplicated); a lighting node's colour is the `Flood` node that is its second input; and `Image` nodes of exported ids
below the crop's are `FILTER_IMAGE_SOURCE` (an `feImage`'s content: `[k, 0, ..]`), `FILTER_BLUR_XY` (two deviations),
`FILTER_MORPHOLOGY` (two radii, dilate or erode) and `FILTER_TURBULENCE` (frequencies, octaves, seed, fractal noise
or turbulence). The content of `feImage` number `k` is drawn inside the filter layer, before the content filtered,
as a layer of opacity 0 whose filter is one `FILTER_IMAGE_SOURCE` node of transform `[k, 1, ..]`: replayed, the
filter layer renders each such layer over the same pixels as its source and hands them to the graph, and every
replay skips them elsewhere; another painter draws them invisible. Unit tests in `src/graph.rs` and
`src/primitives.rs`; WPT-shaped cases in pyro-render's `svg_filter_primitives.rs`.

## `backdrop-filter`

`src/backdrop.rs`. blitz-paint records two markers for an element with a backdrop filter, each an empty layer with
the filter and the element's border box (radii included) as the clip: a capture (opacity 0) where the element starts
to paint and a draw (opacity 1) first in its effect group. The capture renders what the element's backdrop root
painted before it (the commands since the innermost open layer that is a backdrop root: a filter, an opacity, a
blend mode, a backdrop filter's group, blitz-paint's empty-filter mark; else since the start of the scene or of the
filter layer replayed) into a context of its own, cut to the box (and to the context for the scene's root, as
Chromium's root surface is the viewport), filters it with the box mirrored past its edges (Chromium's edge mode: a
blur takes in no colour from outside the box) and keeps the images, keyed by the filter (one per element), the
element's transform and its box. The draw draws them in the box. An opaque, unfiltered group's backdrop is drawn
before the group in place of what lies there (Chromium draws it with a source copy: where the root painted nothing,
the page beneath shows, not the content under the box twice), and the group pushes nothing; a backdrop of one
colour is a fill of that colour, and a box on whole pixels needs no clip (each rounded a level apart under clips in
vello_cpu's u8 compositing). Scenes without a backdrop filter skip all of it. Covered by pyro-render's
`backdrop_filter.rs`.

## Clips that a blend mode blends through

`push_clip_layer` pushes a vello_cpu clip layer, an isolated group: a `mix-blend-mode` inside an
`overflow: hidden` box blended with nothing (the layer starts transparent), where CSS clips make no group and
Chromium's (Skia's) clips are no layers either. In a replayed scene that has a layer with a blend mode,
`replay` makes a clip whose content has such a layer (in it or in clips in it, not in another layer) a clip
path (vello_cpu's non-isolated clipping, `push_clip_path`/`pop_clip`; the painter keeps which kind each push
was for `pop_layer`). Every other clip stays a clip layer: a clip path clips each fill on its own, and where
fills overlap under an antialiased edge their coverage adds up (`clip-path` circles over a box and its border
came out up to 97 levels off with clip paths everywhere). What is a group in CSS (a stacking context with
blending descendants, a mask, a text clip) pushes a layer in blitz-paint. Covered by the unit tests in
`src/scene.rs`.

## Layers rasterized in their own plane

`src/scene.rs` (`draw_local_layer`, `is_local_raster_marker`, `local_raster_transform`, `painted_bounds`). A layer
whose filter is blitz-paint's local-raster mark (a single offset by negative zero, which moves nothing) holds a flat
element with a 3D transform and the elements in it. `replay` rasterizes its content, in a context of its own, in
the plane the layer's transform maps to the frame, over what it paints there that shows in the frame
(`painted_bounds`: a layer's clip cut to what the layer holds, as a blend's isolation layer has the viewport for a
clip; a filter or backdrop layer keeps its clip), at the larger of the scales the transform gives its axes
(Chromium's ideal contents scale for a layer; within 1e-4 of 1 it is 1), and draws the image through the transform,
sampled bilinearly, as Chromium composites the element. A bound within 1e-4 px of a whole pixel is on it: the
inverse's rounding put a box's edge a 1e-13th past its pixel, and the column added there was sampled into the edge.
A plane seen edge-on or too large (past 8192 px a side or 16M px) is replayed as it
is, and so is the content of a layer that captures a backdrop (a 3D transform makes no backdrop root, and a context
of the element's own does not hold what lies beneath it). Covered by pyro-render's `transform_3d.rs` and the unit
tests in `src/scene.rs`.

## Projective layers

`src/projective.rs` (new; `Homography`, `projective_marker`, `draw_projective_layer`), `src/scene.rs` (`replay`,
`painted_bounds`), lane H2. A layer whose filter carries a homography (six offsets: the first by negative zero,
then the matrix's columns' pairs and its last row) holds a plane drawn through a perspective: its commands are in
the plane's own pixels, the homography maps them to the layer's space. `replay` rasterizes them in a context of
their own over what they paint (only the part the target pixels show when all of them see the plane in front of
the viewer, at a lower scale past 16M px), and each frame pixel inside the layer's clip and the plane's projection
samples that raster bilinearly at the plane point the inverse homography gives its centre; where that point is
behind the viewer (W <= 0) nothing is drawn, as Chromium clips a quad at W = 0. The result is drawn pixel for
pixel. `painted_bounds` takes such a layer as its clip.

A second mark, `projective::is_aliased_clip_marker` (a layer whose filter is a single offset by `(0, -0)`, which
moves nothing), is a clip layer `replay` rasterizes aliased (`set_aliasing_threshold`): blitz-paint clips the pieces
of a plane split where planes of a 3D rendering context cross with it, so that two pieces share each pixel along
the cut without a seam, as viz draws the interior edges of split quads. `painted_bounds` takes what such a layer
holds, and what a layer whose filter has no effects holds (blitz-paint's backdrop-root mark, which paints nothing
of its own: as its clip it made a plane's raster region the size of the whole recording space). Covered by the unit
tests in `src/projective.rs` and pyro-render's `three_d_contexts.rs`.

## Aliased glyphs

`VelloCpuScenePainter::set_text_aliasing_threshold` (and `VelloCpuImageRenderer::set_text_aliasing_threshold`) draws
glyphs aliased: `draw_glyphs` sets vello_cpu's aliasing threshold for its run only (a pixel is painted fully when
the glyph covers at least that much of it, out of 255, else not at all) and clears it after, so every other shape
stays antialiased. The run is moved a 512th of a device pixel right and down, so that a pixel exactly half covered
is left out at a glyph's left or top edge and painted at its right or bottom edge, as Skia paints aliased paths (a
pixel centre on an edge rounds down and right), and a glyph at a half pixel spans the pixels a box there would.
`None`, the default, changes nothing. pyro-render passes 128 when `Renderer::text_antialiasing`
is off (`serve --text-antialiasing off`): WPT's runner starts Chrome with font antialiasing off
(`NoFontAntialiasing`), and the reftests are judged so. A filter layer's glyphs (`draw_filtered_layer` replays them
through `draw_glyphs`) follow the same setting. Covered by pyro-render's `text_aliasing.rs`.

Regenerate the patch from a pristine copy of the published crate:
`git diff --no-index --no-prefix a b` with the published crate in `a` and this directory, without this file
and the patch, in `b`; `git apply UPSTREAM.patch` in a pristine copy reproduces `src`.
