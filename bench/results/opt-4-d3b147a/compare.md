# CDP performance: base → head

Base: base, head: head. Threshold +10.0%, alpha 0.05.

| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |
|---|---|---:|---:|---:|---:|---:|---:|
| wv.cdp.Runtime.evaluate | unclear | 0.055 | 0.061 | +10.7% | 0.2222 | 5 | 0.034 → 0.036 (+4.7%) |
| wv.type | unclear | 0.122 | 0.103 | -15.5% | 0.0556 | 5 | 0.046 → 0.045 (-2.0%) |
| rtt.Browser.getVersion | unclear | 0.073 | 0.058 | -21.2% | 0.4206 | 5 | 0.031 → 0.030 (-1.3%) |
| wv.evaluate | unclear | 0.120 | 0.092 | -23.9% | 0.4206 | 5 | 0.079 → 0.077 (-2.3%) |
| wv.flow.e2e | same | 12.926 | 14.109 | +9.1% | 0.0952 | 5 | 13.146 → 13.765 (+4.7%) |
| wv.open+navigate+close | same | 4.793 | 5.175 | +8.0% | 0.0952 | 5 | 10.398 → 11.148 (+7.2%) |
| runtime.callFunctionOn.small | same | 0.055 | 0.059 | +7.5% | 0.6905 | 5 | 0.033 → 0.035 (+5.5%) |
| throughput.evaluate.pipelined256 | same | 1.805 | 1.936 | +7.2% | 0.3095 | 5 | 0.010 → 0.010 (+3.1%) |
| connect.ws+getVersion | same | 1.528 | 1.621 | +6.1% | 0.0952 | 5 | 3.402 → 3.551 (+4.4%) |
| dom.textContent.2k | same | 0.187 | 0.199 | +6.0% | 0.3095 | 5 | 0.166 → 0.178 (+7.4%) |
| page.navigate.scripts | same | 5.771 | 6.022 | +4.3% | 0.4206 | 5 | 6.543 → 6.414 (-2.0%) |
| wv.navigate.scripts | same | 5.971 | 6.180 | +3.5% | 0.8413 | 5 | 7.082 → 7.005 (-1.1%) |
| runtime.handle+release | same | 0.128 | 0.132 | +3.1% | 0.6905 | 5 | 0.074 → 0.074 (+1.0%) |
| wv.click.selector | same | 1.550 | 1.591 | +2.6% | 0.4206 | 5 | 0.369 → 0.358 (-3.0%) |
| input.mouse.click | same | 0.164 | 0.166 | +1.0% | 0.6905 | 5 | 0.113 → 0.110 (-2.8%) |
| wv.navigate.dom2k | same | 8.441 | 8.479 | +0.4% | 0.2222 | 5 | 9.527 → 9.542 (+0.2%) |
| dom.handle.describe+quads | same | 0.303 | 0.303 | +0.1% | 1.0000 | 5 | 0.183 → 0.185 (+1.5%) |
| wv.evaluate.query2k | same | 0.400 | 0.400 | -0.1% | 0.8413 | 5 | 0.380 → 0.381 (+0.2%) |
| dom.mutate.1k | same | 2.905 | 2.899 | -0.2% | 1.0000 | 5 | 2.763 → 2.697 (-2.4%) |
| network.fetch20 | same | 1.294 | 1.283 | -0.8% | 0.8413 | 5 | 2.281 → 2.290 (+0.4%) |
| runtime.callFunctionOn.38KB | same | 0.183 | 0.180 | -1.7% | 0.4206 | 5 | 0.111 → 0.108 (-2.5%) |
| wv.navigate.simple | same | 2.213 | 2.156 | -2.6% | 0.4206 | 5 | 3.501 → 3.459 (-1.2%) |
| rtt.Runtime.evaluate | same | 0.063 | 0.061 | -2.7% | 0.8413 | 5 | 0.035 → 0.035 (+1.1%) |
| page.navigate.dom2k | same | 8.003 | 7.773 | -2.9% | 0.0952 | 5 | 9.101 → 8.932 (-1.8%) |
| dom.querySelectorAll.2k | same | 0.372 | 0.360 | -3.3% | 0.2222 | 5 | 0.326 → 0.323 (-0.9%) |
| page.navigate.simple | same | 1.857 | 1.795 | -3.3% | 0.5476 | 5 | 3.125 → 2.869 (-8.2%) |
| input.key.type | same | 0.130 | 0.125 | -4.0% | 0.0952 | 5 | 0.074 → 0.073 (-2.3%) |
| target.lifecycle | same | 3.051 | 2.915 | -4.5% | 0.0952 | 5 | 3.454 → 3.263 (-5.5%) |
| page.navigate.about:blank | same | 1.926 | 1.813 | -5.9% | 0.4206 | 5 | 3.034 → 2.823 (-7.0%) |
| runtime.evaluate.largeValue | same | 1.158 | 1.081 | -6.6% | 0.3095 | 5 | 1.050 → 1.007 (-4.1%) |
| fetch.intercept.fulfill | same | 0.280 | 0.261 | -6.7% | 0.2222 | 5 | 0.235 → 0.220 (-6.4%) |

0 regression(s), 0 improvement(s), 4 unclear, 27 unchanged, 0 failed.
