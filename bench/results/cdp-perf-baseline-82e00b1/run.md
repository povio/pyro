# CDP performance: a vs b

2026-10-27T10:26:40.310Z on Apple M1 Max (10 CPUs, darwin 27.0.0), 5 round(s), scale 1.
Load average at start 14.62/14.60/18.52, at end 16.75/15.37/18.53.

- **a**: `/Users/xban1x/Povio/pyro-cdp-perf-wt/target/release/pyro` sha256 `a26f6a1f0cc4748b`, startup median 6.820 ms
- **b**: `/Users/xban1x/Povio/pyro-cdp-perf-wt/target/release/pyro` sha256 `a26f6a1f0cc4748b`, startup median 6.752 ms

Times are milliseconds per sample (median, p90 over all rounds); `rsd` is the robust relative spread; `cpu/op` is browser CPU per operation.

| scenario | a median | p90 | rsd | ops/s | cpu/op | b median | p90 | rsd | ops/s | cpu/op |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| connect.ws+getVersion | 1.054 | 1.293 | 0.17 | 949 | 1.121 | 1.053 | 1.411 | 0.13 | 949 | 1.133 |
| rtt.Browser.getVersion | 0.055 | 0.108 | 0.41 | 18116 | 0.028 | 0.047 | 0.099 | 0.36 | 21142 | 0.026 |
| rtt.Runtime.evaluate | 0.053 | 0.103 | 0.34 | 18727 | 0.032 | 0.057 | 0.099 | 0.31 | 17575 | 0.034 |
| throughput.evaluate.pipelined256 | 1.796 | 2.361 | 0.11 | 142531 | 0.011 | 1.803 | 2.225 | 0.13 | 141986 | 0.012 |
| runtime.evaluate.largeValue | 1.010 | 1.345 | 0.12 | 990 | 0.918 | 1.013 | 1.254 | 0.10 | 987 | 0.929 |
| runtime.callFunctionOn.small | 0.056 | 0.094 | 0.25 | 17857 | 0.037 | 0.056 | 0.097 | 0.23 | 17794 | 0.037 |
| runtime.callFunctionOn.38KB | 0.303 | 0.428 | 0.17 | 3299 | 0.250 | 0.291 | 0.387 | 0.15 | 3438 | 0.238 |
| runtime.handle+release | 0.118 | 0.182 | 0.24 | 8489 | 0.069 | 0.122 | 0.195 | 0.25 | 8203 | 0.074 |
| dom.querySelectorAll.2k | 0.345 | 0.444 | 0.14 | 2899 | 0.313 | 0.336 | 0.447 | 0.13 | 2974 | 0.311 |
| dom.textContent.2k | 0.170 | 0.254 | 0.18 | 5886 | 0.161 | 0.163 | 0.225 | 0.15 | 6124 | 0.157 |
| dom.mutate.1k | 3.549 | 4.215 | 0.10 | 282 | 3.446 | 3.531 | 4.010 | 0.09 | 283 | 3.449 |
| dom.handle.describe+quads | 0.293 | 0.494 | 0.26 | 3414 | 0.185 | 0.284 | 0.474 | 0.27 | 3524 | 0.183 |
| page.navigate.about:blank | 1.522 | 2.367 | 0.13 | 657 | 2.561 | 1.538 | 2.334 | 0.13 | 650 | 2.565 |
| page.navigate.simple | 1.845 | 2.476 | 0.12 | 542 | 2.567 | 1.816 | 2.467 | 0.11 | 551 | 2.583 |
| page.navigate.scripts | 5.220 | 6.727 | 0.13 | 192 | 5.840 | 5.253 | 6.191 | 0.11 | 190 | 5.692 |
| page.navigate.dom2k | 8.097 | 10.175 | 0.08 | 123 | 8.963 | 7.892 | 11.245 | 0.07 | 127 | 8.664 |
| target.lifecycle | 2.496 | 3.458 | 0.12 | 401 | 3.008 | 2.577 | 3.599 | 0.14 | 388 | 3.043 |
| network.fetch20 | 1.214 | 1.626 | 0.16 | 823 | 2.310 | 1.217 | 2.004 | 0.16 | 822 | 2.304 |
| fetch.intercept.fulfill | 0.247 | 0.370 | 0.25 | 4047 | 0.209 | 0.247 | 0.337 | 0.23 | 4050 | 0.208 |
| input.mouse.click | 0.143 | 0.215 | 0.23 | 7003 | 0.098 | 0.145 | 0.218 | 0.22 | 6882 | 0.097 |
| input.key.type | 0.121 | 0.240 | 0.29 | 8278 | 0.071 | 0.124 | 0.211 | 0.28 | 8078 | 0.074 |
| wv.open+navigate+close | 7.160 | 8.724 | 0.12 | 140 | 9.772 | 6.814 | 13.452 | 0.12 | 147 | 9.333 |
| wv.navigate.simple | 2.116 | 3.685 | 0.17 | 473 | 3.248 | 2.086 | 3.071 | 0.16 | 480 | 3.049 |
| wv.navigate.scripts | 5.498 | 12.424 | 0.20 | 182 | 5.979 | 5.554 | 7.943 | 0.26 | 180 | 5.857 |
| wv.navigate.dom2k | 7.932 | 12.654 | 0.09 | 126 | 9.083 | 7.714 | 13.380 | 0.06 | 130 | 8.891 |
| wv.evaluate | 0.099 | 0.170 | 0.40 | 10132 | 0.072 | 0.097 | 0.175 | 0.40 | 10299 | 0.075 |
| wv.evaluate.query2k | 0.363 | 0.718 | 0.17 | 2751 | 0.326 | 0.364 | 0.958 | 0.20 | 2747 | 0.329 |
| wv.cdp.Runtime.evaluate | 0.062 | 0.154 | 0.51 | 16077 | 0.033 | 0.063 | 0.112 | 0.45 | 15848 | 0.037 |
| wv.click.selector | 17.730 | 19.846 | 0.01 | 56 | 0.674 | 17.738 | 18.757 | 0.01 | 56 | 0.658 |
| wv.type | 0.113 | 0.168 | 0.30 | 8842 | 0.045 | 0.113 | 0.161 | 0.20 | 8881 | 0.042 |
| wv.flow.e2e | 47.208 | 52.511 | 0.03 | 21 | 13.172 | 47.173 | 57.743 | 0.03 | 21 | 12.819 |

