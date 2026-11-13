# CDP performance: base vs head vs upstream

2026-10-27T13:56:07.076Z on Apple M1 Max (10 CPUs, darwin 27.0.0), 5 round(s), scale 1.
Load average at start 11.37/16.88/16.56, at end 8.78/14.82/15.80.

- **base**: `/Users/xban1x/Povio/pyro-cdp-perf-work/target-85ea4e863f297b6a7822be6e2239adf0ef28ba0c/release/pyro` sha256 `4e6648a314f3d0af`, startup median 6.637 ms
- **head**: `/Users/xban1x/Povio/pyro-cdp-perf-work/target-bed22c8768eeae720b4d89354bf9d9d905397060/release/pyro` sha256 `6ddc110074575140`, startup median 6.625 ms
- **upstream**: `/Users/xban1x/Documents/Codex/2026-10-16/loa/work/lightpanda/bin/lightpanda` sha256 `f1156d20d4ef9b16`, startup median 66.080 ms

Times are milliseconds per sample (median, p90 over all rounds); `rsd` is the robust relative spread; `cpu/op` is browser CPU per operation.

| scenario | base median | p90 | rsd | ops/s | cpu/op | head median | p90 | rsd | ops/s | cpu/op | upstream median | p90 | rsd | ops/s | cpu/op |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| connect.ws+getVersion | 1.057 | 1.318 | 0.16 | 946 | 1.140 | 1.523 | 2.605 | 0.94 | 657 | 3.163 | 1.497 | 1.801 | 0.12 | 668 | 1.378 |
| rtt.Browser.getVersion | 0.061 | 0.127 | 0.49 | 16340 | 0.027 | 0.036 | 0.105 | 0.26 | 27778 | 0.013 | 0.043 | 0.096 | 0.31 | 22989 | 0.018 |
| rtt.Runtime.evaluate | 0.054 | 0.096 | 0.24 | 18622 | 0.031 | 0.039 | 0.067 | 0.25 | 25773 | 0.017 | 0.046 | 0.076 | 0.19 | 21692 | 0.026 |
| throughput.evaluate.pipelined256 | 1.782 | 2.412 | 0.12 | 143683 | 0.011 | 1.208 | 1.642 | 0.13 | 211885 | 0.004 | 2.513 | 2.999 | 0.09 | 101870 | 0.012 |
| runtime.evaluate.largeValue | 1.026 | 1.222 | 0.09 | 975 | 0.933 | 0.528 | 0.673 | 0.13 | 1893 | 0.452 | 1.466 | 1.725 | 0.09 | 682 | 1.484 |
| runtime.callFunctionOn.small | 0.057 | 0.097 | 0.22 | 17452 | 0.035 | 0.038 | 0.063 | 0.22 | 26178 | 0.017 | 0.069 | 0.121 | 0.35 | 14472 | 0.040 |
| runtime.callFunctionOn.38KB | 0.298 | 0.375 | 0.14 | 3357 | 0.244 | 0.155 | 0.220 | 0.20 | 6443 | 0.087 | 0.501 | 0.597 | 0.11 | 1998 | 0.406 |
| runtime.handle+release | 0.117 | 0.175 | 0.21 | 8562 | 0.069 | 0.087 | 0.135 | 0.26 | 11534 | 0.038 | 0.110 | 0.171 | 0.24 | 9074 | 0.060 |
| dom.querySelectorAll.2k | 0.330 | 0.408 | 0.11 | 3030 | 0.301 | 0.318 | 0.400 | 0.12 | 3146 | 0.286 | 0.201 | 0.253 | 0.11 | 4975 | 0.176 |
| dom.textContent.2k | 0.164 | 0.228 | 0.15 | 6116 | 0.151 | 0.150 | 0.219 | 0.14 | 6653 | 0.150 | 0.130 | 0.179 | 0.15 | 7704 | 0.106 |
| dom.mutate.1k | 3.547 | 3.968 | 0.09 | 282 | 3.449 | 2.675 | 3.087 | 0.11 | 374 | 2.622 | 0.962 | 1.237 | 0.13 | 1040 | 0.932 |
| dom.handle.describe+quads | 0.291 | 0.497 | 0.25 | 3432 | 0.186 | 0.205 | 0.285 | 0.18 | 4888 | 0.106 | 0.227 | 0.382 | 0.26 | 4403 | 0.120 |
| page.navigate.about:blank | 1.528 | 2.261 | 0.13 | 655 | 2.549 | 1.468 | 2.075 | 0.14 | 681 | 2.406 | 1.307 | 1.460 | 0.08 | 765 | 2.576 |
| page.navigate.simple | 1.841 | 2.603 | 0.14 | 543 | 2.662 | 1.604 | 2.232 | 0.13 | 624 | 2.606 | 1.542 | 1.788 | 0.10 | 648 | 2.738 |
| page.navigate.scripts | 4.859 | 6.015 | 0.14 | 206 | 5.387 | 4.954 | 6.009 | 0.13 | 202 | 5.383 | 3.620 | 4.245 | 0.09 | 276 | 4.352 |
| page.navigate.dom2k | 7.620 | 8.202 | 0.04 | 131 | 8.319 | 7.450 | 8.343 | 0.06 | 134 | 8.541 | 10.671 | 11.537 | 0.03 | 94 | 11.841 |
| target.lifecycle | 2.561 | 3.520 | 0.12 | 390 | 2.956 | 2.427 | 3.090 | 0.11 | 412 | 2.809 | 1.794 | 2.407 | 0.12 | 558 | 1.657 |
| network.fetch20 | 1.113 | 1.440 | 0.18 | 898 | 2.175 | 1.084 | 3.422 | 0.18 | 922 | 1.691 | 1.135 | 1.400 | 0.11 | 881 | 1.081 |
| fetch.intercept.fulfill | 0.229 | 0.352 | 0.21 | 4359 | 0.204 | 0.206 | 0.429 | 0.22 | 4864 | 0.153 | 0.169 | 0.306 | 0.29 | 5921 | 0.109 |
| input.mouse.click | 0.140 | 0.218 | 0.21 | 7163 | 0.095 | 0.112 | 0.165 | 0.22 | 8913 | 0.065 | 0.095 | 0.143 | 0.18 | 10526 | 0.049 |
| input.key.type | 0.113 | 0.176 | 0.20 | 8857 | 0.068 | 0.088 | 0.145 | 0.22 | 11364 | 0.041 | 0.090 | 0.137 | 0.22 | 11111 | 0.041 |
| wv.open+navigate+close | 6.649 | 7.554 | 0.10 | 150 | 9.284 | 4.208 | 5.655 | 0.13 | 238 | 10.181 | 3.259 | 3.786 | 0.10 | 307 | 3.034 |
| wv.navigate.simple | 2.076 | 2.991 | 0.10 | 482 | 2.920 | 1.990 | 2.916 | 0.12 | 502 | 3.044 | 1.799 | 2.113 | 0.09 | 556 | 2.874 |
| wv.navigate.scripts | 5.231 | 6.569 | 0.14 | 191 | 5.946 | 5.317 | 6.609 | 0.14 | 188 | 6.021 | 3.933 | 5.014 | 0.15 | 254 | 4.798 |
| wv.navigate.dom2k | 7.735 | 8.986 | 0.06 | 129 | 8.931 | 7.928 | 9.166 | 0.07 | 126 | 9.092 | 10.794 | 12.142 | 0.03 | 93 | 11.927 |
| wv.evaluate | 0.092 | 0.173 | 0.38 | 10870 | 0.068 | 0.065 | 0.114 | 0.30 | 15361 | 0.057 | 0.071 | 0.123 | 0.39 | 14144 | 0.042 |
| wv.evaluate.query2k | 0.369 | 0.467 | 0.14 | 2710 | 0.346 | 0.361 | 0.512 | 0.17 | 2772 | 0.321 | 0.245 | 0.344 | 0.27 | 4077 | 0.196 |
| wv.cdp.Runtime.evaluate | 0.064 | 0.116 | 0.41 | 15576 | 0.035 | 0.038 | 0.068 | 0.27 | 26525 | 0.018 | 0.076 | 0.140 | 0.54 | 13141 | 0.033 |
| wv.click.selector | 17.749 | 20.135 | 0.01 | 56 | 0.617 | 1.509 | 1.658 | 0.06 | 663 | 0.247 | 6.139 | 6.439 | 0.02 | 163 | 0.331 |
| wv.type | 0.112 | 0.161 | 0.25 | 8921 | 0.042 | 0.098 | 0.142 | 0.32 | 10256 | 0.028 | 0.094 | 0.149 | 0.30 | 10638 | 0.028 |
| wv.flow.e2e | 47.405 | 49.705 | 0.02 | 21 | 13.400 | 11.901 | 12.560 | 0.04 | 84 | 12.266 | 19.661 | 20.278 | 0.02 | 51 | 4.946 |

