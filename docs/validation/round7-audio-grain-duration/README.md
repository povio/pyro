# Fractional buffer-source grain durations

The signed fixed-head census `01f49f43` isolated two one-sample overruns in `sub-sample-scheduling.html`: output0[38] and output1[44] were 1, not 0. Base `8fe1ce87c` reproduced both. Position interpolation already advances from the scheduled start to the first output sample, but the finite-duration counter previously started at zero at that sample. Candidate `2009f8b71` counts the fractional interval in source-frame units at the absolute effective playback rate. It does not charge the initial buffer offset as elapsed playback.

| Mode | Fully passing pages | Passing/emitted assertions | Process wall including launch |
|---|---:|---:|---:|
| Base module | 2/3 | 52/58 | 0.82 s |
| Candidate module | 3/3 | 58/58 | 0.77 s |
| Native signed Chrome 158.0.8127.37 | 3/3 | 58/58 | 0.76 s |

Four gains are direct signal checks (two full output arrays and two first-silent-frame assertions); two are summaries. No lost passes. Both looping-duration controls, including playback-rate changes, retained all seven passes. Final candidate and native matched every harness status, assertion name and outcome.

Independent Node VM ramp-signal checks: all 11 candidate cases passed; the base overran by one nonzero sample in eight cases (including reverse, loop, half/double rate, resampling and quantum-boundary starts). Integer starts and omitted durations stayed correct. QA flagged a possible silent native fallback in the harness. Final source head `99e0b823d` asserts the installed constructors on each module-mode page. An empty-module negative control failed with the intended assertion. The guarded matched runs produced byte-identical named receipts to the initial measurements; the table uses the guarded timings.

WPT checkout `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Reproduce with `node tests/e2e/audio-grain-duration.mjs "$CHROME" "$WPT_DIR" [module-path]`. The base and candidate runs install the actual repository Web Audio factory in the same signed browser over a pipe; omit the module for native. Base exits 1 after printing its expected failures. Only three named ordinary pages and their local dependencies are served from the checkout via request interception. No Pyro executable rebuilt or claimed, no Cargo target or ports, no withheld/crash fixtures. End sample: load 1.29/1.35/1.40, 85% memory free, 14 GiB disk available.
