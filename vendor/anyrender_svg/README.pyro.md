# anyrender_svg 0.15.0, patched

The published `anyrender_svg` 0.15.0 crate (crates.io; part of
[dioxuslabs/anyrender](https://github.com/dioxuslabs/anyrender), `crates/anyrender_svg` at commit
`cf0f14776102ec775af61bcdcaac2ab345f4fafb`), used through `[patch.crates-io]` in the workspace `Cargo.toml`.
License: MIT OR Apache-2.0, as upstream. `.cargo_vcs_info.json`, `Cargo.toml.orig` and `.cargo-ok` were left
out of the copy.

## The change

`UPSTREAM.patch` is the whole diff: `src/render.rs` is rewritten around the same structure, `src/filter.rs` is
new, `src/lib.rs` passes the tree's viewport (and exports the filter graphs and `render_svg_tree_with_notes`), and
`Cargo.toml` depends on `anyrender_vello_cpu` (the vendored one)
for offscreen rasters and its filter conventions. Upstream draws a translucent red box for a pattern, clips with the first shape of a `<clipPath>` only
(and only when it is a path), and ignores masks; pyro paints SVG content as Chromium does (WPT
`css/css-transforms/matrix`, `css/css-masking/clip-path-svg-content`, `css/css-masking/mask-svg-content`):

- **Patterns** (`<pattern>` fills and strokes): the tile is rendered offscreen at the scene's resolution (the
  scale of the painted path's transform with `patternTransform`, rounded to whole pixels across the tile, at most
  4096 x 4096 px) with the same rasterizer as the page, and drawn as a repeated image brush mapped onto the
  tile's rectangle, with the paint's opacity. The tile's content is drawn from the tile's origin, as resvg does.
- **Clip paths** of any content: a clip path that is one path (directly or in groups without clip paths) and has
  no clip path of its own is one layer clipped to it, as before (now with the clip path's and the shape's
  transforms).
  Any other is applied as a `DestIn` layer over the clipped group in an isolated layer: each shape is filled
  opaque with its clip rule (text as its glyph outlines, `<use>` as its content), a shape with a clip path of its
  own is clipped in a layer of its own, and a clip path's own clip path applies as a second `DestIn` layer. The
  `DestIn` layer covers the isolated layer's whole extent (a destructive layer leaves what it does not cover
  alone).
- **Masks** (`<mask>`, `mask-type` luminance or alpha, a mask's own mask): the mask's content, clipped to its
  rectangle, is rendered offscreen over the masked group's extent (whole device pixels, within the tree's
  viewport) and turned into coverage (luminance 0.2125 R + 0.7154 G + 0.0721 B of the premultiplied colour, or
  alpha), then drawn into a `DestIn` layer as an image. A mask of up to a megapixel is rendered at twice the
  resolution and averaged down: vello flattens curves to a quarter pixel, which the image would keep (one circle
  given as four or as sixteen cubics came out up to 39 levels apart at its edge). A mask on the mask element
  multiplies the content too.
- A group that is one filled axis-aligned rectangle holding the shapes of its clip path (a background cut to a
  shape, the usual clip reftest) draws the shapes filled with the rectangle's paint instead (several shapes only
  with an opaque colour, whose overlaps then look as their union's): their edges are then anti-aliased as filled
  shapes', as a reference that draws the shapes has them (a clip layer's coverage rounds a level apart from a
  fill's). Text in a clip path is its glyph outlines.
- **Filters** (`filter`, a `<filter>`'s primitives or filter functions; `src/filter.rs`): each of a group's
  filters is a layer around its children (the first innermost), clipped to its filter region, its filter usvg's
  converted into an anyrender filter graph, which anyrender_vello_cpu applies when it replays the scene (its
  `graph` module): `feFlood`, `feOffset`, `feGaussianBlur` (one deviation or two), `feDropShadow` of one deviation,
  `feColorMatrix` (every type, Chromium's constants), `feComponentTransfer`, `feMerge`, `feComposite`, `feBlend`
  (every mode), `feDisplacementMap`, `feTile`, `feMorphology`, `feConvolveMatrix`, `feTurbulence`,
  `feDiffuseLighting` and `feSpecularLighting` (distant, point and spot lights) and `feImage`, with subregions as
  crops; what anyrender's filter types cannot say follows anyrender_vello_cpu's conventions (see its README). Each
  primitive works in its `color-interpolation-filters` space: an input in the other space is read through a
  `feComponentTransfer` of conversion tables, a value converted back is the value it came from, a flood (and a
  lighting colour) is drawn in its reader's space, and the output is sRGB; `feOffset`, `feTile` and the input
  `feDisplacementMap` moves are taken as they are, `feImage` is sRGB. `feTile` tiles its input's subregion.
  `feImage` content (an element, or an image usvg loaded) is drawn in the filter layer, before the content, in a
  layer of opacity 0 that only the graph reads, at the subregion's corner (as Blink places it; an image usvg laid
  out in the subregion). Children that reach past the first filter's region are clipped to it (SVG's source
  graphic stops there; a CSS filter function's layer is no region, see below: `FilterGraph::bounded`). A filter
  with a primitive anyrender_vello_cpu does not do (`feDropShadow` of two deviations), or with the transparent black
  flood usvg stands in for a primitive it could not read (an `feImage` it could not load), is left out: the group is
  drawn unfiltered, as before; a flood the caller says is a real `feFlood` (`PrimitiveNotes::flood`: Figma's
  `<feFlood flood-opacity="0" result="BackgroundImageFix"/>`) is a transparent flood. `FilterGraph::reach` is how far
  the graph's output can lie from its content (for its painter's culling).
