# Screenshots and PDFs

Pyro renders pages itself, on the CPU: Stylo computes styles, Taffy and Blitz lay out, Parley shapes text and Vello's CPU rasterizer paints. No GPU, no Chrome and no display server are needed, so PDFs and screenshots work on a plain Linux server or in a container. The browsers in the npm package are built with rendering included.

The output is close to Chrome's, not identical. PDF page geometry follows Chrome's measured rules, but layout can differ by a few pixels, and screenshots do not match Chromium's pixel for pixel. For background, see [Rendering: screenshots and PDF](/guide/rendering).

## PDFs

### With the Playwright library

`page.pdf()` passes its options straight to CDP `Page.printToPDF`. **Use the CDP parameter names, with sizes in inches.** Playwright's own `format`, `width`, `height` and `margin` options are not translated: they are ignored, and you get US Letter with 0.4 in margins.

```ts
import { chromium } from '@povio/pyro/wright';

const browser = await chromium.launch();
const page = await browser.newPage();
await page.goto('http://127.0.0.1:4500/report');
await page.emulateMedia({ media: 'print' }); // apply @media print rules (see below)
await page.pdf({
  path: 'out/report.pdf',
  paperWidth: 8.27, // A4, in inches
  paperHeight: 11.69,
  marginTop: 0.4,
  marginBottom: 0.4,
  marginLeft: 0.4,
  marginRight: 0.4,
  printBackground: true,
});
await browser.close();
```

`page.pdf()` returns the PDF as a `Buffer`; `path` also writes it to a file. For many URLs at once, see [A batch of URLs to PDF](/recipes/scraping-pdf-batch).

### With Puppeteer or raw CDP

Puppeteer converts its options (`format: 'A4'`, `margin: { top: '1cm' }`) to the CDP parameters before sending them, so `page.pdf({ format: 'A4' })` from Puppeteer gives an A4 page. In the native SDK, send the command yourself:

```ts
import { writeFile } from 'node:fs/promises';

const { data } = await page.send('Page.printToPDF', { paperWidth: 8.27, paperHeight: 11.69, printBackground: true });
await writeFile('out/report.pdf', Buffer.from(data, 'base64'));
```

### Parameters

| Parameter | Default | Notes |
| --- | --- | --- |
| `paperWidth`, `paperHeight` | `8.5`, `11` (US Letter) | Inches. |
| `marginTop`, `marginBottom`, `marginLeft`, `marginRight` | `0.4` | Inches. |
| `landscape` | `false` | Swaps width and height. |
| `scale` | `1` | `0.1` to `2`. |
| `printBackground` | `false` | Without it, background colors and images are left out, as in Chrome. |
| `pageRanges` | all | `"1-3, 5, 7-"`. Errors: `Page range syntax error`, `Page range exceeds page count`. |
| `generateTaggedPDF` | `true` | Writes a structure tree (headings, paragraphs, links). |
| `preferCSSPageSize` | `false` | Applies to Paprize page boxes only (see below). |
| `transferMode` | `ReturnAsBase64` | `ReturnAsStream` returns an `IO` stream handle to read with `IO.read`. |
| `displayHeaderFooter`, `headerTemplate`, `footerTemplate`, `generateDocumentOutline` | | Accepted and not produced: no headers, footers or outline. |

Common paper sizes in inches:

| Paper | `paperWidth` | `paperHeight` |
| --- | --- | --- |
| A4 | 8.27 | 11.69 |
| A3 | 11.69 | 16.54 |
| A5 | 5.83 | 8.27 |
| US Letter | 8.5 | 11 |
| US Legal | 8.5 | 14 |

Paper is rounded as Chrome does: an A4 page comes out as 595.92 x 841.92 pt, the same as Chrome's.

### Print media

Pyro prints with the page's emulated media, which is `screen` unless you set it, so `@media print` rules do not apply by default. Chrome prints with print media. To match Chrome, emulate print first:

```ts
await page.emulateMedia({ media: 'print' }); // CDP: Emulation.setEmulatedMedia { media: 'print' }
```

In the native SDK: `await page.send('Emulation.setEmulatedMedia', { media: 'print' })`.

In a test, a paragraph hidden by `@media print { .noprint { display: none } }` was in the PDF without this call and absent with it.

### Page breaks

Without page boxes, the document is laid out at the paper's content width and cut into pages:

- Cuts move up to the top of a line, an image, a form control, an inline block or a `break-inside: avoid` box instead of splitting it, with Chrome's default of 2 orphans and widows.
- `break-before` / `break-after: page` (and the older `page-break-*`) force a cut; `avoid` moves it.
- Not supported: `@page` rules, repeated table headers on each page, CSS fragmentation (a box is not split and laid out again, it is cut), `position: fixed` on every page (it appears once).

