# Rendering: screenshots and PDF

Pyro has no GPU and no compositor. When you ask for pixels or a PDF, it renders the page's live DOM on the CPU with [Blitz](https://github.com/DioxusLabs/blitz): Stylo for styles, Taffy for flex, grid and block layout, Parley for text, and Vello's CPU rasterizer for pixels. The result is deterministic, and it is not Chromium's.

Every bundled browser in the npm package is built with rendering enabled.

## The same layout everywhere

Blitz is not only the screenshot engine. Layout queries in the page (`getBoundingClientRect`, `offsetWidth`, scrolling, `elementFromPoint`) and the hit tests behind clicks are answered from a Blitz layout of the same document. A screenshot therefore shows the boxes your test clicked on.

## Screenshots

There are three ways to take one:

| API | Use |
| --- | --- |
| `matchScreenshot('name')` step, or `page.matchScreenshot()` (native SDK) | visual regression tests, compared in the browser |
| `expect(page).toHaveScreenshot()`, `page.screenshot()` (Playwright module) | Playwright-style visual tests |
| CDP `Page.captureScreenshot` | any client; `png` (default) or `jpeg` with `quality`, `clip`, `captureBeyondViewport` for the full page |
| `page.captureFrame()` (native SDK), `page.screenshot({ raw })` (Playwright module), newer than 0.0.33 | raw pixels (`rgba`, `rgb24`, `yuv420p`) with nothing encoded, for ffmpeg or an image library. See [Raw frames](/scraping/pdf#raw-frames) |

Images are in device pixels: at a device scale factor of 2, a 1920 × 1080 viewport gives a 3840 × 2160 image. The Playwright module's `recordVideo` and CDP `Page.startScreencast` capture a page as it runs, through the same renderer (newer than 0.0.33; see [Video](/playwright/runner#video)).

How a screenshot is made:

1. **Snapshot.** The browser thread copies the live DOM: form state (input values, checked boxes, selected options) goes into the copy, as do style sheets including CSSOM edits, the viewport, device scale factor, color scheme and scroll position. Scripts are dropped and shadow trees flattened.
2. **Layout.** A render thread (one per connection) lays the copy out with Blitz. Images, `@font-face` fonts and `@import`ed sheets are fetched through the page's own network stack, so cookies, mocks and interception apply. The first layout waits for the fonts and images the document asked for, so web fonts are in the first picture.
3. **Display list and hash.** The painted scene gets a content hash.
4. **Raster** on the CPU with `vello_cpu`. Frames of 4 million device pixels or more are rasterized on several threads (`PYRO_RASTER_THREADS` sets how many); the pixels are the same either way.

Repeat screenshots of the same page update the render thread's document with only what changed, so Stylo and Taffy restyle and re-lay out only that.

### Comparing in the browser

The native `matchScreenshot` step compares in the browser. Baselines live in `__screenshots__/<name>.png` next to the test file. The client sends each baseline once per connection. If a later render produces the same scene hash as the one recorded when it last matched, the step passes without rasterizing at all. A mismatch throws `ScreenshotMismatchError` and writes `<name>.actual.png` and `<name>.diff.png`. See [Visual tests](/e2e/screenshots) and [Screenshots and snapshots](/playwright/snapshots).

### What is not captured

- video and WebGL pixels (2D canvases are captured);
- iframe contents (frames render as empty boxes);
- `:active` state, the caret and the text selection (`:hover` and `:focus` do follow the page);
- running CSS animations and transitions, which render at their current computed values.

Known gaps in Blitz's painting: no synthetic bold, synthetic italic slants the wrong way, inline boxes do not paint `padding` or `border-radius`, table column widths differ from Chromium's, and text has no hinting.

Newer than 0.0.33, these are painted: the `placeholder` of an empty `<input>` or `<textarea>`, in its `::placeholder` style; the "…" of `text-overflow: ellipsis`; an inline `<svg>` without a `viewBox`, at the size of its box. At a device scale factor above 1, line heights are rounded to whole CSS pixels and macOS's system font is tracked for its CSS size, both as in Chromium.

## Screenshots versus Chromium

Pixels come from Blitz, not Chromium. A Chromium screenshot is never a usable baseline: on the project's prototype fixtures, 8 to 44% of pixels differed, and 4 to 5% differed by more than 32 per color channel. Record baselines with Pyro and keep them separate from any Chromium baselines.

## Determinism

The CPU rasterizer is bit-exact from run to run, so compare exactly by default and add `threshold` or `maxDiffPixels` only for known noise. What can still change pixels between machines is their fonts: text uses the fonts the page loads, then the system's fonts. Generate baselines on the same operating system and font set your CI uses. The Playwright module's `toHaveScreenshot` puts the platform in the baseline file name (`<name>-<platform>.png`) for this reason.

## PDF

`Page.printToPDF` prints the page's Blitz document. From the native SDK, send it with `page.send`:

```ts
import { writeFileSync } from 'node:fs';
import { Pyro, loaded } from '@povio/pyro';

const pyro = await Pyro.launch();
const page = await pyro.page({ url: 'https://example.com/' });
await page.run([loaded()]);
const { data } = await page.send('Page.printToPDF', {
  paperWidth: 8.27, paperHeight: 11.69,   // A4, in inches
  printBackground: true,
});
writeFileSync('page.pdf', Buffer.from(data, 'base64'));
page.close();
pyro.close();
```

`page.pdf()` in the Playwright module and Puppeteer's `page.pdf()` send the same command. The PDF waits for the fonts and images its layout fetches, up to 10 seconds, while the browser keeps serving other commands.

Two kinds of document:

- **Paged documents.** If the document has `.pz-page` boxes (as Paprize templates produce), each box becomes one PDF page.
- **Everything else** is laid out at the paper's content width and cut into pages the way Chrome cuts it: paper size, `landscape`, margins (0.4 in by default), `scale` (0.1 to 2), `pageRanges`, `printBackground`. Cuts move to the top of a line box, an unbreakable box or a `break-inside: avoid` box they would split; `break-before`/`break-after` are honoured. There is no CSS fragmentation: boxes are not split and re-laid out, and `@page` rules and repeated table headers are not supported.

Links become clickable link annotations and a tag tree is written by default (`generateTaggedPDF`). `displayHeaderFooter`, header and footer templates, `preferCSSPageSize` and `generateDocumentOutline` are accepted but produce nothing.

::: warning Print media
The page prints with its current emulated media, which is `screen` unless you set `print` (`Emulation.setEmulatedMedia`, Playwright's `page.emulateMedia({ media: 'print' })`, or the native `media({ media: 'print' })` step). Chrome prints with print media. Set it explicitly if your print styles live in `@media print`.
:::

[Coverage of every `printToPDF` option](/reference/cdp) is in the CDP reference; [Screenshots and PDFs](/scraping/pdf) has recipes.

### Text in PDFs

Text in the PDF copies and extracts as its source characters. Each glyph maps to the characters it was shaped from, so ligatures and glyphs substituted by OpenType features (tabular figures, small caps, case-sensitive punctuation) extract as their text. Glyphs clipped by an overflow clip or past the page edges are not written, as in Chrome. List bullets are drawn as outlines, not text.

Variable fonts are embedded once per instance a run used, so a `font-weight: 700` heading in a variable font prints bold.

## Fonts

- Fonts come from `@font-face` rules the page loads, then from the system. `@font-face` `unicode-range` decides which face draws each character.
- **Google Fonts serves different files by User-Agent.** Pyro's default page User-Agent is `Mozilla/5.0 (compatible; pyro/<version>)`. For it, Google Fonts returns one TrueType file per weight with no `unicode-range`. For a Chrome User-Agent it returns variable WOFF2 files split into script subsets. To render with the same font files Chrome gets, set a Chrome User-Agent before navigating (Playwright's `userAgent` context option, or CDP `Emulation.setUserAgentOverride`).
- On Linux, system fonts are found through fontconfig. A minimal container may have no fonts at all; see [Installation](/guide/installation#ci).

## Line breaks between a hyphen and a digit

Unicode's line breaking rules keep a hyphen together with a following digit, so by default Pyro never wraps `REG-be08b23e-89be-48b8-9a6f-…` after a hyphen followed by a digit. Chrome does. To wrap like Chrome in screenshots and PDFs:

```sh
pyro serve --hyphen-digit-breaks on
# or PYRO_HYPHEN_DIGIT_BREAKS=on
# or Pyro.launch({ args: ['--hyphen-digit-breaks', 'on'] })
```

Text may then break between a hyphen and a digit when the hyphen follows a letter or digit (`10-` / `20`). A minus sign stays with its number (`-120,00`, `A -5`). Table column widths and other min-content widths follow the same breaks. It is off by default.

## Cost

A screenshot or PDF pays for a layout the first time, then only for changes. The first render in a server also scans the system fonts; `--render-prewarm on` does that at startup instead (on by default with `--prepare`). Taking no screenshots costs nothing at render time.