- **Tainted filter primitives** (`src/filter.rs`, `PrimitiveNotes`, `render_svg_tree_with_notes`): a flood, drop
  shadow or lighting colour computed from `currentcolor` (which a `:visited` link's colour can be) is tainted, and so
  is everything made from it; an `feDisplacementMap` whose map is tainted passes its input on unmoved, as Chromium's
  does (WPT css/filter-effects/tainting-*), and its result is tainted (Blink checks every input). usvg resolves `currentcolor`, so the caller passes what the document
  knows (blitz-paint reads the `<filter>` elements); a filter in a list after a tainted one reads a tainted source.
  The caller also says which lighting primitive has no light source: its result is transparent black (Chromium),
  where usvg's stand-in would leave the filter out. `FilterGraph` is public for blitz-paint, which applies SVG
  filters to HTML elements with it. Mask and pattern rasters are recorded and replayed
  (`append_scene`) as the page is, so that filters apply in them too (WPT `svg-mutation-filter-used-by-mask`).
  Covered by the unit tests in `src/filter.rs` and pyro-render's `svg_filters.rs`.
- **Filter functions** (`filter="drop-shadow(...)"`, CSS `filter: blur(...)` on an SVG element): usvg gives one
  a filter of one primitive named `result` over the element's bounding box widened by half (a blur, a drop
  shadow) or a tenth, a stand-in for the region CSS filter functions do not have, which cut
  `drop-shadow(50px 0)` on a 50 px wide shape off at 25 px (WPT `svg-shorthand-drop-shadow-001`,
  `svg-mutation-*`). Such a filter (`function_kind`: an id usvg generated, `filterN`, and that primitive over
  exactly that region; a `<filter>` written so is taken for one) is drawn in a layer that reaches as far as its
  effect does from what the children draw and two device pixels around it (a drop shadow adds its offset, a blur
  three deviations; a clip at the content's edge lightened its anti-aliased pixels), and the content is not
  clipped to it (`filter_regions` gives each filter's layer region and whether it clips the source graphic). A
  group's layer bounds (`layer_bounds`) take the same reach.
- **Masks of opaque shapes from a box**: in the document blitz-dom makes for a CSS box's `mask-image: url(#m)`
  (its content in a group of id `__pyro_box_reference`), a mask whose content is shapes in one opaque colour inside
  its rectangle (no images, no groups with effects but a clip path of one shape, which clips them in a layer, or is
  filled itself over a rectangle holding it) is drawn into the `DestIn` layer as those shapes (in scene
  coordinates), in a layer of the colour's mask
  value, instead of as a raster: a page drawing the same `<circle>` then has the same edge (the raster, at twice
  the resolution, was up to 52 levels apart from it: WPT `mask-image-3a`...`3e`). Masks in SVG content keep the
  raster: their references are other masks, of other descriptions of a shape (a circle as `r="25"` scaled by 2 and
  as `r="5000"` by 0.01, 39 levels apart drawn directly), which its supersampling brings within their tolerance
  (WPT `mask-*-content-clip-transform`). blitz-dom's `svg_reference_document` writes the group. Whether a render
  is inside it is a scoped thread-local (`IN_BOX_REFERENCE`, cleared in offscreen rasters), not a field of `Ctx`,
  so that code building a `Ctx` from its `cull` stays as it is.
- **Operator groups**: a group whose id is `__pyro_compose_<operator>_<n>` (blitz-dom `svg_css` writes them for CSS
  mask layers and `mix-blend-mode: plus-lighter`, which usvg does not know) is composited with that operator
  (`srcout`, `srcin`, `xor`, `pluslighter`) over all the output shows (`compose_marker`). `has_blend_modes` tells
  whether a tree (an SVG image) blends, for its painter to isolate it.
- Groups keep the order resvg applies: children, filters, clip path, mask, then opacity and blend mode.
- Nested trees (`<image>` of an SVG, text, subtrees of clip paths, masks and patterns) are drawn with their own
  base transform; upstream reset it to the identity below the first group, which misplaced a group inside one.

Offscreen rasters use `VelloCpuImageRenderer` on the calling thread. PDF output (pyro-render `pdf.rs`) leaves
`DestIn` layers out: masks and clip paths of several shapes are not applied there.

## A mask in user units that a box references (second pass, lane P6)

- `src/lib.rs` (`render_svg_tree_within`): renders a tree with the visible area of the scene as the bound of its
  offscreen rasters, in place of the tree's viewport. blitz-paint draws a `<mask maskUnits="userSpaceOnUse">` that a
  CSS box references with it: the mask's content may lie above or left of the box's corner (the tree's origin), over
  the box's shadow, where the viewport bound cut the mask's raster to nothing (WPT `mask-image-3f`).

Regenerate the patch from a pristine copy of the published crate:
`git diff --no-index --no-prefix a b` with the published crate in `a` (without `.cargo_vcs_info.json`,
`Cargo.toml.orig` and `.cargo-ok`) and this directory, without this file and the patch, in `b`, run from their
parent; `git apply UPSTREAM.patch` in a pristine copy reproduces `src` and `Cargo.toml`.
- `src/filter.rs` (`PrimitiveNotes::element_image`): an `feImage` of an element of the document that usvg could not draw (its transparent flood stand-in) is transparent, not a reason to leave the filter out (WPT filter-effects `svg-feimage-004`).
- `src/lib.rs`, `src/render.rs`, `src/util.rs` (`ForeignObject`, `foreign_object`, `foreign_object_id`,
  `render_svg_tree_with_notes_and`, `draw_error_box`): an `<image>` whose PNG data carries a `pyFo` chunk is a
  `<foreignObject>`'s stand-in (blitz-dom `svg_css::foreign_object_href`); the error handler is called for it with
  `foreign_object()` set to its id, transform and size, and blitz-paint paints the element's HTML content there. The
  default handler draws nothing for one.
