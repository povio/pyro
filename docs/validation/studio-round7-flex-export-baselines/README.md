# First and last baselines exported by nested flex containers

Studio, 2026-11-21. Batch 9 (`5503e80ece0f148851478a65fdd36de7d10be500`) prioritized nested flex baseline failures. Matched topic parent `d4e0f49c20b3705eda1023ec562e92674b4ff6e5` descends from frozen `305840f45afbc4f55bfa24f303c1e858d30e4c1d`. Source: `51a14fdc9e350983c208b5a1d1e426fa80a82c01`.

Flex layout previously exported only its first baseline, so a parent aligning to the last baseline could use the bottom border edge instead of the last child's text. Final layout now preserves the edge items' baseline offsets and selects the matching baseline group or visual edge item, accounting for reversed flow and wrapping. Vertical rows use the dominant last baseline for alignment just as they use the dominant first baseline. No changes to grid layout, CSSOM, animations or CSS values.

| Cohort | Engine | Strict pages | Named passes | Runner wall | Browser CPU | HTTP port |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Flex + controls | Parent | 21/59 | 409/584 | 0.525 s | 0.93 s | 52824 |
| Flex + controls | Candidate | 25/59 | 470/584 | 0.396 s | 0.71 s | 57479 |
| Flex + controls | Chrome | 57/59 | 580/584 | 5.213 s | 6.78 s | 54536 |
| Grid/alignment consumers | Parent | 1/5 | 89/135 | 0.186 s | 0.19 s | 58065 |
| Grid/alignment consumers | Candidate | 3/5 | 129/135 | 0.093 s | 0.13 s | 58151 |
| Grid/alignment consumers | Chrome | 5/5 | 135/135 | 0.452 s | 0.87 s | 58234 |

Disjoint cohorts total **+6/-0 strict pages, +101/-0 named assertions**; Chrome passes every gain. Four 48-assertion pages now pass completely: `flex-align-baseline-flex-{001,002}` and `grid-align-baseline-flex-{001,002}`. The two other conversions are `flex-align-baseline-fieldset-{002,003}`. The `003` and `004` nested flex/grid variants still pass 12/16 and 14/16 respectively. The first cohort's 61 named gains break down as 15/33/2/7 on nested flex 001–004, one on each fieldset 002–003 and one on each overflow 002–003. The second cohort gains 15/20/2/3 on nested grid consumers 001–004. All other names are unchanged, including row/column flex-basis calc-size (33/33 each) and the intrinsic width/height controls.

Each `urls.json` is the exact matched manifest, checked against the archived positive main list and withheld exclusions before that cohort ran. Each `outcomes.json.gz` records engine → URL → `[status,harnessStatus,[[name,status],…]]`. Identical name sets in each cohort; every harness complete and OK; no restarts or watchdog kills. Strict means all names pass with a complete OK harness. No full sweep or withheld cases. Single-run timings are not a speed comparison.

WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable `158.0.8127.37` (installed version checked), Studio `pyro-env.sh`, patched JSC `0e0dd7038a7a-6f4649e395e0`. Isolated warm target, three Cargo jobs, separate WPT phases. Parent binary SHA-256 `27fb06f9864c2dbbf559d27b899a0ebb06801458c04a3f038d96cb1f8dfe7014`; candidate `5a282467f3e8139f8d14efbfe7960c4056f0f2a543535105fcc9e29738f16598`. Final incremental browser build 23.46 s. `bench/wpt/run.mjs --tests urls.json --jobs 3 --server wptserve --testdriver` with `--dirs css/css-flexbox,css/css-values/calc-size` or `css/css-grid,css/css-align`, fresh contexts and ephemeral ports. Native `cargo test --locked --profile wptiter -p pyro-layout --test integration flow::flex` compiled in 23.01 s and passed 28/28 in 0.06 s, including first/last alignment in row, reversed row, column and reversed column. Vendor patch verified forward and reverse against the topic parent.
