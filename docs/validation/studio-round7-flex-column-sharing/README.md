# Opposite-flow first and last baseline sharing in flex columns

Studio, 2026-11-21. Batch 10 exposed mixed baseline failures in ordinary flex columns. Source `a4eb5356a3f34db770ed5627db7b43a064c6153b`, matched parent `c98838f2d6024320afb22458108768d996a227f3`, verified descendant of frozen `305840f45afbc4f55bfa24f303c1e858d30e4c1d`.

The column now measures each participating orthogonal child's selected baseline from its group's edge, including the margin on that edge. This lets opposite writing directions with complementary baseline preferences share a group. The maximum distances on both sides determine the group's size; wrap-reverse anchors the group to the other edge without reversing its members. No change to row alignment or table layout.

| Engine | Strict pages | Named passes | Runner wall | Browser CPU | HTTP port |
| --- | ---: | ---: | ---: | ---: | ---: |
| Parent | 24/57 | 584/697 | 0.479 s | 0.64 s | 51535 |
| Candidate | 25/57 | 591/697 | 0.441 s | 0.60 s | 57528 |
| Chrome | 55/57 | 693/697 | 4.968 s | 5.81 s | 52084 |

**+1/-0 strict pages, +7/-0 named tests.** Chrome passes all seven gains. `css/css-flexbox/alignment/flex-align-baseline-004.html` (wrap-reverse) goes from 1/6 to 6/6. Variants 006 and 007 gain one each (both now 2/3). Variant 001 remains 2/6: the four remaining offsets are one pixel to the right of the expected result. No other named status changed, including the nested-flex, reversed-flow, vertical-writing and intrinsic controls; the row and column calc-size flex-basis pages each retain 33/33. These are focused results only, not a full-cohort estimate.

`urls.json` is the union of the prior flex export and consumer controls minus table URLs, checked against the frozen main positive archive and withheld exclusions before execution. `outcomes.json.gz` maps engine → URL → `[status,harnessStatus,[[name,status],…]]`. All three manifests and named sets match, all harnesses completed OK, no restarts or watchdog kills. Single runs are not performance measurements.

WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; installed Chrome `158.0.8127.37`; Studio `pyro-env.sh` with patched JSC `0e0dd7038a7a-6f4649e395e0`. Browser SHA-256: parent `5a282467f3e8139f8d14efbfe7960c4056f0f2a543535105fcc9e29738f16598`, candidate `a7be292b747c9c5744a7c6127083c6b465bdca4555f6639af21305bb46855be7`. Each engine ran separately with `bench/wpt/run.mjs --tests urls.json --dirs css/css-flexbox,css/css-values/calc-size,css/css-grid,css/css-align --jobs 2 --server wptserve --testdriver`. Isolated warm target; browser build 51.54 s with three Cargo jobs. Separate native `cargo test --locked --profile wptiter -p pyro-layout --test integration flow::flex`: 42.40 s build, 29/29 pass in 0.05 s including the new normal/reversed mixed-baseline control. Taffy vendor patch verified forward and reverse against the matched parent.
