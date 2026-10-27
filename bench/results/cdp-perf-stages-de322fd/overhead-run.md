# CDP performance: plain vs traced

2026-10-27T10:41:59.386Z on Apple M1 Max (10 CPUs, darwin 27.0.0), 3 round(s), scale 1.
Load average at start 9.28/10.83/13.58, at end 8.32/10.42/13.32.

- **plain**: `/Users/xban1x/Povio/lp-cdp-perf-wt/target/release/lightningpanda` sha256 `0c9413a63e236452`, startup median 6.744 ms
- **traced**: `/Users/xban1x/Povio/lp-cdp-perf-wt/target-perf/release/lightningpanda` sha256 `2a62c32ca3f66da0`, startup median 6.737 ms

Times are milliseconds per sample (median, p90 over all rounds); `rsd` is the robust relative spread; `cpu/op` is browser CPU per operation.

| scenario | plain median | p90 | rsd | ops/s | cpu/op | traced median | p90 | rsd | ops/s | cpu/op |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| connect.ws+getVersion | 1.106 | 1.306 | 0.12 | 904 | 1.241 | 1.038 | 1.313 | 0.15 | 964 | 1.161 |
| rtt.Browser.getVersion | 0.056 | 0.104 | 0.42 | 17825 | 0.025 | 0.051 | 0.095 | 0.32 | 19531 | 0.026 |
| rtt.Runtime.evaluate | 0.054 | 0.085 | 0.24 | 18450 | 0.031 | 0.056 | 0.090 | 0.23 | 18018 | 0.034 |
| throughput.evaluate.pipelined256 | 1.796 | 2.875 | 0.14 | 142571 | 0.011 | 1.834 | 2.253 | 0.09 | 139601 | 0.012 |
| runtime.evaluate.largeValue | 1.011 | 1.168 | 0.10 | 989 | 0.914 | 1.026 | 1.601 | 0.13 | 975 | 0.928 |
| runtime.callFunctionOn.small | 0.061 | 0.105 | 0.27 | 16447 | 0.036 | 0.059 | 0.099 | 0.27 | 16892 | 0.037 |
| runtime.callFunctionOn.38KB | 0.290 | 0.371 | 0.15 | 3448 | 0.238 | 0.290 | 0.365 | 0.16 | 3453 | 0.234 |
| runtime.handle+release | 0.120 | 0.175 | 0.22 | 8326 | 0.072 | 0.115 | 0.171 | 0.19 | 8688 | 0.071 |
| dom.querySelectorAll.2k | 0.336 | 0.404 | 0.12 | 2976 | 0.304 | 0.327 | 0.390 | 0.12 | 3054 | 0.300 |
| dom.textContent.2k | 0.162 | 0.220 | 0.13 | 6169 | 0.153 | 0.174 | 0.225 | 0.18 | 5754 | 0.156 |
| dom.mutate.1k | 3.475 | 3.982 | 0.08 | 288 | 3.431 | 3.488 | 4.004 | 0.10 | 287 | 3.388 |
| dom.handle.describe+quads | 0.297 | 0.481 | 0.25 | 3362 | 0.190 | 0.289 | 0.443 | 0.22 | 3455 | 0.190 |
| page.navigate.about:blank | 1.515 | 2.035 | 0.12 | 660 | 2.471 | 1.493 | 2.023 | 0.10 | 670 | 2.471 |
| page.navigate.simple | 1.915 | 2.604 | 0.13 | 522 | 2.641 | 1.792 | 2.276 | 0.11 | 558 | 2.553 |
| page.navigate.scripts | 5.005 | 5.810 | 0.09 | 200 | 5.456 | 5.054 | 6.276 | 0.12 | 198 | 5.568 |
| page.navigate.dom2k | 7.678 | 8.149 | 0.03 | 130 | 8.403 | 7.651 | 8.185 | 0.03 | 131 | 8.441 |
| target.lifecycle | 2.506 | 3.320 | 0.08 | 399 | 2.939 | 2.500 | 3.246 | 0.11 | 400 | 2.929 |
| network.fetch20 | 1.139 | 1.437 | 0.16 | 878 | 2.138 | 1.120 | 1.438 | 0.16 | 893 | 2.210 |
| fetch.intercept.fulfill | 0.222 | 0.302 | 0.17 | 4511 | 0.187 | 0.250 | 0.340 | 0.22 | 3994 | 0.217 |
| input.mouse.click | 0.152 | 0.260 | 0.26 | 6588 | 0.100 | 0.134 | 0.195 | 0.19 | 7457 | 0.094 |
| input.key.type | 0.119 | 0.176 | 0.24 | 8432 | 0.071 | 0.112 | 0.175 | 0.17 | 8921 | 0.068 |
| wv.open+navigate+close | 6.703 | 8.607 | 0.09 | 149 | 9.284 | 6.583 | 7.157 | 0.06 | 152 | 9.073 |
| wv.navigate.simple | 2.009 | 2.659 | 0.12 | 498 | 2.921 | 2.139 | 2.974 | 0.10 | 468 | 2.919 |
| wv.navigate.scripts | 5.125 | 6.502 | 0.11 | 195 | 5.706 | 5.113 | 5.983 | 0.10 | 196 | 5.640 |
| wv.navigate.dom2k | 7.659 | 8.375 | 0.04 | 131 | 8.756 | 7.656 | 8.297 | 0.05 | 131 | 8.755 |
| wv.evaluate | 0.094 | 0.173 | 0.38 | 10650 | 0.072 | 0.082 | 0.147 | 0.28 | 12165 | 0.066 |
| wv.evaluate.query2k | 0.347 | 0.421 | 0.14 | 2879 | 0.323 | 0.366 | 0.458 | 0.14 | 2731 | 0.323 |
| wv.cdp.Runtime.evaluate | 0.060 | 0.128 | 0.43 | 16584 | 0.033 | 0.060 | 0.107 | 0.34 | 16779 | 0.036 |
| wv.click.selector | 18.713 | 22.723 | 0.07 | 53 | 1.044 | 18.622 | 22.422 | 0.07 | 54 | 0.982 |
| wv.type | 0.106 | 0.153 | 0.18 | 9398 | 0.044 | 0.097 | 0.129 | 0.15 | 10363 | 0.042 |
| wv.flow.e2e | 47.788 | 51.412 | 0.02 | 21 | 14.225 | 48.234 | 56.207 | 0.03 | 21 | 15.292 |

