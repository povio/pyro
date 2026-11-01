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

## Latest: closing on Chrome's pagination (2026-11-01, afternoon)

Forced layouts in the long overflow documents got cheaper: line breaking is not repeated for the same
inputs, deltas no longer fall back to a whole-tree sync when pagination adds and fills a container or edits,
removes or re-attributes a `<style>`, and inserting or removing a child restyles only what structural
selectors require (Blitz restyled the parent's whole subtree). Measured in one run (2 interleaved repetitions
per case, 6-concurrent throughput after); load average 23 → 16 during the run. Every one of the 44 PDFs is
byte-identical to the morning run's. Per-case table: [summary.md](summary.md).

| | chrome-fixed | lp (flow geometry, earlier) | lp (Blitz, morning) | lp (Blitz, now) |
|---|---|---|---|---|
| page count equal to Chrome | – | 43/44 | 44/44 | **44/44** |
| pixel diff per case, median | – | 4.7% | 4.3% | **4.3%** |
| sum of median pagination ms (44 cases) | 1974 | 3306–3515 | 3291 | **2766** |
| sum of median printToPDF ms | 3974 | 918–938 | 858 | **415** |
| 6 concurrent: docs/s | 9.3 | 24.7–25.3 | 23.7 | **24.4** |
| 6 concurrent: p50 / p95 ms | 241 / 338 | 85–92 / 389–417 | 89 / 380 | **69 / 283** |
| 6 concurrent: peak RSS MB | 1100 | 1131–1164 | 1171 | **1186** |

- Of the 790 ms gap to Chrome, 424 ms is four documents: cmr-overflow (423 vs 279 ms), the booking-overflow
  pair (~190 vs ~95 ms) and house-bl's overflowing table (159 vs 69 ms). The other 40 cases account for the
  remaining ~370 ms, about 9 ms each (not profiled yet).
- Chrome's numbers moved between runs (the machine was shared all day); compare within a run.

## Earlier: Blitz relayout as fast as flow (2026-11-01)

Shaped runs and whole built text layouts are now reused for identical text and styles (pagination clones
content into pages), and two whole-tree scans per sync are gone. Measured in one run (2 interleaved
repetitions per case, 6-concurrent throughput after; load average 25 → 14 during the run), and again
without Chrome right after (3289 ms pagination, 24.3 docs/s). Per-case table: [summary.md](summary.md).

| | chrome-fixed | lp (flow geometry, earlier) | lp (Blitz, 2026-10-31) | lp (Blitz, now) |
|---|---|---|---|---|
| page count equal to Chrome | – | 43/44 | 44/44 | **44/44** |
| pixel diff per case, median | – | 4.7% | 4.3% | **4.3%** |
| sum of median pagination ms (44 cases) | 2074 | 3306–3515 | 4092 | **3291** |
| sum of median printToPDF ms | 3788 | 918–938 | 947 | **858** |
| 6 concurrent: docs/s | 13.0 | 24.7–25.3 | 19.1 | **23.7** |
| 6 concurrent: p50 / p95 ms | 184 / 270 | 85–92 / 389–417 | 89 / 457 | **89 / 380** |
| 6 concurrent: peak RSS MB | 4618 | 1131–1164 | 1155 | **1171** |

- cmr-overflow (2544 forced layouts per render): 925 → 485 ms per render. Blitz's inline construction went from
  466 to 107 ms: 79% of the text it shapes there is identical to text shaped before. The PDF is byte-identical
  after every step.
- What is left of the gap to Chrome is the long overflow documents (cmr-overflow, the booking-overflow pair,
  house-bl's overflowing table): line breaking of freshly cloned content and the pagination script itself.

## Earlier: Blitz is the only geometry, with HTTPS (2026-10-31)

lp-dom's flow layout and the approximate layout are gone: every layout query is answered from the page's
Blitz document, the one printToPDF prints. lp-net fetches HTTPS (rustls), so ToS pdfgen's Inter loads from
Google Fonts. Measured in one run (2 interleaved repetitions per case, 6-concurrent throughput after); the
machine was shared (load average 15–45 during the run). Per-case table: [summary.md](summary.md).

| | chrome-fixed | lp (flow geometry, earlier) | lp (Blitz only, now) |
|---|---|---|---|
| page count equal to Chrome | – | 43/44 | **44/44** |
| pixel diff per case, median | – | 4.7% | **4.3%** |
| sum of median pagination ms (44 cases) | 2040 | 3306–3515 | 4092 |
| sum of median printToPDF ms | 3834 | 918–938 | 947 |
| 6 concurrent: docs/s | 13.8 | 24.7–25.3 | 19.1 |
| 6 concurrent: p50 / p95 ms | 186 / 263 | 85–92 / 389–417 | 89 / 457 |
| 6 concurrent: peak RSS MB | 4383 | 1131–1164 | 1155 |

- Both booking-overflow cases give 10 pages as in Chrome (9 without Inter), with Inter-Regular and
  Inter-Bold embedded; glyphs and line breaks match Chrome
  ([side by side](booking-request-overflow-https-p1-6.7pct.png)). Their header logo (an SVG with
  `h-[37mm] w-auto` in a centered flex row) was 130 px off to the left; fixed (a replaced item's automatic
  minimum width is its natural width).
- Pagination is still LP's slow part: the forced layouts themselves. In cmr-overflow (2544 forced layouts per
  render) one sync costs 0.29 ms: 0.006 ms building lp-dom's change delta, 0.026 ms applying it to the
  mirror, 0.27 ms in Blitz's incremental restyle and relayout. Chrome does the same pagination in 266 ms
  (about 0.1 ms per layout); LP takes 880 ms.
