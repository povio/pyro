# Intrinsic block-size bounds on flex containers

Frozen parent `305840f45afbc4f55bfa24f303c1e858d30e4c1d`; source `dc56987ebe71a995f0de23d28e17bfe068b9cd12`. A definite preferred height previously bypassed the container's own `min-content` / `max-content` height bounds. Measure its content at the resolved inline constraint before the definite-size shortcut. Do not replace a height already assigned by the parent or one supplied by an automatic height's preferred aspect ratio.

Studio macOS, 2026-11-21. All three runs used WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, the frozen runner, `--dirs css/css-flexbox --tests urls.json --server wptserve --testdriver --jobs 4`, fresh contexts and ephemeral ports. The 231 URLs are a subset of the archived round-six main allowlist, disjoint from the withheld list. No general discovery run, omitted URL, or native fault/security case was executed. This is a focused result, not a whole-suite projection.

| Engine | Strict pages / 231 | Named passes / 2,316 | Runner wall | Browser CPU | wptserve HTTP port |
| --- | ---: | ---: | ---: | ---: | ---: |
| Frozen parent | 166 | 2,040 | 1.326 s | 3.30 s | 62268 |
| Candidate | 166 | 2,070 | 0.917 s | 2.25 s | 50254 |
| Chrome stable 158.0.8127.37 | 225 | 2,290 | 23.580 s | 29.94 s | 64140 |

All 231 harnesses completed OK, with identical observed name sets, zero restarts and zero watchdog kills. **Candidate vs parent: pages +0/-0; names +30/-0.** Only `flex-container-{min,max}-content-002.tentative.html` changed. On each, `.flex 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18, 20, 21, 24` changed to pass, taking each page from 8 to 23 of 24. Chrome passed 22 of 24 on each: `.flex 5` and `.flex 6` are WPT gains but **not** Chrome-parity gains; `.flex 23` remains Chrome-only. These single-run timings are not a speed claim.

Both LP binaries used Studio `pyro-env.sh`, patched JSC `0e0dd7038a7a-6f4649e395e0`, isolated `CARGO_TARGET_DIR`, `pyro-build`, four build jobs and no simultaneous WPT jobs. Parent build 210 s; final incremental candidate build 18 s. SHA-256: parent `50c39eb1d6161c8ef97d2063bddf94c53fa73ab8b7995b4d9448f5b3fe3a0880`, candidate `f86ed39554e06a09811c895195ff794b1fe586f1e9f4d499697f3d1aec2fa650`. The native `pyro-layout` integration regression passed (0.05 s), as did all 25 `flow::flex` controls (0.06 s). The Taffy vendor patch passed forward/reverse verification against the frozen base.

`outcomes.json.gz` contains the exact `[runner status, harness status, [[name, status], …]]` for every URL and engine; `urls.json` is the replay allowlist.
