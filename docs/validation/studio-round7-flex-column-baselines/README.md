# First baselines of orthogonal items in column flex lines

Studio, 2026-11-21. Parent `e38f90aeddc2456e4f8bd1d9268a43f9a2a7142a` descends from frozen `305840f45afbc4f55bfa24f303c1e858d30e4c1d`. Source `72268539c5a88317ad1c8ec0bb6da9bb5be15a0f` measures orthogonal items' first baselines from their own block-start margin edges. Items sharing an origin align to their line group's maximum distance; the difference also contributes to the intrinsic line cross size. Previously columns skipped baseline measurement and used fallback positioning. No change to CSS parsing, animation, or CSSOM.

| Engine | Strict pages / 59 | Passing names / 584 | Runner wall | Browser CPU | HTTP port |
| --- | ---: | ---: | ---: | ---: | ---: |
| Parent | 9 | 388 | 0.395 s | 0.72 s | 51124 |
| Candidate | 21 | 409 | 0.371 s | 0.65 s | 64740 |
| Chrome 158.0.8127.37 | 57 | 580 | 5.796 s | 7.52 s | 55589 |

Candidate vs parent: **strict pages +12/-0; named passes +21/-0**. All gains also pass in Chrome. Six whole-page conversions have a horizontal column container and vertical-lr/rl block, flex, or table items; six have a vertical-lr/rl column container and horizontal flex, grid, or table items. Vertical grid items in horizontal containers still miss the second item's expected offset by 4px in opposite directions. Across the cohort, Chrome alone passes 175 names; candidate alone passes four. Row and column flex-basis calc-size controls remain 33/33 each. Width/height compressible controls and the min/max-content and column-wrap calc-size controls retain their parent outcomes.

`urls.json` is the exact focused manifest, checked against the archived main allowlist and withheld exclusions. WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; `bench/wpt/run.mjs --dirs css/css-flexbox,css/css-values/calc-size --tests urls.json --jobs 3 --server wptserve --testdriver`. All three runs had identical sets of named assertions, fresh contexts, complete OK harnesses, zero restarts and zero watchdog kills. Strict means every named assertion passes in a complete OK harness. `outcomes.json.gz` preserves URL → `[status,harnessStatus,[[name,status],…]]` for each engine. No full sweep. Single-run timings are not a performance comparison.

Studio `pyro-env.sh`, patched JSC `0e0dd7038a7a-6f4649e395e0`, isolated warm `wptiter` target; three build jobs, no simultaneous WPT. Parent binary SHA-256 `3eae3c4691d7db3d37d73b55046d0c849dccb3ee12ca418cd1f1d77503512440`, candidate `27fb06f9864c2dbbf559d27b899a0ebb06801458c04a3f038d96cb1f8dfe7014`. Final incremental browser build 25.88 s; `cargo test --locked --profile wptiter -p pyro-layout --test integration flow::flex` compiled in 24.32 s and passed 27/27 in 0.05 s, including both orthogonal baseline origins. Both vendor patches verified forward and reverse against the checkpoint.
