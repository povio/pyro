# CDP performance: base → head

Base: base, head: head. Threshold +10.0%, alpha 0.05.

| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |
|---|---|---:|---:|---:|---:|---:|---:|
| wv.evaluate | unclear | 0.110 | 0.071 | -35.4% | 0.3095 | 5 | 0.060 → 0.060 (-0.3%) |
| wv.cdp.Runtime.evaluate | unclear | 0.072 | 0.041 | -42.7% | 0.3095 | 5 | 0.021 → 0.019 (-7.3%) |
| network.fetch20 | improvement | 1.174 | 0.973 | -17.2% | 0.0159 | 5 | 1.760 → 1.533 (-12.9%) |
| throughput.evaluate.pipelined256 | improvement | 1.809 | 1.241 | -31.4% | 0.0079 | 5 | 0.007 → 0.004 (-41.4%) |
| dom.textContent.2k | same | 0.155 | 0.165 | +6.3% | 0.0952 | 5 | 0.157 → 0.158 (+0.4%) |
| connect.ws+getVersion | same | 1.361 | 1.433 | +5.3% | 1.0000 | 5 | 3.053 → 3.038 (-0.5%) |
| wv.evaluate.query2k | same | 0.339 | 0.355 | +4.9% | 0.3095 | 5 | 0.316 → 0.317 (+0.4%) |
| target.lifecycle | same | 2.425 | 2.530 | +4.3% | 0.2222 | 5 | 2.803 → 2.902 (+3.5%) |
| page.navigate.simple | same | 1.666 | 1.728 | +3.8% | 0.5476 | 5 | 2.711 → 2.814 (+3.8%) |
| input.mouse.click | same | 0.110 | 0.114 | +3.3% | 0.1161 | 5 | 0.066 → 0.068 (+3.3%) |
| page.navigate.about:blank | same | 1.587 | 1.635 | +3.0% | 1.0000 | 5 | 2.530 → 2.551 (+0.8%) |
| fetch.intercept.fulfill | same | 0.213 | 0.218 | +2.2% | 0.8413 | 5 | 0.167 → 0.166 (-0.5%) |
| rtt.Runtime.evaluate | same | 0.040 | 0.040 | +1.8% | 0.5476 | 5 | 0.017 → 0.017 (+0.0%) |
| rtt.Browser.getVersion | same | 0.045 | 0.046 | +1.8% | 1.0000 | 5 | 0.015 → 0.015 (+0.7%) |
| wv.navigate.scripts | same | 5.192 | 5.257 | +1.2% | 0.8413 | 5 | 6.085 → 6.009 (-1.3%) |
| runtime.callFunctionOn.small | same | 0.044 | 0.044 | +1.1% | 0.6905 | 5 | 0.019 → 0.019 (-1.0%) |
| dom.querySelectorAll.2k | same | 0.318 | 0.321 | +0.9% | 1.0000 | 5 | 0.290 → 0.291 (+0.3%) |
| wv.open+navigate+close | same | 4.415 | 4.451 | +0.8% | 0.8413 | 5 | 10.157 → 10.253 (+0.9%) |
| wv.navigate.dom2k | same | 7.948 | 7.967 | +0.2% | 0.8413 | 5 | 8.920 → 9.080 (+1.8%) |
| wv.click.selector | same | 1.535 | 1.535 | +0.1% | 0.5476 | 5 | 0.285 → 0.259 (-9.1%) |
| dom.mutate.1k | same | 2.606 | 2.595 | -0.4% | 0.8413 | 5 | 2.587 → 2.535 (-2.0%) |
| input.key.type | same | 0.092 | 0.091 | -0.5% | 1.0000 | 5 | 0.043 → 0.043 (-0.5%) |
| page.navigate.scripts | same | 5.258 | 5.205 | -1.0% | 0.3095 | 5 | 5.920 → 5.825 (-1.6%) |
| wv.flow.e2e | same | 12.178 | 11.991 | -1.5% | 0.6905 | 5 | 12.415 → 12.350 (-0.5%) |
| runtime.handle+release | same | 0.088 | 0.087 | -1.6% | 0.8413 | 5 | 0.038 → 0.039 (+1.0%) |
| page.navigate.dom2k | same | 7.521 | 7.377 | -1.9% | 0.8413 | 5 | 8.586 → 8.483 (-1.2%) |
| runtime.callFunctionOn.38KB | same | 0.158 | 0.153 | -3.0% | 0.8413 | 5 | 0.086 → 0.086 (+0.0%) |
| runtime.evaluate.largeValue | same | 0.549 | 0.528 | -3.7% | 1.0000 | 5 | 0.460 → 0.461 (+0.2%) |
| wv.type | same | 0.113 | 0.109 | -4.0% | 0.6752 | 5 | 0.029 → 0.027 (-4.2%) |
| wv.navigate.simple | same | 1.974 | 1.882 | -4.6% | 0.8413 | 5 | 3.048 → 2.941 (-3.5%) |
| dom.handle.describe+quads | same | 0.244 | 0.228 | -6.6% | 1.0000 | 5 | 0.120 → 0.120 (+0.2%) |

0 regression(s), 2 improvement(s), 2 unclear, 27 unchanged, 0 failed.
