# CDP performance: base → head

Base: base, head: head. Threshold +10.0%, alpha 0.05.

| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |
|---|---|---:|---:|---:|---:|---:|---:|
| throughput.evaluate.pipelined256 | unclear | 1.378 | 1.189 | -13.7% | 0.3095 | 5 | 0.009 → 0.009 (-1.1%) |
| connect.ws+getVersion | unclear | 0.954 | 0.802 | -16.0% | 0.5476 | 5 | 1.069 → 0.899 (-15.8%) |
| wv.evaluate | unclear | 0.088 | 0.071 | -18.6% | 0.6905 | 5 | 0.058 → 0.060 (+4.1%) |
| wv.cdp.Runtime.evaluate | unclear | 0.098 | 0.046 | -52.7% | 0.2492 | 5 | 0.030 → 0.028 (-8.6%) |
| wv.flow.e2e | improvement | 52.937 | 24.095 | -54.5% | 0.0079 | 5 | 17.725 → 11.151 (-37.1%) |
| wv.click.selector | improvement | 19.464 | 7.066 | -63.7% | 0.0079 | 5 | 1.457 → 0.793 (-45.6%) |
| wv.navigate.simple | same | 1.620 | 1.691 | +4.4% | 0.8413 | 5 | 2.328 → 2.341 (+0.6%) |
| runtime.evaluate.largeValue | same | 0.827 | 0.853 | +3.2% | 1.0000 | 5 | 0.758 → 0.760 (+0.3%) |
| dom.handle.describe+quads | same | 0.224 | 0.229 | +2.4% | 0.6905 | 5 | 0.148 → 0.145 (-2.4%) |
| input.key.type | same | 0.092 | 0.094 | +2.3% | 0.8413 | 5 | 0.057 → 0.056 (-0.9%) |
| fetch.intercept.fulfill | same | 0.180 | 0.183 | +1.9% | 1.0000 | 5 | 0.153 → 0.154 (+1.0%) |
| dom.textContent.2k | same | 0.175 | 0.178 | +1.8% | 0.9166 | 5 | 0.145 → 0.145 (+0.1%) |
| wv.navigate.scripts | same | 4.108 | 4.171 | +1.5% | 0.6905 | 5 | 4.510 → 4.443 (-1.5%) |
| wv.navigate.dom2k | same | 6.790 | 6.888 | +1.4% | 0.6905 | 5 | 7.773 → 7.851 (+1.0%) |
| page.navigate.simple | same | 1.427 | 1.447 | +1.4% | 1.0000 | 5 | 2.130 → 2.094 (-1.7%) |
| page.navigate.dom2k | same | 6.940 | 7.016 | +1.1% | 0.6905 | 5 | 7.736 → 7.782 (+0.6%) |
| input.mouse.click | same | 0.116 | 0.117 | +0.9% | 0.6004 | 5 | 0.080 → 0.079 (-1.5%) |
| page.navigate.scripts | same | 4.045 | 4.075 | +0.7% | 0.6905 | 5 | 4.493 → 4.368 (-2.8%) |
| network.fetch20 | same | 1.077 | 1.084 | +0.7% | 0.8413 | 5 | 1.887 → 1.841 (-2.4%) |
| dom.mutate.1k | same | 3.021 | 3.031 | +0.3% | 1.0000 | 5 | 3.045 → 2.996 (-1.6%) |
| rtt.Browser.getVersion | same | 0.042 | 0.042 | +0.0% | 0.8340 | 5 | 0.022 → 0.021 (-4.1%) |
| runtime.handle+release | same | 0.096 | 0.096 | -0.1% | 0.8340 | 5 | 0.059 → 0.059 (+0.3%) |
| wv.open+navigate+close | same | 5.680 | 5.651 | -0.5% | 1.0000 | 5 | 7.592 → 7.528 (-0.8%) |
| rtt.Runtime.evaluate | same | 0.044 | 0.044 | -0.7% | 0.4620 | 5 | 0.025 → 0.024 (-2.0%) |
| target.lifecycle | same | 2.139 | 2.116 | -1.1% | 0.4206 | 5 | 2.445 → 2.247 (-8.1%) |
| page.navigate.about:blank | same | 1.227 | 1.196 | -2.5% | 0.5476 | 5 | 2.030 → 2.025 (-0.3%) |
| runtime.callFunctionOn.38KB | same | 0.276 | 0.266 | -3.6% | 0.5476 | 5 | 0.228 → 0.217 (-4.8%) |
| wv.type | same | 0.114 | 0.108 | -5.5% | 0.4206 | 5 | 0.040 → 0.040 (+0.8%) |
| dom.querySelectorAll.2k | same | 0.322 | 0.297 | -7.8% | 0.8413 | 5 | 0.277 → 0.278 (+0.4%) |
| runtime.callFunctionOn.small | same | 0.053 | 0.048 | -8.5% | 1.0000 | 5 | 0.031 → 0.032 (+4.6%) |
| wv.evaluate.query2k | same | 0.373 | 0.341 | -8.7% | 0.8413 | 5 | 0.312 → 0.298 (-4.4%) |

0 regression(s), 2 improvement(s), 4 unclear, 25 unchanged, 0 failed.
