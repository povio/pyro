# CDP performance: base → head

Base: base, head: head. Threshold +10.0%, alpha 0.05.

| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |
|---|---|---:|---:|---:|---:|---:|---:|
| wv.open+navigate+close | unclear | 5.413 | 6.250 | +15.5% | 0.3095 | 5 | 10.985 → 11.643 (+6.0%) |
| network.fetch20 | unclear | 1.297 | 1.491 | +14.9% | 0.8413 | 5 | 2.363 → 2.385 (+0.9%) |
| wv.navigate.scripts | unclear | 6.736 | 6.081 | -9.7% | 0.4206 | 5 | 7.123 → 6.928 (-2.7%) |
| wv.type | unclear | 0.121 | 0.108 | -10.8% | 0.4206 | 5 | 0.043 → 0.043 (-0.9%) |
| wv.evaluate | unclear | 0.111 | 0.094 | -14.9% | 1.0000 | 5 | 0.086 → 0.078 (-8.5%) |
| rtt.Browser.getVersion | unclear | 0.071 | 0.056 | -20.8% | 1.0000 | 5 | 0.031 → 0.030 (-3.2%) |
| connect.ws+getVersion | same | 1.510 | 1.629 | +7.9% | 0.6905 | 5 | 3.402 → 3.308 (-2.8%) |
| rtt.Runtime.evaluate | same | 0.057 | 0.060 | +5.7% | 0.6905 | 5 | 0.033 → 0.036 (+10.0%) |
| throughput.evaluate.pipelined256 | same | 1.731 | 1.788 | +3.3% | 0.4206 | 5 | 0.009 → 0.009 (-1.1%) |
| dom.querySelectorAll.2k | same | 0.362 | 0.373 | +3.1% | 0.2222 | 5 | 0.326 → 0.340 (+4.4%) |
| input.mouse.click | same | 0.162 | 0.166 | +2.5% | 1.0000 | 5 | 0.111 → 0.110 (-0.9%) |
| dom.mutate.1k | same | 2.856 | 2.898 | +1.5% | 1.0000 | 5 | 2.757 → 2.792 (+1.3%) |
| wv.navigate.simple | same | 2.117 | 2.134 | +0.8% | 0.8413 | 5 | 3.362 → 3.453 (+2.7%) |
| runtime.handle+release | same | 0.132 | 0.133 | +0.6% | 0.5476 | 5 | 0.077 → 0.080 (+4.4%) |
| target.lifecycle | same | 2.986 | 3.003 | +0.6% | 0.6905 | 5 | 3.494 → 3.360 (-3.9%) |
| wv.click.selector | same | 1.601 | 1.602 | +0.0% | 0.6905 | 5 | 0.409 → 0.408 (-0.2%) |
| wv.cdp.Runtime.evaluate | same | 0.061 | 0.061 | -0.5% | 1.0000 | 5 | 0.035 → 0.037 (+3.7%) |
| input.key.type | same | 0.130 | 0.129 | -0.8% | 1.0000 | 5 | 0.076 → 0.078 (+2.0%) |
| wv.navigate.dom2k | same | 8.690 | 8.614 | -0.9% | 0.6905 | 5 | 9.512 → 9.567 (+0.6%) |
| runtime.evaluate.largeValue | same | 0.610 | 0.603 | -1.2% | 0.3095 | 5 | 0.524 → 0.524 (+0.0%) |
| dom.handle.describe+quads | same | 0.307 | 0.303 | -1.3% | 1.0000 | 5 | 0.194 → 0.196 (+1.0%) |
| page.navigate.scripts | same | 6.315 | 6.226 | -1.4% | 0.6905 | 5 | 6.702 → 6.693 (-0.1%) |
| dom.textContent.2k | same | 0.202 | 0.199 | -1.6% | 1.0000 | 5 | 0.182 → 0.184 (+0.7%) |
| page.navigate.about:blank | same | 1.785 | 1.726 | -3.3% | 0.5476 | 5 | 2.861 → 2.870 (+0.3%) |
| runtime.callFunctionOn.38KB | same | 0.194 | 0.187 | -4.0% | 0.1732 | 5 | 0.118 → 0.114 (-4.1%) |
| page.navigate.simple | same | 1.828 | 1.750 | -4.3% | 0.4206 | 5 | 3.055 → 3.057 (+0.1%) |
| page.navigate.dom2k | same | 8.456 | 8.079 | -4.5% | 1.0000 | 5 | 9.106 → 9.143 (+0.4%) |
| fetch.intercept.fulfill | same | 0.284 | 0.265 | -6.6% | 0.8413 | 5 | 0.239 → 0.241 (+0.7%) |
| wv.flow.e2e | same | 13.726 | 12.800 | -6.7% | 0.2222 | 5 | 13.813 → 13.150 (-4.8%) |
| runtime.callFunctionOn.small | same | 0.065 | 0.059 | -8.0% | 0.0952 | 5 | 0.038 → 0.035 (-9.9%) |
| wv.evaluate.query2k | same | 0.407 | 0.372 | -8.7% | 0.0159 | 5 | 0.397 → 0.364 (-8.2%) |

0 regression(s), 0 improvement(s), 6 unclear, 25 unchanged, 0 failed.