- The 6-concurrent throughput dropped from ~25 to 19 docs/s under a heavier machine load than the earlier
  runs; not separated from the pagination cost above.

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

## Blitz-backed layout queries (`LIGHTNINGPANDA_GEOMETRY=blitz`, `a04026e`)

Paprize paginates with `getBoundingClientRect()` heights (plus the computed margins). By default LP answers those
from lp-dom's flow layout, an approximation. With `LIGHTNINGPANDA_GEOMETRY=blitz`, LP answers them from the same
Blitz layout that printToPDF paints. Per-case table: [summary.md](summary.md). Final run, load average 12–22.

**Design.** Layout queries are synchronous on the JS thread, and the render worker's Blitz documents live on
another thread. In this mode the browser thread therefore keeps its own `lp_render::Renderer`, with one persistent,
mirrored Blitz document per page document. lp-dom recomputes its layout only when the DOM, styles or viewport
changed (the existing stamp cache skips everything otherwise). When it does, a `GeometrySource` hook builds the
keyed node list, the mirror applies the difference, Blitz restyles and relays out incrementally, and the element
boxes replace lp-dom's. Every geometry getter (bounding/client rects, `offset*`, `client*`, `scroll*`) follows from
that one replacement. Text and range geometry stays lp-dom's. Fonts are the ones Blitz paints with: system fonts
plus `data:` web fonts. Other sub-resources are not loaded in this mode.

Chromium reports boxes differently from what Blitz paints in four places, so the queries are adjusted:

- Boxes are unrounded (fractional), as Chromium reports them; painting still uses the rounded layout.
- Table rows and row groups get the union of their cells' boxes. Blitz has no box for a row, so rows measured
  0 px tall, and Paprize packed too many rows per page.
- Inline elements get the union of their text fragments, since Blitz gives them no box either.
- In collapsed-border tables, cells take half of the collapsed cell and row borders. Blitz lays collapsed
  borders out as gutters and ignores row borders, so `tr.border-t border-b` rows measured 1 px short each.

| | Chrome (fixed) | LP, lp-dom geometry | LP, Blitz geometry |
|---|---|---|---|
| page count equal to Chrome | – | 43/44 | 42/44 |
| cases with `.pz-page` content overflowing vertically | – | 1 (vermas-overflow, 324 px) | **0** |
| pixel diff per case, median / mean | – | 4.7% / 5.0% | 4.7% / 4.9% |
| text overlap with Chrome, min / cases < 0.9 | – | 0.833 / 2 | 0.899 / 1 |
| sum of median pagination ms (44 cases) | 2295 | 3515 | 9491 |
| sum of median printToPDF ms | 4104 | 938 | 936 |
| 6 concurrent: docs/s | 12.5 | 24.7 | 8.2 |
| 6 concurrent: p50 / p95 ms | 213 / 335 | 92 / 417 | 118 / 1469 |
| 6 concurrent: peak RSS MB | 4285 | 1164 | 1332 |

- house-bl-final-overflowing-table now has 4 pages, as in Chrome (3 with lp-dom geometry). vermas-overflow no
  longer clips 13 rows: 36 rows on page 1, as in Chrome. Its text match went from 0.83 to 0.99.
- booking-request-overflow and booking-confirmation-overflow give 9 pages; Chrome gives 10. All heights match now
  except one cell, where Chrome wraps "1.130,069 ft³" onto two lines in a 62 px column and Blitz keeps one line
  (Blitz's text widths come out whole-pixel: 62.00 vs Chrome's 61.56). Each page ends up a row longer. Not fixed.
- Cost: 3205 forced layouts over the 44 cases, 1.7 ms each. That splits into 1.1 ms of mirror diff plus Blitz
  restyle/relayout, 0.35 ms rebuilding the node list and 0.25 ms reading boxes, plus lp-dom's own pass. cmr-overflow
  alone forces 2544 layouts (3.5 s; Chrome takes 0.3 s for the same work). house-bl forces 147 (570 ms; Chrome 77 ms).
  Already done: no work when the DOM/style/viewport stamp is unchanged, incremental Blitz relayout through the
  mirror, an O(n) box read, text layouts walked only for inline elements that need a rect, and FxHash maps.
  The next step is feeding lp-dom's change journal to the mirror, instead of rebuilding and diffing the whole node
  list per layout, and skipping lp-dom's flow pass in this mode.
- Default behaviour is unchanged: without the variable, all getters use lp-dom's flow layout.

## The two cases below 90% text match after the Taffy fix

- **vermas-overflow (0.83):** a pagination mismatch, the same cause as house-bl. Paprize, measuring with lp-dom's
  geometry, put 36 rows on page 1. In Blitz they ran 324 px past the page box and 13 rows were clipped. Blitz
  geometry fixes it (0.99). The word "TRANSOCEAN" is still missing: it is SVG `<text>` in the logo, which usvg draws
  as paths.
- **cmr-populated (0.899), with cmr-reference and cmr-long-address at 0.90:** the margin labels use
  `writing-mode: vertical-rl`. Blitz does not support it (its blitz-dom `writing-mode` feature is off), so the labels
  are laid out horizontally across the form.
