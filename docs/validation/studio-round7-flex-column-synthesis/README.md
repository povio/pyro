# Synthesized baselines in mixed-writing-mode flex columns

Batch 12 (`8695db84513be44b2aedc700e3e042ee9354b2b4`) showed four column alignment pages each failing only on the parallel-writing-mode item. The orthogonal siblings already shared a measured first or last baseline, but the parallel item fell back to the column edge. Flex columns now use the parallel item's border box to synthesize its baseline when a compatible measured group exists. Synthesis uses the middle for a central baseline and the line-under edge for the horizontal alphabetic case. No rounding, row alignment, or table-track changes.

| Engine | Strict pages | Named passes | Runner wall | Browser CPU | HTTP port |
| --- | ---: | ---: | ---: | ---: | ---: |
| Topic parent | 30/63 | 724/850 | 0.510 s | 0.73 s | 58465 |
| Candidate | 34/63 | 728/850 | 0.686 s | 0.99 s | 61733 |
| Chrome | 61/63 | 846/850 | 5.612 s | 6.46 s | 59046 |

**+4/-0 whole pages; +4/-0 named tests.** Each of `align-items-baseline-column-horz.html`, `align-items-baseline-column-vert.html`, `alignment/flex-align-baseline-006.html` and `alignment/flex-align-baseline-007.html` gains exactly one Chrome-passing test. No other named result changed. This includes earlier opposite-flow and nested-flex gains, the unrepaired one-pixel offsets on `flex-align-baseline-001.html`, intrinsic controls, and both row and column calc-size flex-basis pages (33/33 each). These are matched focused results, not a full run or a performance comparison.

**Reproduction.** Studio, 2026-11-21. Topic parent `897d8cc044a122a65f2c076a8684764b246cae17` descends from frozen `305840f45afbc4f55bfa24f303c1e858d30e4c1d`; source `3b25717ccea86619c95cd689b8c09e755f01eb42`. Pinned WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; installed Chrome stable `158.0.8127.37`; Studio `pyro-env.sh`, patched JSC `0e0dd7038a7a-6f4649e395e0`. Isolated target, three Cargo jobs. Parent binary SHA-256 `a7be292b747c9c5744a7c6127083c6b465bdca4555f6639af21305bb46855be7`; candidate `8b249222394014b13619067479daa9ca7acb7c402a72300e20acd2d1c82ee93e`. Final incremental browser build: 31.77 s. `cargo test --locked --profile wptiter -p pyro-layout --test integration flow::flex`: built in 38.35 s, 30/30 passed in 0.05 s, including the new vertical-lr/vertical-rl mixed-synthesis control. Vendor patch verified forward and reverse against the matched topic parent.

The exact manifest `urls.json` was checked against the positive archive and withheld exclusions before running. Each engine ran separately with `bench/wpt/run.mjs --tests urls.json --dirs css/css-flexbox,css/css-values/calc-size,css/css-grid,css/css-align --jobs 2 --server wptserve --testdriver`. `outcomes.json.gz` records engine → URL → `[status,harnessStatus,[[name,status],…]]`. All manifests and name sets matched; all harnesses completed OK with zero restarts or watchdog kills. Strict pages require every name to pass.
