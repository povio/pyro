# Canvas 2D

`crates/lp-webapi/src/canvas.rs` implements `CanvasRenderingContext2D`,
`OffscreenCanvas` 2D contexts, `ImageBitmap` and canvas pixels. Pixels are
painted by `crates/lp-canvas` (vello_cpu on the CPU, the same rasterizer
screenshots use). The value types are made in JS
(`crates/lp-webapi/src/canvas2d.js`) on a realm's first use of any of them:
`Path2D`, `ImageData`, `CanvasGradient`, `CanvasPattern`, `TextMetrics`,
`ImageBitmap`, `OffscreenCanvas`, `OffscreenCanvasRenderingContext2D`,
`createImageBitmap`, `DOMMatrixReadOnly`, `DOMMatrix` (also
`WebKitCSSMatrix`), `DOMPointReadOnly` and `DOMPoint`. A realm that never
uses them has one global accessor pair per name; the first read replaces the
accessor with the usual data property.

## Bitmaps and memory

- Each 2D context owns an `lp_canvas::Surface`. Its bitmap (premultiplied
  RGBA) is allocated on the first draw or pixel write. Reading a canvas that
  was never drawn on (`getImageData`, `toDataURL`, `drawImage` of it)
  allocates nothing and reads transparent black.
- Setting `width`/`height` (the IDL attributes, or `setAttribute`) clears and
  releases the bitmap and resets the context state.
- Source-over draws without a shadow are batched into one vello scene and
  rasterized onto the bitmap when pixels are needed (a read, a snapshot,
  another operator, or 4096 batched draws). Other operators and shadows
  render the source image alone and composite it per pixel.
- Limits: a side of at most 16384 px and 32 Mi pixels per bitmap (larger
  canvases draw nothing and read transparent); 2 GiB of bitmaps per process,
  counted for as long as any holder of the pixels lives (canvas bitmaps,
  snapshots, `ImageBitmap`s, patterns, decoded images): past it a draw that
  needs a new bitmap draws nothing and an image source that needs decoding
  has no pixels; `getImageData` of more than 32 Mi pixels is a `RangeError`.