## Phase breakdown (median ms since the step started)

- page.navigate.about:blank [base]: navigateReply 1.502, frameNavigated 1.512, domContentEventFired 1.522, loadEventFired 1.525
- page.navigate.about:blank [head]: navigateReply 1.451, frameNavigated 1.459, domContentEventFired 1.462, loadEventFired 1.466
- page.navigate.about:blank [upstream]: navigateReply 1.265, frameNavigated 1.273, domContentEventFired 1.298, loadEventFired 1.306
- page.navigate.simple [base]: navigateReply 1.810, frameNavigated 1.821, domContentEventFired 1.835, loadEventFired 1.837
- page.navigate.simple [head]: navigateReply 1.574, frameNavigated 1.586, domContentEventFired 1.601, loadEventFired 1.602
- page.navigate.simple [upstream]: navigateReply 1.451, frameNavigated 1.460, domContentEventFired 1.537, loadEventFired 1.539
- page.navigate.scripts [base]: navigateReply 1.705, frameNavigated 1.717, domContentEventFired 4.701, loadEventFired 4.704
- page.navigate.scripts [head]: navigateReply 1.614, frameNavigated 1.627, domContentEventFired 4.813, loadEventFired 4.816
- page.navigate.scripts [upstream]: navigateReply 1.113, frameNavigated 1.125, domContentEventFired 3.022, loadEventFired 3.024
- page.navigate.dom2k [base]: navigateReply 2.593, frameNavigated 2.613, domContentEventFired 7.611, loadEventFired 7.614
- page.navigate.dom2k [head]: navigateReply 2.314, frameNavigated 2.330, domContentEventFired 7.442, loadEventFired 7.445
- page.navigate.dom2k [upstream]: navigateReply 1.719, frameNavigated 1.735, domContentEventFired 10.663, loadEventFired 10.667
- target.lifecycle [base]: createBrowserContext 0.143, createTarget 1.649, attachToTarget 1.799, enable 2.176, closeTarget 2.375, disposeBrowserContext 2.557
- target.lifecycle [head]: createBrowserContext 0.118, createTarget 1.617, attachToTarget 1.744, enable 2.078, closeTarget 2.262, disposeBrowserContext 2.424
- target.lifecycle [upstream]: createBrowserContext 0.101, createTarget 1.034, attachToTarget 1.152, enable 1.454, closeTarget 1.550, disposeBrowserContext 1.787
- wv.flow.e2e [base]: navigate 7.944, input 44.475, result 47.359, close 47.402
- wv.flow.e2e [head]: navigate 5.015, input 9.132, result 11.865, close 11.896
- wv.flow.e2e [upstream]: navigate 4.230, input 16.834, result 19.563, close 19.657