## Phase breakdown (median ms since the step started)

- page.navigate.about:blank [plain]: navigateReply 1.484, frameNavigated 1.504, domContentEventFired 1.512, loadEventFired 1.513
- page.navigate.about:blank [traced]: navigateReply 1.468, frameNavigated 1.482, domContentEventFired 1.486, loadEventFired 1.491
- page.navigate.simple [plain]: navigateReply 1.872, frameNavigated 1.896, domContentEventFired 1.912, loadEventFired 1.913
- page.navigate.simple [traced]: navigateReply 1.765, frameNavigated 1.780, domContentEventFired 1.788, loadEventFired 1.790
- page.navigate.scripts [plain]: navigateReply 1.745, frameNavigated 1.759, domContentEventFired 4.824, loadEventFired 4.831
- page.navigate.scripts [traced]: navigateReply 1.798, frameNavigated 1.816, domContentEventFired 4.895, loadEventFired 4.898
- page.navigate.dom2k [plain]: navigateReply 2.641, frameNavigated 2.657, domContentEventFired 7.669, loadEventFired 7.673
- page.navigate.dom2k [traced]: navigateReply 2.629, frameNavigated 2.646, domContentEventFired 7.642, loadEventFired 7.646
- target.lifecycle [plain]: createBrowserContext 0.136, createTarget 1.611, attachToTarget 1.772, enable 2.131, closeTarget 2.345, disposeBrowserContext 2.501
- target.lifecycle [traced]: createBrowserContext 0.128, createTarget 1.594, attachToTarget 1.738, enable 2.140, closeTarget 2.329, disposeBrowserContext 2.497
- wv.flow.e2e [plain]: navigate 8.414, input 45.002, result 47.757, close 47.783
- wv.flow.e2e [traced]: navigate 8.710, input 45.396, result 48.197, close 48.227