- Hostile geometry is bounded: paths with points far outside the bitmap are
  flattened and clipped to a box that far out, pens are at most a few canvas
  sizes wide, more than 100000 dash periods along one stroke draw it solid
  (Skia's behaviour past its limit), glyphs over 100000 device px draw
  nothing.

## Implemented

- `getContext('2d')` returns one context per canvas; other context types
  return null (no WebGL, no `bitmaprenderer`).
- Drawing state, saved and restored by `save()`/`restore()` and reset by
  `reset()` and by resizing: `fillStyle`/`strokeStyle` (CSS colors
  serialized as Chrome does, or a `CanvasGradient`/`CanvasPattern`),
  `globalAlpha`, `globalCompositeOperation` (all 26: the Porter-Duff
  operators, `lighter`, and the W3C blend modes), `lineWidth`, `lineCap`,
  `lineJoin`, `miterLimit`, `setLineDash`/`getLineDash`, `lineDashOffset`,
  shadows (`shadowColor`, `shadowBlur` as a three-box-blur Gaussian,
  offsets in device pixels), `font`, `textAlign`, `textBaseline`,
  `direction`, `letterSpacing`, `wordSpacing`, `imageSmoothingEnabled`
  (bilinear or nearest), the current transform and the clipping region.
  `fontKerning`, `fontStretch`, `fontVariantCaps`, `textRendering`,
  `imageSmoothingQuality` and `filter` are kept and do not change pixels.
- Transforms: `translate`, `rotate`, `scale`, `transform`, `setTransform`,
  `resetTransform`, `getTransform`. Strokes are traced in user space and
  transformed, so non-uniform scales shear the pen.
- Paths (kurbo geometry, built with the transform applied):
  `moveTo`, `lineTo`, `quadraticCurveTo`, `bezierCurveTo`, `arcTo`, `arc`,
  `ellipse`, `rect`, `roundRect`, `closePath`, `beginPath`; `fill` and
  `clip` (nonzero/evenodd), `stroke`, for the current path or a `Path2D`
  (commands, `addPath`, SVG path data). `isPointInPath`/`isPointInStroke`.
- `fillRect`, `strokeRect` (a zero width or height strokes a line),
  `clearRect` (within the clipping region).
- `createLinearGradient`, `createRadialGradient` (two-point conical),
  `createConicGradient`; stops interpolate unpremultiplied as HTML says,
  stops added after a gradient was set apply to later draws.
  `createPattern` with every repetition and `CanvasPattern.setTransform`;
  the pattern captures the source's pixels when it is made.
- Text: `fillText`, `strokeText` (with `maxWidth` squeezing) and
  `measureText` shape one line with Parley on the system fonts (the fonts
  page rendering uses, `lp_canvas::fonts`): advance width, the primary
  font's ascent/descent, ink bounds from the glyph outlines, baselines for
  `textBaseline`. Whitespace becomes spaces as HTML's text preparation says.
  Font fallback, synthetic bold/oblique and variable fonts come from Parley.
- `drawImage` (3, 5 and 9 arguments) and `createPattern` from: a loaded
  `<img>` (decoded on first use: PNG, JPEG, GIF first frame, WebP), a
  `<canvas>` (its bitmap now, itself included), an `ImageBitmap`, an
  `OffscreenCanvas`, a `<video>` (the frame shown now, decoded by lp-media
  for AV1 and H.264; nothing before `HAVE_CURRENT_DATA` or for other codecs;
  cross-origin video without CORS taints). An `<img>` still loading draws nothing and a broken one
  throws `InvalidStateError`; a zero-sized canvas throws; a closed
  `ImageBitmap` throws.
- `createImageData`, `getImageData` (negative sizes, outside areas read
  transparent), `putImageData` (dirty rectangle; no compositing, transform
  or clip), `ImageData` with `colorSpace`. Pixels read back unpremultiplied
  with Chrome's rounding.
- `HTMLCanvasElement.toDataURL(type, quality)` and `toBlob(callback, type,
  quality)`: PNG, JPEG (`image/jpeg`, transparent pixels over black,
  quality 0-1, default 0.92) and WebP (`image/webp`, lossless); other types
  give PNG. A canvas without pixels (zero-sized or over the limits) gives
  `data:,` and a null blob. `toBlob` encodes at the call and calls back in a
  task.
- `createImageBitmap(source[, sx, sy, sw, sh][, options])` from the image
  sources above, an `ImageData` or a `Blob` (decoded), with crop,
  `resizeWidth`/`resizeHeight`/`resizeQuality` and
  `imageOrientation: 'flipY'`. `ImageBitmap.width/height/close()`.
- `OffscreenCanvas(width, height)`: `width`/`height` (resizing clears),
  `getContext('2d')` (one context; `webgl`/`bitmaprenderer` are null),
  `convertToBlob({type, quality})`, `transferToImageBitmap()`. Its context
  is a `CanvasRenderingContext2D` whose `canvas` is the OffscreenCanvas;
  `instanceof OffscreenCanvasRenderingContext2D` recognizes it.
- Origin-clean: drawing a cross-origin `<img>` (one whose final response URL
  is not same-origin with the document's origin, which is opaque in a
  sandboxed frame and inherited in `about:blank`/`about:srcdoc`; `data:`
  and `blob:` are clean), or
  a pattern, canvas or bitmap made from one, taints the canvas:
  `getImageData`, `toDataURL`, `toBlob` and `convertToBlob` throw
  `SecurityError`.
- `getContextAttributes()`, `isContextLost()` (false).
- Workers: `OffscreenCanvas` with its 2D context, `ImageBitmap`,
  `createImageBitmap`, `ImageData`, `Path2D`, the gradient, pattern and
  text-metric types and the geometry interfaces, made on the worker realm's
  first use as on a window. Text uses the same system fonts.
- Structured clone and transfer: an `ImageBitmap` clones (a new one over
  the same pixels, also across threads) unless it is closed or not
  origin-clean; in a transfer list it moves and the original is closed. An
  `OffscreenCanvas` without a context transfers (`postMessage` to or from a
  worker, a `MessagePort`, `structuredClone`): the new one has its size, and
  the original is detached (size 0, InvalidStateError on use). See
  [STRUCTURED_CLONE](STRUCTURED_CLONE.md).
- `transferControlToOffscreen()`: the `<canvas>` (which must have no context;
  a second call throws InvalidStateError) becomes a placeholder for a new
  `OffscreenCanvas` of its size, on this thread or transferred to a worker.
  After each task of the thread drawing on it, a changed bitmap (or a
  resize) is committed to the placeholder: the pixels are shared, not
  copied. The placeholder shows the last committed frame in screenshots
  (`PageDom::canvas_bitmaps`, at the frame's own size, scaled by CSS), to
  `drawImage`/`createPattern`/`createImageBitmap` and to `toDataURL`/`toBlob`;
  its `getContext` throws InvalidStateError. A page without placeholders
  pays one empty-list check per task.
- Screenshots: render snapshots carry the bitmaps of drawn canvases
  (`PageDom::canvas_bitmaps`), and the render mirror gives each canvas
  element its bitmap as raster content, so `Page.captureScreenshot` (and
  PDFs of a live page) show canvases, scaled by CSS like an image.
  Unchanged bitmaps are passed as the same `Arc` and not copied again.

## How `<img>` pixels reach a canvas

lp-browser fetches `<img>` resources to learn their natural size and drops
the body. When the document may draw images (it created a `<canvas>`
element, a flag set at element creation, or used a canvas API), the body is
kept in `PageDom` (`set_image_bytes`, at most 64 MiB per document, a
running total) with its origin-clean flag, and decoded on the first
`drawImage`/`createPattern`/`createImageBitmap` of the image (the decoded
pixels are then kept with it while the document's cached decodes fit
256 MiB; past that an image is decoded again on each use). Pages without
canvases pay a flag test per image load.

## Gaps

- An `<img>` that finished loading before the document had a `<canvas>` or
  used a canvas API, and images past the 64 MiB budget, draw nothing.
- SVG images, and video frames of codecs without a decoder (VP8, VP9,
  HEVC), draw nothing (`createPattern` returns null for them);
  `<img crossorigin>` CORS loads are not made, so every
  cross-origin image taints.
- `filter` is kept but not applied; `imageSmoothingQuality` does not change
  the (bilinear) filter; `fontKerning`, `fontStretch`, `fontVariantCaps`,
  `textRendering` are not applied. `direction: inherit` is left-to-right.
- `measureText`'s em-height and hanging/ideographic baselines are derived
  from ascent/descent, not from the font's BASE table.
- Shadow offsets are rounded to whole pixels.
- No WebGL or `bitmaprenderer`. `drawFocusIfNeeded` draws nothing. Workers
  have no `requestAnimationFrame`: an offscreen canvas commits after each
  task instead. Setting a placeholder `<canvas>`'s `width`/`height` does not
  throw. A frame cleared by a resize shows as no content.
- `display-p3` ImageData holds sRGB values. `ImageData` is not
  serializable (`postMessage`, `structuredClone` throw DataCloneError).
- Path2D SVG data with an error draws nothing (browsers keep the part
  before the error). `isPointInStroke` ignores the dash pattern;
  `clip()` does not affect hit testing.
- `DOMMatrix` string parsing accepts absolute lengths and angles only (as in
  Chrome, relative units are a SyntaxError).