## Phase breakdown (median ms since the step started)

- page.navigate.about:blank [a]: navigateReply 1.497, frameNavigated 1.509, domContentEventFired 1.516, loadEventFired 1.520
- page.navigate.about:blank [b]: navigateReply 1.513, frameNavigated 1.528, domContentEventFired 1.534, loadEventFired 1.536
- page.navigate.simple [a]: navigateReply 1.808, frameNavigated 1.821, domContentEventFired 1.841, loadEventFired 1.843
- page.navigate.simple [b]: navigateReply 1.789, frameNavigated 1.803, domContentEventFired 1.809, loadEventFired 1.813
- page.navigate.scripts [a]: navigateReply 1.824, frameNavigated 1.836, domContentEventFired 5.027, loadEventFired 5.030
- page.navigate.scripts [b]: navigateReply 1.820, frameNavigated 1.834, domContentEventFired 5.081, loadEventFired 5.084
- page.navigate.dom2k [a]: navigateReply 2.795, frameNavigated 2.813, domContentEventFired 8.089, loadEventFired 8.092
- page.navigate.dom2k [b]: navigateReply 2.679, frameNavigated 2.697, domContentEventFired 7.885, loadEventFired 7.888
- target.lifecycle [a]: createBrowserContext 0.138, createTarget 1.626, attachToTarget 1.767, enable 2.113, closeTarget 2.316, disposeBrowserContext 2.486
- target.lifecycle [b]: createBrowserContext 0.142, createTarget 1.654, attachToTarget 1.819, enable 2.199, closeTarget 2.404, disposeBrowserContext 2.573
- wv.flow.e2e [a]: navigate 7.922, input 44.458, result 47.178, close 47.203
- wv.flow.e2e [b]: navigate 7.892, input 44.216, result 47.144, close 47.170
