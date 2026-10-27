# CDP performance: base → head

Base: base, head: head. Threshold +10.0%, alpha 0.05.

| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |
|---|---|---:|---:|---:|---:|---:|---:|
| connect.ws+getVersion | **REGRESSION** | 1.083 | 1.494 | +38.0% | 0.0079 | 5 | 1.140 → 3.163 (+177.5%) |
| fetch.intercept.fulfill | unclear | 0.234 | 0.205 | -12.3% | 0.3095 | 5 | 0.204 → 0.153 (-25.1%) |
| wv.type | unclear | 0.119 | 0.095 | -20.7% | 0.3095 | 5 | 0.042 → 0.028 (-32.8%) |
| rtt.Browser.getVersion | unclear | 0.068 | 0.035 | -49.0% | 0.1508 | 5 | 0.027 → 0.013 (-51.9%) |
| page.navigate.simple | improvement | 1.929 | 1.595 | -17.3% | 0.0079 | 5 | 2.662 → 2.606 (-2.1%) |
| input.mouse.click | improvement | 0.139 | 0.110 | -21.2% | 0.0159 | 5 | 0.095 → 0.065 (-31.1%) |
| input.key.type | improvement | 0.110 | 0.085 | -23.5% | 0.0079 | 5 | 0.068 → 0.041 (-40.0%) |
| dom.mutate.1k | improvement | 3.545 | 2.701 | -23.8% | 0.0079 | 5 | 3.449 → 2.622 (-24.0%) |
| rtt.Runtime.evaluate | improvement | 0.052 | 0.039 | -24.6% | 0.0088 | 5 | 0.031 → 0.017 (-45.3%) |
| wv.evaluate | improvement | 0.100 | 0.073 | -27.4% | 0.0119 | 5 | 0.068 → 0.057 (-16.6%) |
| runtime.handle+release | improvement | 0.114 | 0.083 | -27.5% | 0.0079 | 5 | 0.069 → 0.038 (-44.3%) |
| runtime.callFunctionOn.small | improvement | 0.057 | 0.040 | -30.3% | 0.0079 | 5 | 0.035 → 0.017 (-50.1%) |
| throughput.evaluate.pipelined256 | improvement | 1.748 | 1.193 | -31.7% | 0.0079 | 5 | 0.011 → 0.004 (-64.9%) |
| dom.handle.describe+quads | improvement | 0.310 | 0.210 | -32.1% | 0.0079 | 5 | 0.186 → 0.106 (-43.3%) |
| wv.open+navigate+close | improvement | 6.955 | 4.305 | -38.1% | 0.0079 | 5 | 9.284 → 10.181 (+9.7%) |
| wv.cdp.Runtime.evaluate | improvement | 0.062 | 0.037 | -39.7% | 0.0079 | 5 | 0.035 → 0.018 (-48.2%) |
| runtime.callFunctionOn.38KB | improvement | 0.303 | 0.160 | -47.3% | 0.0079 | 5 | 0.244 → 0.087 (-64.5%) |
| runtime.evaluate.largeValue | improvement | 1.033 | 0.519 | -49.7% | 0.0079 | 5 | 0.933 → 0.452 (-51.6%) |
| wv.flow.e2e | improvement | 47.485 | 11.901 | -74.9% | 0.0079 | 5 | 13.400 → 12.266 (-8.5%) |
| wv.click.selector | improvement | 17.696 | 1.497 | -91.5% | 0.0079 | 5 | 0.617 → 0.247 (-59.9%) |
| wv.navigate.dom2k | same | 7.742 | 7.986 | +3.2% | 0.3095 | 5 | 8.931 → 9.092 (+1.8%) |
| wv.evaluate.query2k | same | 0.357 | 0.367 | +2.9% | 1.0000 | 5 | 0.346 → 0.321 (-7.1%) |
| page.navigate.dom2k | same | 7.465 | 7.555 | +1.2% | 0.8413 | 5 | 8.319 → 8.541 (+2.7%) |
| wv.navigate.scripts | same | 5.340 | 5.394 | +1.0% | 1.0000 | 5 | 5.946 → 6.021 (+1.3%) |
| page.navigate.scripts | same | 4.831 | 4.804 | -0.6% | 0.8413 | 5 | 5.387 → 5.383 (-0.1%) |
| dom.querySelectorAll.2k | same | 0.324 | 0.318 | -1.9% | 0.4206 | 5 | 0.301 → 0.286 (-5.1%) |
| wv.navigate.simple | same | 2.062 | 2.021 | -2.0% | 0.3095 | 5 | 2.920 → 3.044 (+4.2%) |
| target.lifecycle | same | 2.520 | 2.434 | -3.4% | 0.2222 | 5 | 2.956 → 2.809 (-5.0%) |
| page.navigate.about:blank | same | 1.567 | 1.495 | -4.6% | 0.2222 | 5 | 2.549 → 2.406 (-5.6%) |
| network.fetch20 | same | 1.123 | 1.035 | -7.9% | 1.0000 | 5 | 2.175 → 1.691 (-22.3%) |
| dom.textContent.2k | same | 0.161 | 0.147 | -8.2% | 0.0952 | 5 | 0.151 → 0.150 (-0.7%) |

1 regression(s), 16 improvement(s), 3 unclear, 11 unchanged, 0 failed.
