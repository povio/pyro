# Screenshots: `Page.captureScreenshot` and `LP.run` `matchScreenshot`

Built with the `render` feature (`cargo build --release --features render`, off by default). lightningpanda has no
paint engine of its own; screenshots are rendered by **lp-render**, which wraps
[Blitz](https://github.com/DioxusLabs/blitz) 0.3.0-beta.4: html5ever, Stylo (Firefox's style engine), Taffy
(flex/grid/block layout), Parley (text), painted with Vello's CPU rasterizer (`vello_cpu`, 8 threads). The prototype
and the measurements that led here are on the `proto/blitz-screenshot` branch (`bench/screenshot-blitz/RESULTS.md`).

## Pipeline

1. **Snapshot** (browser thread, `PageDom::render_snapshot`, about 0.02-0.4 ms on the fixtures below): the live
   DOM as HTML (`Dom::render_html`):
   - Form state is written into the markup: input values, checkedness, textarea values and option selectedness.
   - `<style>` text comes from the style engine, so CSSOM edits are included; `<link rel=stylesheet>` keeps its
     absolute URL, and the sheet's text (as loaded, plus CSSOM edits) is passed beside it.
   - Scripts, `<noscript>`, `<template>`, comments and event-handler attributes are dropped.
   - Shadow trees are flattened, with slots filled from the host's light children.

   The snapshot also carries the emulated viewport, device scale factor and color scheme, and the window scroll
   position.
2. **Render thread** (one per CDP connection, started on first use): Blitz parses, styles and lays the snapshot
   out. Sub-resources Blitz asks for (images, `@font-face` fonts, `@import`) are fetched back on the browser thread
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
- Element scroll offsets (only the window's).
- `:hover`, `:focus` and `:active` state, the caret, the selection.
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
