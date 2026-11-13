# CDP performance: upstream vs rewrite

2026-10-27T10:28:48.594Z on Apple M1 Max (10 CPUs, darwin 27.0.0), 5 round(s), scale 1.
Load average at start 9.79/13.69/17.69, at end 11.39/13.10/17.15.

- **upstream**: `/Users/xban1x/Documents/Codex/2026-10-16/loa/work/lightpanda/bin/lightpanda` sha256 `f1156d20d4ef9b16`, startup median 64.758 ms
- **rewrite**: `/Users/xban1x/Povio/pyro-cdp-perf-wt/target/release/pyro` sha256 `a26f6a1f0cc4748b`, startup median 6.564 ms

Times are milliseconds per sample (median, p90 over all rounds); `rsd` is the robust relative spread; `cpu/op` is browser CPU per operation.

| scenario | upstream median | p90 | rsd | ops/s | cpu/op | rewrite median | p90 | rsd | ops/s | cpu/op |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| connect.ws+getVersion | 1.472 | 1.706 | 0.11 | 680 | 1.362 | 1.053 | 1.288 | 0.14 | 950 | 1.173 |
| rtt.Browser.getVersion | 0.052 | 0.112 | 0.51 | 19120 | 0.020 | 0.048 | 0.095 | 0.28 | 20877 | 0.024 |
| rtt.Runtime.evaluate | 0.051 | 0.085 | 0.24 | 19685 | 0.027 | 0.052 | 0.089 | 0.24 | 19380 | 0.032 |
| throughput.evaluate.pipelined256 | 2.551 | 3.620 | 0.14 | 100369 | 0.012 | 1.787 | 2.532 | 0.11 | 143249 | 0.011 |
| runtime.evaluate.largeValue | 1.487 | 1.695 | 0.08 | 672 | 1.488 | 1.032 | 1.365 | 0.12 | 969 | 0.953 |
| runtime.callFunctionOn.small | 0.093 | 0.143 | 0.35 | 10718 | 0.043 | 0.065 | 0.114 | 0.33 | 15504 | 0.038 |
| runtime.callFunctionOn.38KB | 0.526 | 0.615 | 0.10 | 1900 | 0.413 | 0.333 | 0.419 | 0.14 | 3000 | 0.243 |
| runtime.handle+release | 0.111 | 0.205 | 0.27 | 9009 | 0.060 | 0.118 | 0.201 | 0.26 | 8503 | 0.069 |
| dom.querySelectorAll.2k | 0.218 | 0.287 | 0.17 | 4577 | 0.182 | 0.326 | 0.427 | 0.11 | 3070 | 0.303 |
| dom.textContent.2k | 0.150 | 0.211 | 0.25 | 6658 | 0.109 | 0.179 | 0.270 | 0.22 | 5574 | 0.158 |
| dom.mutate.1k | 0.990 | 1.235 | 0.11 | 1011 | 0.954 | 3.487 | 3.958 | 0.07 | 287 | 3.417 |
| dom.handle.describe+quads | 0.223 | 0.437 | 0.26 | 4480 | 0.115 | 0.281 | 0.447 | 0.21 | 3560 | 0.187 |
| page.navigate.about:blank | 1.335 | 1.560 | 0.10 | 749 | 2.640 | 1.498 | 2.018 | 0.12 | 667 | 2.473 |
| page.navigate.simple | 1.601 | 1.856 | 0.09 | 624 | 2.840 | 1.792 | 2.393 | 0.12 | 558 | 2.580 |
| page.navigate.scripts | 3.654 | 4.347 | 0.11 | 274 | 4.551 | 5.018 | 5.997 | 0.12 | 199 | 5.511 |
| page.navigate.dom2k | 10.726 | 11.832 | 0.04 | 93 | 11.941 | 7.646 | 8.455 | 0.03 | 131 | 8.539 |
| target.lifecycle | 1.754 | 2.133 | 0.13 | 570 | 1.622 | 2.429 | 3.268 | 0.11 | 412 | 2.864 |
| network.fetch20 | 1.116 | 1.367 | 0.12 | 896 | 1.067 | 1.143 | 1.594 | 0.20 | 875 | 2.264 |
| fetch.intercept.fulfill | 0.161 | 0.265 | 0.25 | 6219 | 0.101 | 0.225 | 0.323 | 0.23 | 4440 | 0.191 |
| input.mouse.click | 0.102 | 0.165 | 0.26 | 9852 | 0.050 | 0.139 | 0.204 | 0.20 | 7189 | 0.093 |
| input.key.type | 0.088 | 0.139 | 0.20 | 11351 | 0.040 | 0.114 | 0.171 | 0.20 | 8780 | 0.066 |
| wv.open+navigate+close | 3.332 | 3.884 | 0.13 | 300 | 2.986 | 6.650 | 7.384 | 0.06 | 150 | 9.189 |
| wv.navigate.simple | 1.768 | 1.998 | 0.09 | 566 | 2.865 | 2.068 | 2.985 | 0.10 | 484 | 3.005 |
| wv.navigate.scripts | 3.742 | 4.238 | 0.11 | 267 | 4.596 | 4.989 | 6.538 | 0.16 | 200 | 5.601 |
| wv.navigate.dom2k | 10.825 | 11.230 | 0.02 | 92 | 11.930 | 7.570 | 8.573 | 0.04 | 132 | 8.710 |
| wv.evaluate | 0.085 | 0.140 | 0.50 | 11806 | 0.044 | 0.079 | 0.145 | 0.30 | 12610 | 0.066 |
| wv.evaluate.query2k | 0.225 | 0.298 | 0.20 | 4446 | 0.185 | 0.356 | 0.446 | 0.11 | 2809 | 0.327 |
| wv.cdp.Runtime.evaluate | 0.076 | 0.122 | 0.50 | 13106 | 0.031 | 0.052 | 0.114 | 0.30 | 19120 | 0.032 |
| wv.click.selector | 6.211 | 8.393 | 0.04 | 161 | 0.424 | 18.094 | 21.744 | 0.04 | 55 | 1.015 |
| wv.type | 0.085 | 0.138 | 0.29 | 11737 | 0.027 | 0.109 | 0.170 | 0.25 | 9149 | 0.045 |
| wv.flow.e2e | 19.550 | 22.712 | 0.04 | 51 | 4.856 | 49.047 | 53.889 | 0.07 | 20 | 15.812 |

