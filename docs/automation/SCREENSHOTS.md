# Screenshots: `Page.captureScreenshot` and `Pyro.run` `matchScreenshot`

Built with the `render` feature (`cargo build --release --features render`, off by default). pyro has no
paint engine of its own; screenshots are rendered by **pyro-render**, which wraps
[Blitz](https://github.com/DioxusLabs/blitz) 0.3.0-beta.4: html5ever, Stylo (Firefox's style engine), Taffy
(flex/grid/block layout), Parley (text), painted with Vello's CPU rasterizer (`vello_cpu`). The prototype
and the measurements that led here are on the `proto/blitz-screenshot` branch (`bench/screenshot-blitz/RESULTS.md`).

## Pipeline

1. **Snapshot** (browser thread, `PageDom::render_snapshot_tree`, about 0.02-0.4 ms on the fixtures below): the
   live DOM as a list of nodes keyed by their pyro-dom identity (`Dom::render_tree`; `Dom::render_html` writes the same
   content as HTML):
   - Form state is written into the markup: input values, checkedness, textarea values and option selectedness.
   - `<style>` text comes from the style engine, so CSSOM edits are included; `<link rel=stylesheet>` keeps its
     absolute URL, and the sheet's text (as loaded, plus CSSOM edits) is passed beside it.
   - Scripts, `<noscript>`, `<template>`, comments and event-handler attributes are dropped.
   - Shadow trees are flattened, with slots filled from the host's light children.

   The snapshot also carries the emulated viewport, device scale factor and color scheme, and the window scroll
   position.
2. **Render thread** (one per CDP connection, started on first use): Blitz builds, styles and lays the document
   out. It keeps each page's document between screenshots (see [Repeat screenshots](#repeat-screenshots)). Sub-resources Blitz asks for (images, `@font-face` fonts, `@import`) are fetched back on the browser thread
   through the page's own loader (`Browser::fetch_subresource`): cookies, `Pyro.run` mocks, Fetch interception and
   the HTTP cache apply, and CDP sees the requests. The bytes are handed to Blitz on the render thread and cached
   for that document (loader id). The wait is capped at 10 s. The first layout waits for what the document asked
   for before it (sheets, `@font-face` fonts, images), up to 3 s past the sheets, so a page with web fonts is laid out
   once, with them.

   System fonts are scanned once per process and shared by every connection's renderer. `pyro serve`
   loads them, with the rest of the process-wide render state (usvg's font database, Stylo's global data, font
   fallback), on a background thread at startup, so the first screenshot does not pay for it. Not in `--fork` mode,
   where connections are served by forks of the template process.
3. **Display list** (`anyrender` recording) and its xxh3 **scene hash**. Glyph fonts are hashed by content, so the
   hash is stable across processes.
4. **Raster** (vello_cpu) and readback to RGBA. Frames of 4 M device px or more (3840x2160 is 8.3 M) are
   rasterized on vello_cpu's own threads, one less than the cores and at most 8 (`PYRO_RASTER_THREADS=n` sets the
   count, 0 keeps every frame on the render thread): 0.35-0.45 of the time for 20-60 % more CPU time (M1 Max). Smaller
   frames stay on the render thread, where threads would cost twice the CPU for little time saved. The pixels are the
   same either way. Many pages rendering at once (`serve --fork`, a page per process) get more frames out of the CPU
   with `PYRO_RASTER_THREADS=0`: a feature-video frame took 29 ms of CPU time on the render thread, 35 on threads. The CPU backend is bit-exact across runs. The GPU backend (vello on wgpu/Metal, pyro-render
   feature `gpu`) is not: in the prototype, 1-2 pixels flipped on some frames.
5. **Encode**: PNG, JPEG (libjpeg-turbo, 4:2:0 chroma at every quality as Chromium's; an image of 4 M px or more in
   bands on up to 4 threads, spliced with restart markers), or nothing for pyro's raw formats.
6. The result is queued to the browser thread, which wakes its loop and replies or settles the waiting step. The
   event loop never blocks on a render.

## Repeat screenshots

The render thread keeps one Blitz document per page that took a screenshot, and each later screenshot changes it
instead of building a new one, so Stylo restyles and Taffy re-lays out only what changed:

- The browser thread builds the node list again only when the page's render stamp moved (the DOM version, which
  covers tree, attribute, text and form-state changes, and the style engine's sheet generation). Otherwise it sends
  the previous list, and the render thread does no work for the document.
- The render thread compares the list with the previous one by key and applies the differences through blitz-dom's
  `DocumentMutator`: attributes, text, inserted, removed and moved nodes, `<style>` text, linked sheet text (CSSOM
  edits), viewport, device scale factor, color scheme and scroll. A `<textarea>` whose text changed and an
  `<input>` whose `type` changed are replaced, as blitz-dom sets them up only when it builds their box.
