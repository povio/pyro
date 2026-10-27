# CDP performance: plain → traced

Base: plain, head: traced. Threshold +10.0%, alpha 0.05.

| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |
|---|---|---:|---:|---:|---:|---:|---:|
| fetch.intercept.fulfill | unclear | 0.222 | 0.246 | +11.1% | 0.1000 | 3 | 0.187 → 0.217 (+16.1%) |
| input.mouse.click | unclear | 0.151 | 0.137 | -9.3% | 0.2000 | 3 | 0.100 → 0.094 (-6.2%) |
| wv.type | unclear | 0.108 | 0.096 | -10.9% | 0.2000 | 3 | 0.044 → 0.042 (-5.0%) |
| input.key.type | unclear | 0.121 | 0.108 | -10.9% | 0.2000 | 3 | 0.071 → 0.068 (-4.5%) |
| wv.cdp.Runtime.evaluate | unclear | 0.074 | 0.056 | -24.3% | 1.0000 | 3 | 0.033 → 0.036 (+9.4%) |
| rtt.Browser.getVersion | unclear | 0.068 | 0.050 | -25.6% | 1.0000 | 3 | 0.025 → 0.026 (+5.2%) |
| wv.evaluate | unclear | 0.112 | 0.079 | -29.4% | 0.4000 | 3 | 0.072 → 0.066 (-7.4%) |
| wv.navigate.simple | same | 1.990 | 2.175 | +9.3% | 0.1000 | 3 | 2.921 → 2.919 (-0.1%) |
| wv.evaluate.query2k | same | 0.342 | 0.370 | +8.4% | 0.7000 | 3 | 0.323 → 0.323 (-0.1%) |
| dom.textContent.2k | same | 0.163 | 0.176 | +7.6% | 0.4000 | 3 | 0.153 → 0.156 (+2.2%) |
| throughput.evaluate.pipelined256 | same | 1.744 | 1.843 | +5.7% | 0.7000 | 3 | 0.011 → 0.012 (+10.8%) |
| rtt.Runtime.evaluate | same | 0.054 | 0.056 | +4.1% | 0.7000 | 3 | 0.031 → 0.034 (+8.7%) |
| runtime.callFunctionOn.small | same | 0.058 | 0.060 | +2.7% | 1.0000 | 3 | 0.036 → 0.037 (+3.1%) |
| page.navigate.scripts | same | 5.031 | 5.112 | +1.6% | 0.7000 | 3 | 5.456 → 5.568 (+2.1%) |
| wv.flow.e2e | same | 47.940 | 48.469 | +1.1% | 0.7000 | 3 | 14.225 → 15.292 (+7.5%) |
| dom.mutate.1k | same | 3.461 | 3.496 | +1.0% | 1.0000 | 3 | 3.431 → 3.388 (-1.3%) |
| target.lifecycle | same | 2.506 | 2.521 | +0.6% | 1.0000 | 3 | 2.939 → 2.929 (-0.3%) |
| wv.navigate.dom2k | same | 7.631 | 7.650 | +0.2% | 1.0000 | 3 | 8.756 → 8.755 (-0.0%) |
| page.navigate.dom2k | same | 7.666 | 7.669 | +0.0% | 1.0000 | 3 | 8.403 → 8.441 (+0.4%) |
| wv.click.selector | same | 18.747 | 18.728 | -0.1% | 1.0000 | 3 | 1.044 → 0.982 (-6.0%) |
| runtime.evaluate.largeValue | same | 1.022 | 1.018 | -0.4% | 1.0000 | 3 | 0.914 → 0.928 (+1.6%) |
| page.navigate.about:blank | same | 1.511 | 1.502 | -0.6% | 0.4000 | 3 | 2.471 → 2.471 (-0.0%) |
| dom.querySelectorAll.2k | same | 0.331 | 0.328 | -0.8% | 0.7000 | 3 | 0.304 → 0.300 (-1.4%) |
| runtime.callFunctionOn.38KB | same | 0.291 | 0.286 | -1.9% | 1.0000 | 3 | 0.238 → 0.234 (-1.7%) |
| wv.navigate.scripts | same | 5.228 | 5.125 | -2.0% | 0.4000 | 3 | 5.706 → 5.640 (-1.1%) |
| network.fetch20 | same | 1.145 | 1.119 | -2.3% | 1.0000 | 3 | 2.138 → 2.210 (+3.4%) |
| runtime.handle+release | same | 0.119 | 0.115 | -3.5% | 0.1000 | 3 | 0.072 → 0.071 (-0.4%) |
| dom.handle.describe+quads | same | 0.295 | 0.281 | -4.4% | 1.0000 | 3 | 0.190 → 0.190 (-0.1%) |
| wv.open+navigate+close | same | 6.956 | 6.577 | -5.4% | 0.7000 | 3 | 9.284 → 9.073 (-2.3%) |
| page.navigate.simple | same | 1.942 | 1.804 | -7.1% | 0.1000 | 3 | 2.641 → 2.553 (-3.3%) |
| connect.ws+getVersion | same | 1.099 | 1.020 | -7.1% | 0.4000 | 3 | 1.241 → 1.161 (-6.4%) |

0 regression(s), 0 improvement(s), 7 unclear, 24 unchanged, 0 failed.
