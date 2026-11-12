# Automatic-height repair: bounded geometry and TOS evidence

Exact main eabee486709f79167520a46887cd6f303b26711d versus candidate9d33aa2819c6624906c4be71944364e09169d96b. The only production delta is automatic-height enclosure in pyro-dom; candidate source and unit tests are separate from application fixtures. Signed binary hashes and static script provenance are retained in provenance.json; main66e7448/candidate72812cf3 both have allow-jit and polling traps. Candidate release uses DEBUG0/STRIPnone to preserve signing. No speed comparison is claimed from test durations.

The unchanged W3 static probe reproduces both inline/class pointer-events:none misses on main (BODY, click handler not reached). Candidate preserves positions and explicit cell height while extending the automatic wrapper5→20px; both centers hit inside the wrapper and its native click reaches the listener. The non-pointer-none control passes both. This matches the native red-before regressions; no ancestor-hit fallback or selector-specific rectangle was introduced.

Three previously failing f9f TOS cases were pre-recorded before execution, then run unchanged on both binaries with the same batch16 fixture, original routed manifest, backend-token localStorage auth and documented direct-lane version-gate bypass. Per-run DBs are isolated clones of the same-day registered template. Cleanup verified0 run databases remaining and three ledger templates retained for both runs.

|Case|Exact main|Candidate|
|---|---|---|
|Route incoterm|Fails: no option found|Pass|
|Route transport mode|Timeout during scroll/click|Pass|
|HBL Document Party reference|Timeout waiting for update response|Same timeout remains|

The route results establish a concrete full-page repair for these two unchanged assertions. They do not establish complete layout/clipping/stacking correctness, all route cases, native focus behavior, or HBL placement repair. HBL remains a separately tracked problem; prior investigation showed intentional @povio/ui top-placement reversal, not JSC sorting. Full sanitized test status/provenance/selection hashes are in tos-targeted.json; raw reports stay locally retained with their hashes.

DOM/WebAPI debug286 tests, release286 tests and strict clippy pass. The first release suite exposed an unrelated null-body fetch test race: body consumption can finish at headers before a separate transport Finished notification. The test now drains transport owner tasks before asserting the terminal frontier; production fetch is unchanged. The original failure log is retained locally, and the corrected complete release suite passes. Final evidence/test-only commits preserve the exact production tree of tested9d33aa2. Existing source documentation records the measured automatic-height cache rebuild tradeoff; no general CPU/memory improvement claim.
