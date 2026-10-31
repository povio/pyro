# Page.printToPDF (krilla) vs Chrome: ToS pdfgen visual cases

Branch `proto/printtopdf-krilla` (local). M1 Max, 10 cores, 64 GB; shared machine, load average 11–36 during
the run (other sessions' builds and servers). Chrome 155 headless=new, system install. pdfgen built from tos-monorepo
`HEAD` (clean export, not the working tree); cases from `37bdbe095^`. Per-case table: [summary.md](summary.md).

Engines: `chrome-base` (Chrome, base build), `chrome-fixed` (Chrome, `--paprize-page-box-shadow: none`),
`lp-fixed` (Lightningpanda, fixed build). Production flow (postMessage → `#document-generated` → `fonts.ready` →
`page.pdf` A4, margin 0, printBackground), driven by Playwright `connectOverCDP` for both engines. Three interleaved
repetitions per case after one untimed warm-up; medians. Peak RSS is the whole process tree, sampled every 50 ms.

## Measured

| | chrome-base | chrome-fixed | lp-fixed |
|---|---|---|---|
| sum of median paginate ms (44 cases) | 2127 | 2104 | 3421 |
| sum of median printToPDF ms | 8103 | 3768 | 882 |
| median printToPDF ms | 122 | 70 | 16 |
| total PDF MB | 11.8 | 7.2 | 2.7 |
| 6 concurrent, all 44 docs: docs/s | 9.5 | 12.5 | 27.6 |
| 6 concurrent: p50 / p95 ms | 261 / 863 | 199 / 283 | 88 / 358 |
| 6 concurrent: peak RSS MB | 4983 | 4372 | 5486 |
| sequential peak RSS MB | 2225 | 2092 | 3818 |

- The shadow fix halves Chrome's printToPDF on this Mac (not 6x as on Linux) and changes no pixel (control diff 0.0%).
- LP pagination is slower (Paprize runs in LP's JS engine on lp-dom); LP printToPDF is ~4x faster than chrome-fixed.
- LP RSS grows ~35 MB per navigation and is not returned (measured with and without printToPDF: same growth), so a
  long-lived LP process ends larger than Chrome. This is the navigation path, not PDF output; not investigated.
- 6-concurrent LP = one server, six CDP connections (a render thread each). Chrome = one browser, six pages.

## Fidelity (lp-fixed vs chrome-fixed, pdftoppm 100 dpi, pixelmatch 0.1)

- Page counts match in 43/44 cases. `house-bl-final-overflowing-table`: Chrome 4 pages, LP 3. Paprize measured
  with lp-dom's geometry and packed more rows per page, and Blitz then drew them taller, so rows overlap the
  footer block (see `house-bl-final-overflowing-table-*.png`). My overflow metric does not catch this, because
  the overlap stays inside the page box.
- Pixel diff: median 4.5% per case, mean 4.9%, worst existing page 7.8%. Most of the diff is anti-aliasing, font
  hinting and 1–2 px position offsets, not missing content.
- Text: `pdftotext` works on every LP PDF (reverse-cmap text). Word overlap with Chrome is ≥0.98 except where
  content is clipped. The 15 cases below 0.9 are invoices and footers with a flex row that Blitz lays out as one
  unwrapped line (`display:flex` row inside an `align-items:center` column: Chrome clamps it to the container,
  Blitz uses max-content). That row overflows the page and its text is cut off.
- Overflow past the `.pz-page` box in Blitz: 27/44 cases, almost all horizontal (footer row above; 92 px in the
  booking/vermas header). Vertical: `vermas-overflow` (content box 797 px tall, content 1190 px).
- Other visible gaps: `writing-mode: vertical-rl` (CMR margin labels) is not supported by Blitz, so the text is
  drawn horizontally in the wrong place.

## krilla mapping

Mapped: fills and strokes (solid, linear/radial/sweep gradients incl. stop alpha), image brushes (incl. repeat tiling),
glyph runs (embedded subset fonts, per-glyph transforms for synthetic italic), clip layers, opacity/blend layers,
inset box shadows (box-minus-hole even-odd fill). Not mapped (counted in the reply's `lp.stats`): outset box
shadows (skipped), layer filters/backdrop filters, compose modes other than source-over, variable-font coordinates
(default instance), synthetic bold. Zero-area fills are dropped. A PDF rasterizer paints any pixel a fill
touches, so Tailwind's `border: 0 solid` printed as hairlines until vendor/blitz-paint skipped zero-width border
edges. No fonts failed, no brushes skipped, no variable fonts in these 44 cases.
