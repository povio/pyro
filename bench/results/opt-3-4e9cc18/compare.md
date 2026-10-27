# CDP performance: base → head

Base: base, head: head. Threshold +10.0%, alpha 0.05.

| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |
|---|---|---:|---:|---:|---:|---:|---:|
| wv.navigate.scripts | unclear | 7.389 | 19.408 | +162.7% | 0.3095 | 5 | 8.075 → 9.613 (+19.0%) |
| network.fetch20 | unclear | 2.046 | 2.780 | +35.9% | 0.4206 | 5 | 2.478 → 2.582 (+4.2%) |
| wv.cdp.Runtime.evaluate | unclear | 0.067 | 0.079 | +18.3% | 1.0000 | 5 | 0.038 → 0.039 (+2.1%) |
| page.navigate.scripts | unclear | 7.291 | 8.195 | +12.4% | 0.4206 | 5 | 7.273 → 7.457 (+2.5%) |
| input.mouse.click | unclear | 0.152 | 0.170 | +11.9% | 0.5476 | 5 | 0.120 → 0.118 (-2.1%) |
| rtt.Runtime.evaluate | unclear | 0.057 | 0.064 | +11.4% | 0.0952 | 5 | 0.037 → 0.040 (+7.2%) |
| rtt.Browser.getVersion | unclear | 0.066 | 0.059 | -10.7% | 0.5476 | 5 | 0.035 → 0.033 (-6.6%) |
| target.lifecycle | unclear | 4.471 | 3.751 | -16.1% | 0.5476 | 5 | 3.860 → 3.968 (+2.8%) |
| dom.textContent.2k | unclear | 0.238 | 0.192 | -19.4% | 0.5476 | 5 | 0.176 → 0.182 (+3.1%) |
| fetch.intercept.fulfill | unclear | 0.327 | 0.258 | -21.3% | 0.0952 | 5 | 0.308 → 0.242 (-21.5%) |
| wv.evaluate | unclear | 0.148 | 0.092 | -37.7% | 0.0556 | 5 | 0.097 → 0.079 (-19.3%) |
| input.key.type | unclear | 0.432 | 0.204 | -52.6% | 1.0000 | 5 | 0.115 → 0.100 (-13.0%) |
| dom.mutate.1k | improvement | 4.041 | 2.983 | -26.2% | 0.0079 | 5 | 3.760 → 2.808 (-25.3%) |
| wv.evaluate.query2k | same | 0.459 | 0.498 | +8.5% | 0.5476 | 5 | 0.400 → 0.439 (+9.8%) |
| wv.flow.e2e | same | 15.858 | 17.121 | +8.0% | 1.0000 | 5 | 13.846 → 14.410 (+4.1%) |
| runtime.callFunctionOn.38KB | same | 0.190 | 0.205 | +7.8% | 0.4206 | 5 | 0.117 → 0.119 (+2.0%) |
| runtime.evaluate.largeValue | same | 1.099 | 1.176 | +7.0% | 0.6905 | 5 | 1.058 → 1.072 (+1.3%) |
| connect.ws+getVersion | same | 1.893 | 2.002 | +5.8% | 0.8413 | 5 | 3.583 → 3.632 (+1.4%) |
| wv.navigate.dom2k | same | 10.533 | 11.034 | +4.8% | 1.0000 | 5 | 10.713 → 10.682 (-0.3%) |
| wv.open+navigate+close | same | 5.708 | 5.966 | +4.5% | 1.0000 | 5 | 10.722 → 11.189 (+4.4%) |
| wv.type | same | 0.121 | 0.120 | -0.3% | 1.0000 | 5 | 0.047 → 0.048 (+0.8%) |
| page.navigate.simple | same | 1.970 | 1.952 | -1.0% | 0.6905 | 5 | 3.212 → 3.129 (-2.6%) |
| dom.querySelectorAll.2k | same | 0.388 | 0.383 | -1.1% | 1.0000 | 5 | 0.353 → 0.348 (-1.4%) |
| runtime.handle+release | same | 0.146 | 0.142 | -2.5% | 0.8413 | 5 | 0.085 → 0.085 (-0.4%) |
| wv.click.selector | same | 1.721 | 1.670 | -2.9% | 1.0000 | 5 | 0.425 → 0.418 (-1.6%) |
| wv.navigate.simple | same | 2.599 | 2.506 | -3.6% | 0.6905 | 5 | 3.674 → 3.647 (-0.7%) |
| throughput.evaluate.pipelined256 | same | 2.221 | 2.129 | -4.2% | 1.0000 | 5 | 0.010 → 0.010 (+0.0%) |
| runtime.callFunctionOn.small | same | 0.069 | 0.065 | -5.2% | 0.8413 | 5 | 0.040 → 0.040 (-2.0%) |
| page.navigate.dom2k | same | 9.749 | 9.213 | -5.5% | 0.8413 | 5 | 9.858 → 10.215 (+3.6%) |
| dom.handle.describe+quads | same | 0.371 | 0.348 | -6.3% | 1.0000 | 5 | 0.201 → 0.208 (+3.2%) |
| page.navigate.about:blank | same | 2.475 | 2.265 | -8.5% | 0.3095 | 5 | 3.239 → 3.040 (-6.1%) |

0 regression(s), 1 improvement(s), 12 unclear, 18 unchanged, 0 failed.