## Phase breakdown (median ms since the step started)

- page.navigate.about:blank [upstream]: navigateReply 1.285, frameNavigated 1.298, domContentEventFired 1.329, loadEventFired 1.332
- page.navigate.about:blank [rewrite]: navigateReply 1.475, frameNavigated 1.489, domContentEventFired 1.494, loadEventFired 1.495
- page.navigate.simple [upstream]: navigateReply 1.500, frameNavigated 1.509, domContentEventFired 1.596, loadEventFired 1.599
- page.navigate.simple [rewrite]: navigateReply 1.759, frameNavigated 1.776, domContentEventFired 1.784, loadEventFired 1.789
- page.navigate.scripts [upstream]: navigateReply 1.111, frameNavigated 1.118, domContentEventFired 3.026, loadEventFired 3.030
- page.navigate.scripts [rewrite]: navigateReply 1.758, frameNavigated 1.772, domContentEventFired 4.861, loadEventFired 4.864
- page.navigate.dom2k [upstream]: navigateReply 1.732, frameNavigated 1.743, domContentEventFired 10.718, loadEventFired 10.722
- page.navigate.dom2k [rewrite]: navigateReply 2.589, frameNavigated 2.602, domContentEventFired 7.639, loadEventFired 7.642
- target.lifecycle [upstream]: createBrowserContext 0.095, createTarget 0.999, attachToTarget 1.115, enable 1.412, closeTarget 1.510, disposeBrowserContext 1.751
- target.lifecycle [rewrite]: createBrowserContext 0.131, createTarget 1.578, attachToTarget 1.712, enable 2.080, closeTarget 2.255, disposeBrowserContext 2.426
- wv.flow.e2e [upstream]: navigate 4.307, input 16.776, result 19.508, close 19.546
- wv.flow.e2e [rewrite]: navigate 9.468, input 46.155, result 48.991, close 49.037
