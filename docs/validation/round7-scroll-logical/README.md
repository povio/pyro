# Logical scrollIntoView alignment

Parent `6e0a8f847`, source `1e181cdd`. The archived batch18 list was joined by exact URL/name to the signed Chrome reference, then reproduced on the combined binary before editing: 60 Fail→Chrome Pass on eight pages. CSSOM View §6.1 determines block and inline edges from **each scrolling box**, not the target. Pyro previously used inline for physical X and block for Y regardless of the scrolling box. The correction maps those axes and their beginning edges before the existing offset/clamp path. Physical reveal policies and scroll ranges are unchanged.

| Focus (direction) | Baseline gaps | Recovered | Still failing |
|---|---:|---:|---:|
| horizontal-tb RTL | 6 | 6 | 0 |
| vertical-lr LTR / RTL | 6 / 8 | 6 / 8 | 0 / 0 |
| vertical-rl LTR | 8 | 8 | 0 |
| sideways-rl LTR / RTL | 8 / 6 | 8 / 6 | 0 / 0 |
| sideways-lr LTR / RTL | 9 / 9 | 0 / 0 | 9 / 9 |

**42 named gains, zero losses, six newly all-pass pages.** Horizontal LTR (9 passes), scroll margin (3 passes), and scroll padding (3 preexisting failures) unchanged. All 87 names repeated on both builds. No full suite. No grid diagnostics or score changes.

**Independent geometry controls:** 160 fresh positioned targets (5 writing modes × 2 directions × 4 block × 4 inline alignments); target position relative to scroller and both scroll offsets recorded before and after. Exact Chrome matches 56→160. The 18 residuals use floats. With scripts removed from the two pinned fixtures the target starts **400px** from the left in Pyro versus **200px** in Chrome, before any scrolling; both Pyro builds give identical starting geometry. This is not an alignment-only correction; no layout workaround. The float diagnostics use default CDP page metrics, not WPT scrollbar emulation.

**Reproduction:** matched WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, runner tree `7efc9828957ae18dbd8c8c7df59cba3c59524633`. Primary combined binary SHA-256 `cdb297969010a92f4a68040bd7a81a9378ef4b767a5e81c23fbe5d7b1dd488e5`; private parent `4c553dad122b56a96fb6d72b9ebef5ea0b4da6420c681045365088ba90772644`; candidate `5971523a648695f657e86b4b01bd66d3a871e34ac074f592f8449da64b95ddf6`. Private build: `CARGO_BUILD_JOBS=3 pyro-build --features pyro/render`. Run `bench/wpt/run.mjs` with Bun, `--tests` pointing at this manifest, `--wpt "$WPT_DIR" --kind testharness --dirs css --server wptserve --testdriver --jobs 1 --bin label=/path/to/pyro --out label.json`. Manifest contains only completed archived pages and excludes withheld URLs. `chrome.json` is the corresponding subset of signed Chrome main. The direct probes use local signed Chrome 158.0.8127.37, verified team EQHXZ8M8AV; run each `.cjs` from the repository root using Bun with argument `chrome` or an isolated Pyro CDP HTTP endpoint. Run `python3 docs/validation/round7-scroll-logical/verify.py` to print exact directional name sets.
