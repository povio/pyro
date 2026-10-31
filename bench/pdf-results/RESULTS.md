# Page.printToPDF (krilla) vs Chrome: ToS pdfgen visual cases

Branch `proto/printtopdf-krilla` (local). M1 Max, 10 cores, 64 GB. The machine was shared: load average 8–22
during the final run, 11–36 during the first. Chrome 155 headless=new, system install. pdfgen was built from
tos-monorepo `HEAD` (a clean export, not the working tree); the cases come from `37bdbe095^`. Per-case table:
[summary.md](summary.md) (final run).

Engines: `chrome-base` (Chrome, base build), `chrome-fixed` (Chrome, `--paprize-page-box-shadow: none`),
`lp-fixed` (Lightningpanda, fixed build). Each runs the production flow: postMessage → `#document-generated` →
`fonts.ready` → `page.pdf` (A4, margin 0, printBackground). `run.mjs` drives both engines with Playwright
`connectOverCDP`. `puppeteer-flow.mjs` runs the ToS `PdfRenderer` steps unchanged with puppeteer-core 25.2.1
against LP. Each case runs three interleaved repetitions after one untimed warm-up, and the tables report
medians. Peak RSS covers the whole process tree, sampled every 50 ms.

## Measured: first run → after the three fixes

| | chrome-base | chrome-fixed | lp-fixed (first) | lp-fixed (fixed) |
|---|---|---|---|---|
| sum of median paginate ms (44 cases) | 2254 | 2281 | 3421 | 3306 |
| sum of median printToPDF ms | 8615 | 4054 | 882 | 918 |
| median printToPDF ms | 130 | 75 | 16 | 17 |
| total PDF MB | 11.8 | 7.2 | 2.7 | 2.7 |
| 6 concurrent, all 44 docs: docs/s | 7.9 | 11.6 | 27.6 | 25.3 |
| 6 concurrent: p50 / p95 ms | 308 / 807 | 212 / 348 | 88 / 358 | 85 / 389 |
| 6 concurrent: peak RSS MB | 4728 | 4381 | 5486 | **1131** |
| sequential peak RSS MB (132 renders, one process) | 2227 | 2166 | 3818 | **403** |

The Chrome columns come from the final run; the first run gave similar Chrome numbers (see git history). On this
Mac the shadow fix halves Chrome's printToPDF and changes zero pixels; it did not reach the 6x seen on Linux.

## Fidelity (lp-fixed vs chrome-fixed; pdftoppm at 100 dpi, pixelmatch threshold 0.1)

| | first | after fixes |
|---|---|---|
| cases with the same page count | 43/44 | 43/44 |
| pixel diff per case, median / mean | 4.5% / 4.9% | 4.7% / 5.0% |
| `pdftotext` word overlap with Chrome, median / min | 0.982 / 0.687 | 0.988 / 0.833 |
| cases with word overlap < 0.9 | 15 | 2 |
| pages whose content overflows the `.pz-page` box | 27 | 14 |

- Pixel diff is mostly anti-aliasing, hinting and 1–2 px offsets. The fixed footers now wrap into Chrome's
  columns, which moved text without lowering the pixel ratio.
- Remaining overflow: a header SVG logo, 92 px (23 pages). There is no `<svg>` overflow in Chrome, which shrinks
  the logo to its container. The invoice watermark overflows the same way in Chrome and is clipped there too.
  `bmd-report` overflows by 784 px because Blitz computes table column widths differently from Chrome.
- `house-bl-final-overflowing-table`: Chrome gives 4 pages, LP 3. Paprize measures with lp-dom's geometry,
  whose rows are 0.5–1 px shorter than Chrome's and which counts one wrapped two-line cell as a single line
  (22 vs 44 px). It therefore fits 26 rows on page 1 instead of Chrome's 24. Fixes 1–3 cannot change this,
  because Blitz is not what Paprize measures. Exact pagination needs `getBoundingClientRect` answered from
  Blitz's layout, or lp-dom text wrapping that matches.
- Lowest text overlaps after the fixes: `vermas-overflow` 0.83 (not investigated) and the CMR cases at 0.90.
  The CMR margin labels use `writing-mode: vertical-rl`, which Blitz does not support.

## The three fixes

1. **Flex fit-content** (`ad8e85b`, vendored taffy 0.15.1). A single-line, auto-width row flex container in
   definite available space took its max-content width, so a row inside an `align-items:center` column
   (ToS footers) overflowed by up to 1092 px instead of wrapping. It is now `min(max-content,
   max(min-content, available))`, applied in the inline axis only. All 6605 upstream tests pass, the patch is in
   `vendor/taffy/UPSTREAM.patch`, and there are two regression tests in lp-render. I tried a second Taffy
   change for the SVG logo (content size suggestion ignoring the preferred cross size). It broke 12 Chromium
   tests, so I reverted it.
2. **Memory on navigation** (`8e21af0`). This was not a leak: a navigation disposed the old document but left
   it to JSC's heuristic collector, which a busy server rarely ran. Navigation now releases it the way page
   close already did (synchronous collection, then free pages returned to the OS). Over 200 iterations of
   goto + generate + printToPDF, RSS went from 6783 MB to 330 MB, and each iteration got faster (172 → 157 ms).
   The collection costs about 5 ms per navigation. About 0.3 MB per navigation is still retained.
   `LIGHTNINGPANDA_RELEASE_ON_CLOSE=0` turns this off. `navmem.mjs` measures it.
3. **Puppeteer** (`db84ceb`). puppeteer ≥ 22 auto-attaches to tab targets and attaches each page from its tab's
   session; LP had only page targets, so `newPage()` hung. Each page now has a virtual tab target. Page setup
   also needed Audits/Performance enable, `Network.setUserAgentOverride`, and
   `Emulation.setTouchEmulationEnabled(false)`. Result: the unchanged ToS `PdfRenderer` flow produces 44/44 PDFs
   with puppeteer-core 25.2.1, with page counts identical to the Playwright run.

## krilla mapping

- Mapped: fills and strokes; solid colors and linear, radial and sweep gradients, including stop alpha; image
  brushes, including repeat tiling; glyph runs, with embedded subset fonts and text recovered from a reverse
  cmap; clip, opacity and blend layers; inset box shadows, drawn as a box-minus-hole even-odd fill.
- Not mapped, counted in the reply's `lp.stats`: outset box shadows (skipped), filters, compose modes other
  than source-over, variable-font coordinates, synthetic bold. The 44 cases use none of these.
- Zero-area fills are dropped. vendor/blitz-paint also skips zero-width border edges: Tailwind's
  `border: 0 solid` otherwise prints as hairlines.
