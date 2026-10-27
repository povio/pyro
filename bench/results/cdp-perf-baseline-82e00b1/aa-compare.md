# CDP performance: a → b

Base: a, head: b. Threshold +10.0%, alpha 0.05.

| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |
|---|---|---:|---:|---:|---:|---:|---:|
| input.key.type | unclear | 0.112 | 0.132 | +18.3% | 0.5476 | 5 | 0.071 → 0.074 (+3.9%) |
| rtt.Browser.getVersion | unclear | 0.058 | 0.049 | -16.7% | 0.3095 | 5 | 0.028 → 0.026 (-6.5%) |
| wv.evaluate | unclear | 0.113 | 0.089 | -21.2% | 0.8340 | 5 | 0.072 → 0.075 (+4.3%) |
| rtt.Runtime.evaluate | same | 0.052 | 0.057 | +8.8% | 0.6905 | 5 | 0.032 → 0.034 (+7.2%) |
| runtime.handle+release | same | 0.115 | 0.124 | +8.6% | 0.5476 | 5 | 0.069 → 0.074 (+7.8%) |
| throughput.evaluate.pipelined256 | same | 1.745 | 1.838 | +5.3% | 0.6905 | 5 | 0.011 → 0.012 (+0.9%) |
| runtime.callFunctionOn.small | same | 0.056 | 0.058 | +3.2% | 0.6905 | 5 | 0.037 → 0.037 (-0.5%) |
| wv.type | same | 0.110 | 0.112 | +2.3% | 1.0000 | 5 | 0.045 → 0.042 (-6.0%) |
| network.fetch20 | same | 1.226 | 1.252 | +2.1% | 0.6905 | 5 | 2.310 → 2.304 (-0.2%) |
| connect.ws+getVersion | same | 1.047 | 1.061 | +1.3% | 0.5476 | 5 | 1.121 → 1.133 (+1.0%) |
| input.mouse.click | same | 0.148 | 0.149 | +1.2% | 0.5476 | 5 | 0.098 → 0.097 (-0.5%) |
| page.navigate.about:blank | same | 1.545 | 1.563 | +1.1% | 0.5476 | 5 | 2.561 → 2.565 (+0.1%) |
| target.lifecycle | same | 2.580 | 2.600 | +0.8% | 1.0000 | 5 | 3.008 → 3.043 (+1.2%) |
| wv.flow.e2e | same | 47.043 | 47.305 | +0.6% | 1.0000 | 5 | 13.172 → 12.819 (-2.7%) |
| runtime.evaluate.largeValue | same | 1.010 | 1.014 | +0.3% | 1.0000 | 5 | 0.918 → 0.929 (+1.3%) |
| wv.click.selector | same | 17.704 | 17.746 | +0.2% | 0.5476 | 5 | 0.674 → 0.658 (-2.4%) |
| dom.mutate.1k | same | 3.560 | 3.554 | -0.2% | 0.8413 | 5 | 3.446 → 3.449 (+0.1%) |
| wv.cdp.Runtime.evaluate | same | 0.050 | 0.050 | -0.2% | 0.6905 | 5 | 0.033 → 0.037 (+11.1%) |
| page.navigate.simple | same | 1.853 | 1.847 | -0.3% | 0.6905 | 5 | 2.567 → 2.583 (+0.6%) |
| fetch.intercept.fulfill | same | 0.238 | 0.236 | -0.7% | 0.8413 | 5 | 0.209 → 0.208 (-0.3%) |
| page.navigate.scripts | same | 5.256 | 5.218 | -0.7% | 1.0000 | 5 | 5.840 → 5.692 (-2.5%) |
| dom.querySelectorAll.2k | same | 0.345 | 0.337 | -2.2% | 0.3095 | 5 | 0.313 → 0.311 (-0.7%) |
| wv.evaluate.query2k | same | 0.353 | 0.344 | -2.4% | 1.0000 | 5 | 0.326 → 0.329 (+1.0%) |
| wv.navigate.dom2k | same | 7.911 | 7.709 | -2.5% | 0.6905 | 5 | 9.083 → 8.891 (-2.1%) |
| wv.navigate.scripts | same | 5.395 | 5.253 | -2.6% | 0.8413 | 5 | 5.979 → 5.857 (-2.0%) |
| page.navigate.dom2k | same | 8.094 | 7.832 | -3.2% | 1.0000 | 5 | 8.963 → 8.664 (-3.3%) |
| wv.navigate.simple | same | 2.291 | 2.209 | -3.6% | 0.3095 | 5 | 3.248 → 3.049 (-6.1%) |
| dom.textContent.2k | same | 0.168 | 0.162 | -4.0% | 0.1508 | 5 | 0.161 → 0.157 (-2.4%) |
| runtime.callFunctionOn.38KB | same | 0.307 | 0.290 | -5.8% | 0.2222 | 5 | 0.250 → 0.238 (-4.7%) |
| dom.handle.describe+quads | same | 0.302 | 0.280 | -7.2% | 0.4206 | 5 | 0.185 → 0.183 (-1.1%) |
| wv.open+navigate+close | same | 7.274 | 6.724 | -7.5% | 0.8413 | 5 | 9.772 → 9.333 (-4.5%) |

0 regression(s), 0 improvement(s), 3 unclear, 28 unchanged, 0 failed.
