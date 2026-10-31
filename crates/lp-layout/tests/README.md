# lp-layout tests

`flow.rs`: lp-dom's flow-layout parity tests, ported to Blitz geometry when lp-dom lost its own layout.
The expected values were measured in headless Chromium (Playwright 1.59.1, 800x600). Tests that only
exercised flow internals (its style source, reuse caches, result diffing) were dropped.

Ignored tests (`Blitz parity gap`) fail against Blitz today, grouped:

- scrollWidth/Height: one border too many when nothing overflows, and a nested scroll container's
  overflow propagating to its ancestors (shell, popover, cross-min-max, compressible inputs).
- Stale geometry after mutations: Blitz's incremental restyle/relayout misses `:has()`, `aria-`/`data-`
  attribute selectors, focus/hover, radio groups and some removals (cached vs fresh layout differ).
- Text: `<br>` line boxes, trailing space in right-aligned lines, nbsp, `overflow-wrap`, generated
  content, text input/textarea scroll sizes from their value, whitespace in shrink-to-fit widths.
- Tables: fixed-layout column widths, rowspan, cell min/max widths.
- Floats, margin collapsing through parents, caret positions, scroll anchoring, SVG shape rects, image
  natural size (the tests' fetcher loads nothing).

`pending/layout.rs`: lp-dom's faux-layout tests (upstream's renderless geometry). Most expectations are that
layout's synthetic numbers; the scroll/visibility behaviour among them is still to be ported.
