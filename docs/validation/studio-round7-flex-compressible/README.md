# Mixed percentage lengths in a form control’s min-content contribution

Studio, 2026-11-21. Frozen `305840f45afbc4f55bfa24f303c1e858d30e4c1d` is an ancestor of the matched parent `fdf1f7bf80a55d358671a5a5db9b2038edcda95c`. Candidate source: `e49c48daadb1416ef670678e223eb61205e27691`. The CSS Sizing 3 cyclic-percentage rule resolves the percentage against zero, not the entire preferred inline size. For `calc(100% + 140px)`, the text control previously contributed its intrinsic width, while the slider discarded the 140px. Use the resolved length for this min-content measurement, accounting for box sizing; leave the actual preferred size and max-content measurement alone.

| Engine | Strict pages / 19 | Named passes / 202 | Runner wall | Browser CPU | HTTP port |
| --- | ---: | ---: | ---: | ---: | ---: |
| Checkpoint parent | 9 | 179 | 0.210 s | 0.36 s | 57941 |
| Candidate | 11 | 183 | 0.184 s | 0.32 s | 51802 |
| Chrome 158.0.8127.37 | 15 | 183 | 1.780 s | 2.57 s | 58242 |

Candidate vs parent: **strict pages +2/-0; named passes +4/-0**. `flex-item-compressible-001.html` (width, row) and `flex-item-compressible-002.html` (height, column, vertical writing) each went from 13/15 to 15/15. Only `.flexbox 11` (text) and `.flexbox 12` (range) on each changed; Chrome passed all four. All other results were unchanged. Equal candidate and Chrome pass counts do not imply parity: each still passes eight names the other fails. All three runs observed identical name sets, with 19 complete OK harnesses and zero restarts or watchdog kills. A strict page has a complete OK harness and all named assertions passing.

`urls.json` is the exact matched manifest, selected before patching from the previous checkpoint’s safe flex cohort. Membership in archived round-six main and disjointness from the withheld manifest were checked before execution. No omitted cases or full sweep. `outcomes.json.gz` stores per-engine URL → `[status,harnessStatus,[[name,status],…]]`. WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; `bench/wpt/run.mjs --dirs css/css-flexbox --tests urls.json --jobs 4 --server wptserve --testdriver`, fresh contexts and ephemeral ports throughout. Timings are single-run observations, not a performance claim.

The checkpoint binary was copied before rebuilding (SHA-256 `f86ed39554e06a09811c895195ff794b1fe586f1e9f4d499697f3d1aec2fa650`); final candidate `3eae3c4691d7db3d37d73b55046d0c849dccb3ee12ca418cd1f1d77503512440`. Studio `pyro-env.sh`, patched JSC `0e0dd7038a7a-6f4649e395e0`, isolated warm `wptiter` target, four build jobs and no overlapping WPT jobs. Final incremental browser build: 19.32 s. Native `flow::flex`: 26/26 passed in 0.05 s, including row/column and content-box/border-box controls for both input types. The Blitz DOM vendor patch passed forward and reverse verification against the checkpoint.
