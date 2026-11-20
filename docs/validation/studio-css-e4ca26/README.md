# Studio CSS testharness checkpoint

WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, base `e4ca26f4883eb47e507fa8bb2aa87aa6ae9a7d27`, candidate `40cb43b03be76df6effe33d732d1b3fe89f716a6`. macOS 27.2.1 (26B131), LightningPanda 0.0.17, JSC build key `0e0dd7038a7a-6f4649e395e0`. Primary reference: local headless Google Chrome **stable 158.0.8127.37** over CDP with a disposable profile. All three engines use the same WPT and runner, `--kind testharness --dirs css --server wptserve --testdriver --tests urls.json`; `urls.json` is the `matched` array in `coverage.json.gz`. LP used one run per binary, six jobs. Chrome used 16 disjoint shards; shards 9, 11, 13 and 15 additionally disabled extensions and background component extensions after a CDP attachment failure. No experimental web platform flags. Testdriver coverage is limited to the runner's implementation.

| Engine | Strict pages / 8,082 | Named passes | Distinct names observed | Harness OK / error / timeout (completed) | Runner timeout / hang / error |
|---|---:|---:|---:|---:|---:|
| LP base | 5,074 | 160,746 | 193,900 | 7,856 / 102 / 105 | 14 / 1 / 4 |
| LP candidate | 5,097 | 163,109 | 193,929 | 7,863 / 102 / 105 | 11 / 1 / 0 |
| Chrome stable | 6,950 | 182,963 | 194,912 | 7,986 / 23 / 62 | 10 / 1 / 0 |

Strict page pass means runner complete, raw harness status OK, at least one real named assertion and all assertions passing. Named sets use `(URL including query, name)`; absence is not a failing assertion. Outcomes contain runner status, raw harness status (null if unavailable), then all observed named statuses; the synthetic no-suite marker is excluded. `difference-single-vs-*` gives exact page, named-pass and observed-name differences: left is candidate. Against base: +23/-0 pages and +2,363/-0 named. Against Chrome: 232 candidate-only / 2,085 Chrome-only pages, 5,283 / 25,137 named passes, 246 / 1,229 names observed only on one side. Full-run gains can include timing variance.

The focused before/after/Chrome files isolate 31 URLs. Keyframes/name parsing and integer fragmentation animation gain 274 named passes with no losses. Gap decoration inset longhands/shorthands gain 2,075 with no losses (186 → 2,261; 5 → 24 pages, matches stable). This covers style parsing, computation and interpolation, not painting. Four orphans extrapolation cases still differ from Chrome.

`inventory` records 8,062 source files and 8,162 distinct URLs from native WPT discovery. `coverage` records all matched and uncovered URLs: 39 crash/security exclusions, 41 not discovered by the runner (15 dedicated worker, 11 shared worker, 11 service worker, four charset HTML). All 75 matched query variants and 20 HTTPS-named URLs are included; no matched H2 URL. No exact-revision stable run was found on wpt.fyi. `canary-supplement` is a **separate** Linux 22.04 experimental comparison, Chrome Canary 159.0.8143.0, wpt.fyi run 4907012216061952; do not pool it with stable.

Largest individual stable-only named sets: calc-size height interpolation (968), system fonts (588), calc-size width interpolation (580), all-property cascade `revert-layer` (371). These are remaining gaps, not claims about their root cause.
