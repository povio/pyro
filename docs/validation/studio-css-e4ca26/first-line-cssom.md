# First-line computed style

Parent `9f70b9a0`; WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable `158.0.8127.37`, LP 0.0.17 with JSC `0e0dd7038a7a-6f4649e395e0`. Matched Studio wptserve/testdriver runs at six jobs. CSSOM now recognizes both `:first-line` and `::first-line` and uses the first-line style already resolved by Blitz. Enable the same selector preference before stylesheet parsing as during layout. Native layout-text tests: 12/12. On 446 separately run control URLs, 305 → 307 strict pages and 4,869 → 4,980 passing names, no losses.

| Cohort | Parent strict / named | Candidate strict / named | Chrome strict / named | Observed names LP / Chrome |
|---|---:|---:|---:|---:|
| Main (8,082) | 5,472 / 177,172 | 5,483 / 177,300 | 6,950 / 182,963 | 194,540 / 194,912 |
| Supplemental (41) | 27 / 577 | 27 / 577 | 35 / 619 | 874 / 874 |

Main +11/-0 strict, +128/-0 names; 105 gains on first-line allowed properties, ten on container-query pseudo-element tests, four on first-line variables. Main LP-only / stable-only: 255 / 1,722 strict and 5,655 / 11,318 names. Main LP: 11 timeouts, one hang; Chrome: ten timeouts, one hang. Supplement unchanged, 4 UTF-16 and 37 worker URLs; 39 other URLs remain excluded. Sibling `linecss` artifacts retain per-page statuses and named differences. Timed outcomes are observations, not guarantees.
