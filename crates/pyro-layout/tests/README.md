# pyro-layout tests

`flow.rs`: pyro-dom's flow-layout parity tests, ported to Blitz geometry when pyro-dom lost its own layout.
The expected values were measured in headless Chromium (Playwright 1.59.1, 800x600). Tests that only
exercised flow internals (its style source, reuse caches, result diffing) were dropped.

Every parity test runs; none is ignored. Where a test's expectation differed from Chromium (rounded
values, a behaviour assumed rather than measured), it was re-measured in Chrome and corrected.

`pending/layout.rs`: pyro-dom's faux-layout tests (upstream's renderless geometry). Most expectations are that
layout's synthetic numbers; the scroll/visibility behaviour among them is still to be ported.