For full control over pages, lay them out yourself as boxes with the class `pz-page` (the convention of the Paprize paged-media library). Then each `.pz-page` box becomes one PDF page, and `preferCSSPageSize: true` writes each page at the box's own size instead of placing it on the paper.

### Fonts and images

Before writing the PDF, the browser waits up to 10 seconds for the web fonts and images the layout needs. Text uses `@font-face` fonts the page loads, else the system's fonts. Variable fonts are embedded at each instance used, so a `font-weight: 700` heading prints bold.

On Linux, the browser finds system fonts through fontconfig. A minimal server or container image may have no fonts at all; install some, for example `fonts-dejavu-core` and `fonts-liberation` on Ubuntu (see the Docker example in [Running as a service](/scraping/service#docker)). Pages that name fonts the system lacks fall back to what is installed, so for repeatable output load your fonts with `@font-face`.

The first PDF or screenshot of a server scans the system fonts and warms the renderer. `--render-prewarm on` does that at startup instead (about 3 MB while idle; on by default with `--prepare`).

### Hyphens and digits: `--hyphen-digit-breaks`

Long identifiers such as `REG-be08b23e-89be-48b8-9a6f-...` break differently in Chrome and in the Unicode line breaking rules: Chrome allows a break between a hyphen and a digit, Unicode does not. Pyro follows Unicode by default, so such a string can overflow a narrow table cell where Chrome would wrap it. To wrap like Chrome:

```sh
pyro serve --hyphen-digit-breaks on
# or: PYRO_HYPHEN_DIGIT_BREAKS=on
```

```ts
const browser = await chromium.launch({ args: ['--hyphen-digit-breaks', 'on'] });
```

A break is then allowed after a hyphen that follows a letter or digit and precedes a digit (`10-` / `20`). A minus sign stays with its number (`-120,00`, `A -5`). Table column widths follow the same rule.

### PDFs from your own HTML

To turn generated HTML into a PDF (invoices, reports), set it as the page's content:

```ts
const page = await browser.newPage();
await page.setContent(html); // waits for load
await page.pdf({ path: 'out/invoice.pdf', paperWidth: 8.27, paperHeight: 11.69, printBackground: true });
```

Relative URLs in `html` have no base; use absolute URLs, `data:` URLs, or a `<base href>`. To serve a whole directory of templates and assets without a web server, see `--static` in [Serving a build](/e2e/static).

## Getting text back out of a PDF

The PDFs carry real text, so they are searchable and extractable. Each glyph maps back to the characters it was shaped from: ligatures (`fi`), tabular figures, small caps and other OpenType substitutions extract as the original text. Links are clickable (`<a href>` becomes a link annotation; `#id` links jump within the document).

With poppler's `pdftotext`:

```sh
pdftotext -layout out/report.pdf - | head
```

Use `-layout`: without it, `pdftotext` joins a word broken across lines at a hyphen and drops the hyphen, which corrupts identifiers like the one above. From JavaScript, [pdf.js](https://mozilla.github.io/pdf.js/) (`pdfjs-dist`) reads the same text with positions.

Text clipped by `overflow: hidden` or beyond the page's sides is not written, as in Chrome. List bullets are drawn as shapes, not text.

## Screenshots

```ts
await page.screenshot({ path: 'out/page.png' }); // the viewport
await page.screenshot({ path: 'out/full.png', fullPage: true }); // the whole document
await page.screenshot({ path: 'out/top.jpg', type: 'jpeg', quality: 70, clip: { x: 0, y: 0, width: 400, height: 200 } });
await page.locator('h1').screenshot({ path: 'out/heading.png' }); // one element's box
```

The Playwright library passes `path`, `type` (`png` or `jpeg`, also taken from the file extension), `quality`, `clip` (document CSS pixels), `fullPage` and `omitBackground` (a transparent background, PNG only). `scale`, `animations`, `mask` and `caret` are not passed on.

The image is in device pixels. For 2x or 3x images, make the context with `deviceScaleFactor: 2` or `3`: a 1920 × 1080 viewport at `deviceScaleFactor: 2` gives a 3840 × 2160 image. Other clients set it with CDP `Emulation.setDeviceMetricsOverride` (see [Proxies, user agents, headers](/scraping/identity#viewport-pixel-ratio-mobile)). In releases up to 0.0.33 the library does not pass `omitBackground`, and its `deviceScaleFactor` option has no effect, so the CDP command is the only way there.

Over CDP the command is `Page.captureScreenshot`: `format` `png` or `jpeg` (`webp` is refused), `quality`, `clip` with `scale`, and `captureBeyondViewport` for the whole document. JPEGs use 4:2:0 chroma at every quality, as Chromium's do. `Emulation.setDefaultBackgroundColorOverride` sets the background behind transparent pages. Puppeteer's `page.screenshot()` uses it and works.

The reply carries, beside `data`, a `pyro` object: `{ width, height, bytes, sceneHash, ms }`, where `ms` has the time of each stage (`snapshot`, `fetch`, `layout`, `scene`, `raster`, `encode`, `write`, `total`) and `cpu`, the browser process's CPU time while it rendered. In releases up to 0.0.33 this object is named `lp`.

Frames of 4 million device pixels or more (3840 × 2160 is 8.3 million) are rasterized on several threads: one less than the cores, at most 8. `PYRO_RASTER_THREADS=n` in the server's environment sets the count; `0` keeps every frame on the connection's render thread, which uses less CPU time per frame and suits many pages rendering at once.

The picture is rendered from a snapshot of the live DOM, off the browser thread, and includes the hover and focus styles of where the pointer and focus are. Pixels differ from Chromium's (no font hinting, a different layout engine), so do not compare them with Chrome screenshots. For visual regression tests against Pyro's own baselines, see [Visual tests](/e2e/screenshots).

### Raw frames

Newer than 0.0.33. To make a video from a page you step frame by frame, or to hand pixels to an image library, skip the image encoder: Pyro returns the frame as raw pixels. From the native SDK:

```ts
const frame = await page.captureFrame({ format: 'rgba' });
// { width, height, bytes, data: Buffer, ms: { snapshot, layout, scene, raster, encode, write, total, cpu } }
```

| Option | Meaning |
| --- | --- |
| `format` | `'rgba'` (default; the render as it is), `'rgb24'` (alpha dropped) or `'yuv420p'` (I420, BT.601 limited range). The names are ffmpeg's `-pix_fmt` values. |
| `path` | An absolute path on the browser's machine. The browser appends the frame to that file, or to a FIFO, and `data` comes back empty. |
| `clip` | `{ x, y, width, height, scale? }` in document CSS pixels. Default: the viewport. |

`width` and `height` are in device pixels: the viewport times the device scale factor. A page's pending `route` or navigation runs before the capture.

`FrameFifo` streams frames into a reader such as ffmpeg without a file in between. This is the example from the package's README; it was not run for this page:

```ts
import { spawn } from 'node:child_process';
import { FrameFifo, evaluate } from '@povio/pyro';

const fifo = new FrameFifo('/tmp/frames.fifo'); // mkfifo; the path must be absolute and must not exist
const ffmpeg = spawn('ffmpeg', ['-f', 'rawvideo', '-pix_fmt', 'yuv420p', '-s', '3840x2160', '-r', '30', '-i', fifo.path, 'out.mp4']);
await fifo.open();                              // resolves once ffmpeg has opened the FIFO
for (let f = 0; f < 300; f++) {
  await page.run([evaluate(`frame(${f / 30})`)]); // your page's function that draws time t
  await page.captureFrame({ format: 'yuv420p', path: fifo.path });
}
await fifo.close();                             // ends the stream and removes the FIFO
```

- Start the reader before `fifo.open()`. A capture into a FIFO that has no reader fails at once, and a reader that takes nothing for 30 s fails the capture.
- The browser opens and closes the path for each frame. `FrameFifo` holds the FIFO open in between, so the reader sees one stream until `close()`.
- `-s` must be the frame's size in device pixels (`frame.width` × `frame.height`).
- `FrameFifo` works on macOS and Linux.
- Against `pyro serve --fork` each page is a process of its own, so several pages render frames in parallel.

Other clients use the same capture: the Playwright library with `page.screenshot({ raw: { format, path } })` (see [Raw frames](/playwright/snapshots#raw-frames)), and CDP with `Page.captureScreenshot { format: 'rgba' | 'rgb24' | 'yuv420p', path }`. To record what a page does as it runs, without stepping it, use the Playwright library's [`recordVideo`](/playwright/runner#video) or CDP `Page.startScreencast`.

## Next

- [A batch of URLs to PDF](/recipes/scraping-pdf-batch): many URLs to A4 PDFs a few at a time, error pages skipped, each file's text checked. Tested end to end.
- [Rendering: screenshots and PDF](/guide/rendering): how the renderer works.
- [CDP coverage](/reference/cdp): `Page.printToPDF` and `Page.captureScreenshot` in full detail.
- [serve options](/reference/serve): `--hyphen-digit-breaks`, `--render-prewarm`.
