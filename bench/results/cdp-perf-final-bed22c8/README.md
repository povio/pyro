# CDP performance: start vs now vs upstream

One interleaved run, three binaries, 5 rounds, all 31 scenarios (Playwright off), 2026-10-27T13:56:07.076Z.
Host: Apple M1 Max, 10 CPUs, macOS 27.0.1; Load average at start 11.37/16.88/16.56, at end 8.78/14.82/15.80.

- **base**: 85ea4e8, the baseline before any optimization (release build in its own target directory).
- **head**: bed22c8, the tip after every step (opt-1..opt-7 and two merges of main).
- **upstream**: Lightpanda nightly `1.1.0-nightly.10661+738f61e00` (official release binary).

```sh
bun bench/scripts/cdp-perf.mjs --bin base=<85ea4e8 build> --bin head=<bed22c8 build> --bin upstream=<nightly> --rounds 5 --out final.json
bun bench/scripts/cdp-perf-compare.mjs final.json --base base --head head --no-fail --md compare-base-head.md
bun bench/scripts/cdp-perf-compare.mjs final.json --base upstream --head head --no-fail --md compare-upstream-head.md
```

base → head: 1 regression(s), 16 improvement(s), 3 unclear, 11 unchanged, 0 failed.
upstream → head: 9 regression(s), 10 improvement(s), 6 unclear, 6 unchanged, 0 failed.

Median of round medians (ms):

| scenario | start (85ea4e8) ms | now (bed22c8) ms | upstream ms | now vs start | now vs upstream |
|---|---:|---:|---:|---:|---:|
| connect.ws+getVersion | 1.083 | 1.494 | 1.570 | 38% | -5% |
| rtt.Browser.getVersion | 0.068 | 0.035 | 0.041 | -49% | -16% |
| rtt.Runtime.evaluate | 0.052 | 0.039 | 0.047 | -25% | -17% |
| throughput.evaluate.pipelined256 | 1.748 | 1.193 | 2.541 | -32% | -53% |
| runtime.evaluate.largeValue | 1.033 | 0.519 | 1.488 | -50% | -65% |
| runtime.callFunctionOn.small | 0.057 | 0.040 | 0.068 | -30% | -41% |
| runtime.callFunctionOn.38KB | 0.303 | 0.160 | 0.492 | -47% | -68% |
| runtime.handle+release | 0.114 | 0.083 | 0.108 | -28% | -24% |
| dom.querySelectorAll.2k | 0.324 | 0.318 | 0.204 | -2% | 56% |
| dom.textContent.2k | 0.161 | 0.147 | 0.128 | -8% | 15% |
| dom.mutate.1k | 3.545 | 2.701 | 0.966 | -24% | 180% |
| dom.handle.describe+quads | 0.310 | 0.210 | 0.229 | -32% | -8% |
| page.navigate.about:blank | 1.567 | 1.495 | 1.310 | -5% | 14% |
| page.navigate.simple | 1.929 | 1.595 | 1.594 | -17% | 0% |
| page.navigate.scripts | 4.831 | 4.804 | 3.563 | -1% | 35% |
| page.navigate.dom2k | 7.465 | 7.555 | 10.599 | 1% | -29% |
| target.lifecycle | 2.520 | 2.434 | 1.815 | -3% | 34% |
| network.fetch20 | 1.123 | 1.035 | 1.135 | -8% | -9% |
| fetch.intercept.fulfill | 0.234 | 0.205 | 0.162 | -12% | 27% |
| input.mouse.click | 0.139 | 0.110 | 0.093 | -21% | 17% |
| input.key.type | 0.110 | 0.085 | 0.088 | -23% | -4% |
| wv.open+navigate+close | 6.955 | 4.305 | 3.328 | -38% | 29% |
| wv.navigate.simple | 2.062 | 2.021 | 1.801 | -2% | 12% |
| wv.navigate.scripts | 5.340 | 5.394 | 4.076 | 1% | 32% |
| wv.navigate.dom2k | 7.742 | 7.986 | 10.794 | 3% | -26% |
| wv.evaluate | 0.100 | 0.073 | 0.064 | -27% | 14% |
| wv.evaluate.query2k | 0.357 | 0.367 | 0.249 | 3% | 47% |
| wv.cdp.Runtime.evaluate | 0.062 | 0.037 | 0.083 | -40% | -55% |
| wv.click.selector | 17.696 | 1.497 | 6.166 | -92% | -76% |
| wv.type | 0.119 | 0.095 | 0.104 | -21% | -9% |
| wv.flow.e2e | 47.485 | 11.901 | 19.696 | -75% | -40% |

The one regression from start, connect.ws+getVersion (+38%), comes from main's 8eb9f37, which starts the
engine and first realm for the next connection while serving the current one; it was flagged in
../main-merge-e31dfa9/compare.md.
