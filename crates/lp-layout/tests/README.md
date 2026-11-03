# lp-layout tests

`flow.rs`: lp-dom's flow-layout parity tests, ported to Blitz geometry when lp-dom lost its own layout.
The expected values were measured in headless Chromium (Playwright 1.59.1, 800x600). Tests that only
exercised flow internals (its style source, reuse caches, result diffing) were dropped.

Ignored tests (`Blitz parity gap`) fail against Blitz today, grouped:

- Text measured in system-ui (SF on macOS): form-control intrinsic widths (Chromium uses the font's
  average and max character widths), text-align.
- Flex: a percentage flex-basis in a column of indefinite height should behave as content.
- Tables: rowspan (row heights and row boxes around a spanning cell).

`pending/layout.rs`: lp-dom's faux-layout tests (upstream's renderless geometry). Most expectations are that
layout's synthetic numbers; the scroll/visibility behaviour among them is still to be ported.
