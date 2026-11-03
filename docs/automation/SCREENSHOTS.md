# Screenshots: `Page.captureScreenshot` and `LP.run` `matchScreenshot`

Built with the `render` feature (`cargo build --release --features render`, off by default). lightningpanda has no
paint engine of its own; screenshots are rendered by **lp-render**, which wraps
[Blitz](https://github.com/DioxusLabs/blitz) 0.3.0-beta.4: html5ever, Stylo (Firefox's style engine), Taffy
(flex/grid/block layout), Parley (text), painted with Vello's CPU rasterizer (`vello_cpu`, 8 threads). The prototype
and the measurements that led here are on the `proto/blitz-screenshot` branch (`bench/screenshot-blitz/RESULTS.md`).

## Pipeline

1. **Snapshot** (browser thread, `PageDom::render_snapshot_tree`, about 0.02-0.4 ms on the fixtures below): the
   live DOM as a list of nodes keyed by their lp-dom identity (`Dom::render_tree`; `Dom::render_html` writes the same
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
   through the page's own loader (`Browser::fetch_subresource`): cookies, `LP.run` mocks, Fetch interception and
   the HTTP cache apply, and CDP sees the requests. The bytes are handed to Blitz on the render thread and cached
   for that document (loader id). The wait is capped at 10 s. The first layout waits for what the document asked
   for before it (sheets, `@font-face` fonts, images), up to 3 s past the sheets, so a page with web fonts is laid out
   once, with them.

   System fonts are scanned once per process and shared by every connection's renderer. `lightningpanda serve`
   loads them, with the rest of the process-wide render state (usvg's font database, Stylo's global data, font
   fallback), on a background thread at startup, so the first screenshot does not pay for it. Not in `--fork` mode,
   where connections are served by forks of the template process.
3. **Display list** (`anyrender` recording) and its xxh3 **scene hash**. Glyph fonts are hashed by content, so the
   hash is stable across processes.
4. **Raster** (vello_cpu) and readback to RGBA. The CPU backend is bit-exact across runs. The GPU backend (vello on
   wgpu/Metal, lp-render feature `gpu`) is not: in the prototype, 1-2 pixels flipped on some frames.
5. The result is queued to the browser thread, which wakes its loop and replies or settles the waiting step. The
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
  never took a screenshot. Nothing is recorded on the DOM mutation path: lp-dom's own change journal already exists
  for its style caches, and the list is built at screenshot time.
- Replies say how the document was used: `ms.mirror` is `{rebuilt, mutations, pages}` (`rebuilt`: `first`,
  `document`, `changes` or null when updated in place; `pages`: documents the connection's renderer holds).

The rendered result is the same as a fresh build of the same node list (lp-render's `mirror` tests compare scene
hashes after random mutation sequences). Known exception: blitz-dom has no anonymous table boxes, and when table
parts sit outside a table (a `<tr>` moved into a `<span>`, a `<td>` in a `<td>`) its incremental relayout can
differ from a fresh one. `bun bench/screenshot/repeat.ts` measures repeat screenshots.

## `Page.captureScreenshot`

`format` `png` (default) or `jpeg` (`quality`). `clip` is `{x, y, width, height, scale}` in document CSS px.
`captureBeyondViewport: true` captures the whole document. `webp` is refused; `fromSurface` and `optimizeForSpeed`
are ignored. The reply also carries `lp: {width, height, sceneHash, ms}`.

## `LP.run` step `matchScreenshot`

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
- The `lightningpanda-test` client stores baselines beside the tests (`__screenshots__/`), sends each one once per
  connection, and writes `.actual.png`/`.diff.png` on a failure (see its README and LLM.md).

## Not captured

- Canvas, video and WebGL pixels.
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
30.1 MB, M1 Max, sccache warm). lp-render uses blitz-dom with one patch (`vendor/blitz-dom`: class names interned
once per element instead of on every selector check; see its README). Builds without the feature are unchanged. Without it, `captureScreenshot` and
`matchScreenshot` fail with an explicit error.

The end-to-end benchmark (`bun bench/screenshot/match-screenshot.ts`) runs through the client, CDP and `LP.run` on
the three prototype fixtures, with Lato served as a web font. Numbers are in the PR that added this; the browser-side
split shows layout (Blitz) dominating, raster at 2.4-4.2 ms, and compare under 0.6 ms.

## Deferred full-page comparisons (v0.0.14)

The `lp-snapshot-compare` worker consumes immutable HTML captures and writes PNG-only baselines. It no longer reads or writes JSON scene-hash sidecars: every comparison checks pixels. Existing sidecars may be removed; missing baseline PNGs still fail unless update mode is explicitly requested. This applies to the deferred worker; the native synchronous matchScreenshot API is unchanged.

Full-page bounds respect root overflow clipping. A viewport-locked app with independently scrolling drawers can therefore have a full-page image equal to its viewport size; ordinary tall documents include content below the viewport.

HTML snapshot rendering resolves external SVG references to self-contained symbols through the provided resource loader, preserving SVG currentColor/stroke/viewBox behavior. Missing resources or symbols fail explicitly. The live-tree mirror path (screenshots, `matchScreenshot`, `printToPDF`) resolves them too, without changing the mirrored nodes: each external `<use href>` the mirror creates or changes is loaded once per resource scope and handed to Blitz, which adds the symbol to the `<svg>` when it renders it; a sprite that cannot be loaded draws nothing there (no error). Element scroll offsets (`scrollTop`/`scrollLeft` of overflow containers) are applied to the mirrored document before each capture. External symbols depending on definitions elsewhere in the sprite are not covered.

The engine now uses computed stylesheet positioning for offsetParent. This keeps overlay libraries from positioning a popover in viewport coordinates when its actual offset parent is a CSS-positioned drawer.