- A new document is built on a navigation (new loader id) or base URL, when more than half of the nodes were added
  or removed, and after a render panicked. A closed page's document is dropped; nothing is kept for a page that
  never took a screenshot. Nothing is recorded on the DOM mutation path: pyro-dom's own change journal already exists
  for its style caches, and the list is built at screenshot time.
- Replies say how the document was used: `ms.mirror` is `{rebuilt, mutations, pages}` (`rebuilt`: `first`,
  `document`, `changes` or null when updated in place; `pages`: documents the connection's renderer holds).

The rendered result is the same as a fresh build of the same node list (pyro-render's `mirror` tests compare scene
hashes after random mutation sequences). Known exception: blitz-dom has no anonymous table boxes, and when table
parts sit outside a table (a `<tr>` moved into a `<span>`, a `<td>` in a `<td>`) its incremental relayout can
differ from a fresh one. `bun bench/screenshot/repeat.ts` measures repeat screenshots.

## `Page.captureScreenshot`

`format` `png` (default) or `jpeg` (`quality`). `clip` is `{x, y, width, height, scale}` in document CSS px.
`captureBeyondViewport: true` captures the whole document. `webp` is refused; `fromSurface` and `optimizeForSpeed`
are ignored. The reply also carries `lp: {width, height, bytes, sceneHash, ms}`; `ms` has the stages (`snapshot`,
`fetch`, `layout`, `scene`, `raster`, `encode`, `write`, `total`, wall ms) and `cpu`, the browser process's CPU time
while it rendered (all its threads: with `serve --fork`, one page per process, that is the frame's).

JPEG is 4:2:0, as Chromium's at every quality (Chrome 147, q50-q100). 3840x2160, q95, M1 Max, CPU time per
screenshot: 144 / 72 / 79 ms before (jpeg-encoder, scalar 4:4:4) and 57 / 22.5 / 27.5 ms now on a photo / gradient /
text page; the JPEGs are 0.64 / 0.63 / 0.94 of the size.

**Raw frames** (pyro's formats, for ffmpeg's `-f rawvideo -pix_fmt <format> -s <width>x<height>`): `rgba` is the
render as it is (nothing converted), `rgb24` drops alpha, `yuv420p` is I420 (BT.601 limited range, chroma the mean
of each 2x2 block). `path` (absolute) appends the image to that file, or to a FIFO ffmpeg reads, instead of
returning it base64 in `data` (empty then); a FIFO without a reader fails at once, and a reader that takes nothing
for 30 s fails the capture. The browser opens and closes the path for each frame, so whoever streams through a
FIFO holds it open for writing in between (`FrameFifo` in `@povio/pyro`, with `page.captureFrame()`; wright:
`page.screenshot({ raw: { format, path } })`). Stepping a page frame by frame (set styles, capture) this way skips
image encoding and the WebSocket: a 3840x2160 frame of a page showing a 6168x3600 screenshot in a rounded stage
(the feature videos' scene, `lp-breakdown.mjs`) took 42-45 ms as PNG and 102-113 ms as JPEG before; now 15-25 ms as
PNG, 13-18 ms as JPEG, 6-8 ms as `rgba` and 11-15 ms as `yuv420p` (M1 Max under load, threaded raster; 31-34 ms of CPU
time as `rgba`).

## `Pyro.run` step `matchScreenshot`

```json
{"matchScreenshot": {"name": "home", "baseline": "<base64 PNG>", "sceneHash": "<hex>",
                     "threshold": 0, "maxDiffPixels": 0, "maxDiffPixelRatio": 0,
                     "clip": {"x": 0, "y": 0, "width": 100, "height": 100} | "fullPage": true | "locator": {...},
                     "update": false, "as": "key", "timeout": 5000}}
```

- **Baselines** are cached per `name` on the connection. The PNG is decoded on the render thread the first time a
  render is compared with it. Leave `baseline` out to use the cached one. Send it again to replace it: a different
  PNG digest replaces the cache entry.
- **Hash hit:** once a render matched a baseline exactly, the scene hash is stored with the baseline (or comes
  from `sceneHash`). A later render with that hash passes with no raster and no compare.
- **No baseline** (none sent, none cached, or `update: true`): the render is returned as `actual` with
  `recorded: true`.
- **Compare:** `threshold` (0-1) is the largest per-channel difference a matching pixel may have. Each pixel is
  compared with the one at the same position (no anti-aliasing detection, unlike Playwright's pixelmatch). The
  compare stops counting once the allowance is exceeded, then counts fully for the report.
- **Reply:** `{name, pass, hashHit, recorded, diffPixels, sceneHash, width, height, ms: {snapshot, decode, fetch,
  layout, scene, raster, compare, encode, total}, actual?}`.
- **Mismatch:** the run fails with the step's error. `error.data` holds the same fields plus `actual` and `diff`
  (base64 PNGs; differing pixels red over a gray copy).
- **`locator`:** captures the element's border box as Blitz lays it out. The element is resolved by the driver
  (retried until it exists) and marked in the snapshot.
- The `pyro` client stores baselines beside the tests (`__screenshots__/`), sends each one once per
  connection, and writes `.actual.png`/`.diff.png` on a failure (see its README and LLM.md).

## Deferred HTML captures

`captureOnly` HTML includes current inner `scrollLeft`/`scrollTop` offsets in reserved
`data-pyro-scroll-left` and `data-pyro-scroll-top` attributes. The renderer restores these offsets
when replaying the immutable document. Page-authored attributes with these names are ignored;
the live DOM is unchanged. This complements the existing scroll metadata used by live-tree captures.

Password values are replaced with masking bullets in both HTML and keyed render copies. Plain
text inputs retain their current values. Time inputs render their saved value instead of appearing
empty. The comparison worker accepts embedded `data:` resources without fetching them, while
retaining origin and filesystem-root checks for external resources.

## Top layer

Modal dialogs and showing popovers are painted above the document in top-layer order, each over its `::backdrop`
(see [POPOVER_DIALOG.md](../webapi/POPOVER_DIALOG.md#painting-screenshots-pdf)). Deferred HTML captures do not carry
the top layer.

## Not captured

- Video and WebGL pixels. (2D canvases are captured: their bitmaps travel
  with the snapshot and paint as the canvas element's content; see
  [CANVAS.md](../webapi/CANVAS.md).)
- Iframe contents (frames render as empty boxes).
- `:active` state, the caret, the selection. (`:hover` and `:focus` follow the page's pointer and focus.)
- Running CSS animations and transitions (rendered at their computed values).
- Shadow-tree style scoping: shadow `<style>` applies document-wide.
- `srcset`/`<picture>` selection is Blitz's own.

## Fidelity

Pixels are Blitz's, not Chromium's. A Chromium screenshot is never a usable baseline (on the prototype fixtures, 8-44%
of pixels differ exactly and 4-5% by more than 32 per channel). Known Blitz gaps at 0.3.0-beta.4:
- No synthetic bold.
- Synthetic italic slants the wrong way.
- Inline-box `padding`/`border-radius` are not painted.
- Table column widths differ from Chromium's.
- No text hinting.

## Cost

The `render` feature adds about 200 crates, about a minute to a release build and 15 MB to the binary (14.9 to
30.1 MB, M1 Max, sccache warm). pyro-render uses blitz-dom with one patch (`vendor/blitz-dom`: class names interned
once per element instead of on every selector check; see its README). Builds without the feature are unchanged. Without it, `captureScreenshot` and
`matchScreenshot` fail with an explicit error.

The end-to-end benchmark (`bun bench/screenshot/match-screenshot.ts`) runs through the client, CDP and `Pyro.run` on
the three prototype fixtures, with Lato served as a web font. Numbers are in the PR that added this; the browser-side
split shows layout (Blitz) dominating, raster at 2.4-4.2 ms, and compare under 0.6 ms.

## Deferred full-page comparisons (v0.0.14)

The `pyro-snapshot-compare` worker consumes immutable HTML captures and writes PNG-only baselines. It no longer reads or writes JSON scene-hash sidecars: every comparison checks pixels. Existing sidecars may be removed; missing baseline PNGs still fail unless update mode is explicitly requested. This applies to the deferred worker; the native synchronous matchScreenshot API is unchanged.

Full-page bounds respect root overflow clipping. A viewport-locked app with independently scrolling drawers can therefore have a full-page image equal to its viewport size; ordinary tall documents include content below the viewport.

HTML snapshot rendering resolves external SVG references to self-contained symbols through the provided resource loader, preserving SVG currentColor/stroke/viewBox behavior. Missing resources or symbols fail explicitly. The live-tree mirror path (screenshots, `matchScreenshot`, `printToPDF`) resolves them too, without changing the mirrored nodes: each external `<use href>` the mirror creates or changes is loaded once per resource scope and handed to Blitz, which adds the symbol to the `<svg>` when it renders it; a sprite that cannot be loaded draws nothing there (no error). Element scroll offsets (`scrollTop`/`scrollLeft` of overflow containers) are applied to the mirrored document before each capture. External symbols depending on definitions elsewhere in the sprite are not covered.

The engine now uses computed stylesheet positioning for offsetParent. This keeps overlay libraries from positioning a popover in viewport coordinates when its actual offset parent is a CSS-positioned drawer.
