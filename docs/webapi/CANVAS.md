# Canvas 2D

`crates/lp-webapi/src/canvas.rs` implements `CanvasRenderingContext2D`
without a rasterizer: it keeps the state and geometry a page can observe,
and it draws no pixels. The value types are made in JS
(`crates/lp-webapi/src/canvas2d.js`) on a realm's first use of any of them:
`Path2D`, `ImageData`, `CanvasGradient`, `CanvasPattern`, `TextMetrics`,
`DOMMatrixReadOnly`, `DOMMatrix` (also `WebKitCSSMatrix`), `DOMPointReadOnly`
and `DOMPoint`. A realm that never uses them has one global accessor pair
per name; the first read replaces the accessor with the usual data property.

## Implemented

- `getContext('2d')` returns one context per canvas; other context types
  return null (no WebGL, no `bitmaprenderer`).
- Drawing state, saved and restored by `save()`/`restore()` and reset by
  `reset()` and by setting the canvas's `width` or `height`: `fillStyle` and
  `strokeStyle` (CSS colors serialized as Chrome does, `#rrggbb` or
  `rgba(...)`, or a `CanvasGradient`/`CanvasPattern` object), `globalAlpha`,
  `globalCompositeOperation`, `lineWidth`, `lineCap`, `lineJoin`,
  `miterLimit`, `setLineDash`/`getLineDash`, `lineDashOffset`, shadows,
  `font`, `textAlign`, `textBaseline`, `direction`, `letterSpacing`,
  `wordSpacing`, `fontKerning`, `fontStretch`, `fontVariantCaps`,
  `textRendering`, `filter`, `imageSmoothingEnabled`/`Quality`, and the
  current transform. Invalid values are ignored, as in browsers.
- Transforms: `translate`, `rotate`, `scale`, `transform`, `setTransform`
  (six numbers or a `DOMMatrix2DInit`), `resetTransform`, `getTransform`
  (a `DOMMatrix`).
- Paths: the current path is built with the transform applied when points
  are added (kurbo geometry): `moveTo`, `lineTo`, `quadraticCurveTo`,
  `bezierCurveTo`, `arcTo`, `arc`, `ellipse`, `rect`, `roundRect`,
  `closePath`, `beginPath`. `isPointInPath` (nonzero/evenodd, points on an
  edge are inside) and `isPointInStroke` (line width, caps, joins and miter
  limit; no dashes) answer for it and for a `Path2D`. `Path2D` records the
  same commands, `addPath(path, transform)` and SVG path data.
- `createLinearGradient`, `createRadialGradient`, `createConicGradient`
  (`addColorStop` validates its offset and color), `createPattern`
  (canvas, and loaded images; a video has no frame, so null).
- `measureText` returns a `TextMetrics` from the flow layout's text measure:
  width, bounding boxes from the font's ascent and descent, and the
  baselines relative to `textBaseline`.
- `ImageData` (constructor from a size or a `Uint8ClampedArray`,
  `colorSpace`), `createImageData`.
- `getContextAttributes()`, `isContextLost()` (false).
- `HTMLCanvasElement.toBlob(callback)` calls back with null in a task: there
  are no pixels to encode. `toDataURL()` returns `data:,`.

## Pixels

There is no rasterizer. `getImageData` returns transparent black while
nothing was drawn since the bitmap was cleared (created, resized, `reset()`
or a `clearRect` over the whole canvas with the identity transform), and
throws `NotSupportedError` once anything was drawn (`fill`, `stroke`, the
rect and text calls, `drawImage`, `putImageData`), so no page receives
invented pixels. Drawing calls accept their arguments and draw nothing.

## Gaps

- No pixels, so canvas fingerprinting, image export and pixel-based tests
  do not work; `toDataURL` is `data:,` for any canvas.
- Path2D SVG data with an error draws nothing (browsers keep the part
  before the error). `isPointInStroke` ignores the dash pattern.
- `clip()` does not affect hit testing; `drawFocusIfNeeded` draws nothing.
- Glyph bounding boxes are the font's ascent/descent, not per-glyph ink.
- No `OffscreenCanvas`, `ImageBitmap`, `createImageBitmap`, WebGL.
- `DOMMatrix` string parsing accepts absolute lengths and angles only (as in
  Chrome, relative units are a SyntaxError).
