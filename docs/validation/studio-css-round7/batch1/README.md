# Batch 1: frozen-to-candidate CSS testharness comparison

Frozen source `305840f45afbc4f55bfa24f303c1e858d30e4c1d`, candidate `c55860cfebf5175c659c9327b1d1d1b4db12d22b`. Rehashed the read-only candidate (137197216 bytes, SHA-256 `c44fae0806862ea87974b1050127b59522657833435a679f74c32fa7ca1cf934`). Both builds used patched JSC `0e0dd7038a7a-6f4649e395e0`. Comparison uses the existing final candidate runs, not a duplicate. Both LP runs used the frozen runner, two workers, identical exact allowlists, WPT, wptserve, testdriver and worker-global settings. Frozen used Node, batch main used Bun; runner wall time is not a benchmark. See `../frozen/provenance.md` for the pinned historical Chrome reference and limitations.

| Cohort | URLs | Strict pages: frozen / batch / stable | Named passes: frozen / batch / stable | Observed names: frozen / batch / stable |
|---|---:|---:|---:|---:|
| Main | 8082 | 5834 / 5837 / 6950 | 182498 / 182542 / 182963 | 194672 / 194672 / 194912 |
| Supplement | 41 | 27 / 27 / 35 | 577 / 577 / 619 | 874 / 874 / 874 |

The three newly strict pages are `animationevent-interface.html`, `transitionevent-interface.html` and `StyleSheet-properties.html`. These gained 6, 6 and 2 named passes respectively; the min-content and max-content flex container pages gained 15 each. Of 44 new passes, 40 also pass in pinned Chrome. No previously passing page or assertion was lost, and no observed assertion or page-level harness or runner status changed. No loss repeats were warranted. The 39 withheld URLs were not run. Main: 103 completed harness timeouts, 12 runner timeouts and 1 hang (same exact URLs as frozen); supplement: all 41 harnesses completed OK.

Batch-only / stable-only main: 289 / 1402 strict pages, 5926 / 6347 named passes and 260 / 500 observations. Supplement: 0 / 8 strict pages, 0 / 42 named passes, 0 / 0 observations. Directional files contain the exact URL or URL/name sets, including status changes. `compare.py` reads final raw files (never partial output), verifies exact plan and result membership, and compares with the adjacent frozen archive. Supply the raw run directory, output directory, then `main supplement`. The SHA-256 receipts cover the archived files; raw results are referenced by digest in the summaries.
