# CDP performance: base → head

Base: base, head: head. Threshold +10.0%, alpha 0.05.

| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |
|---|---|---:|---:|---:|---:|---:|---:|
| page.navigate.simple | unclear | 1.742 | 1.577 | -9.5% | 0.4206 | 5 | 3.056 → 2.567 (-16.0%) |
| fetch.intercept.fulfill | unclear | 0.221 | 0.195 | -11.8% | 0.2222 | 5 | 0.190 → 0.154 (-18.9%) |
| wv.evaluate.query2k | unclear | 0.386 | 0.341 | -11.8% | 0.0952 | 5 | 0.354 → 0.319 (-9.9%) |
| rtt.Runtime.evaluate | unclear | 0.049 | 0.041 | -16.1% | 0.0556 | 5 | 0.028 → 0.018 (-34.8%) |
| input.mouse.click | unclear | 0.127 | 0.103 | -18.7% | 0.0556 | 5 | 0.090 → 0.063 (-29.8%) |
| runtime.handle+release | unclear | 0.116 | 0.090 | -22.3% | 0.0952 | 5 | 0.066 → 0.041 (-38.0%) |
| runtime.callFunctionOn.small | unclear | 0.049 | 0.038 | -22.4% | 0.0556 | 5 | 0.029 → 0.016 (-44.7%) |
| wv.cdp.Runtime.evaluate | unclear | 0.059 | 0.038 | -36.4% | 0.1508 | 5 | 0.029 → 0.017 (-41.8%) |
| rtt.Browser.getVersion | unclear | 0.061 | 0.035 | -41.7% | 0.4206 | 5 | 0.027 → 0.013 (-52.0%) |
| wv.evaluate | unclear | 0.104 | 0.060 | -42.5% | 0.0952 | 5 | 0.074 → 0.052 (-29.6%) |
| wv.type | improvement | 0.118 | 0.094 | -20.4% | 0.0079 | 5 | 0.042 → 0.027 (-36.4%) |
| input.key.type | improvement | 0.112 | 0.081 | -27.9% | 0.0088 | 5 | 0.065 → 0.039 (-39.8%) |
| dom.handle.describe+quads | improvement | 0.324 | 0.185 | -43.0% | 0.0317 | 5 | 0.186 → 0.105 (-43.4%) |
| throughput.evaluate.pipelined256 | same | 1.665 | 1.800 | +8.2% | 0.0159 | 5 | 0.009 → 0.007 (-19.5%) |
| network.fetch20 | same | 1.174 | 1.202 | +2.4% | 0.8413 | 5 | 2.154 → 1.750 (-18.8%) |
| wv.open+navigate+close | same | 4.305 | 4.386 | +1.9% | 0.1508 | 5 | 9.874 → 9.967 (+0.9%) |
| page.navigate.scripts | same | 5.006 | 5.082 | +1.5% | 0.8413 | 5 | 5.875 → 5.698 (-3.0%) |
| page.navigate.dom2k | same | 7.437 | 7.424 | -0.2% | 0.8413 | 5 | 8.550 → 8.552 (+0.0%) |
| wv.navigate.dom2k | same | 7.941 | 7.897 | -0.6% | 1.0000 | 5 | 9.046 → 8.921 (-1.4%) |
| wv.navigate.simple | same | 1.967 | 1.952 | -0.8% | 0.6905 | 5 | 3.140 → 3.132 (-0.2%) |
| wv.navigate.scripts | same | 5.179 | 5.112 | -1.3% | 0.6905 | 5 | 5.934 → 5.884 (-0.8%) |
| page.navigate.about:blank | same | 1.670 | 1.639 | -1.8% | 0.6905 | 5 | 2.636 → 2.603 (-1.3%) |
| wv.click.selector | same | 1.572 | 1.533 | -2.5% | 0.6905 | 5 | 0.324 → 0.267 (-17.6%) |
| wv.flow.e2e | same | 12.065 | 11.700 | -3.0% | 0.6905 | 5 | 12.458 → 11.996 (-3.7%) |
| dom.mutate.1k | same | 2.729 | 2.625 | -3.8% | 0.2222 | 5 | 2.617 → 2.568 (-1.9%) |
| runtime.evaluate.largeValue | same | 0.536 | 0.513 | -4.3% | 0.4206 | 5 | 0.460 → 0.452 (-1.9%) |
| connect.ws+getVersion | same | 1.405 | 1.342 | -4.5% | 0.8413 | 5 | 3.212 → 3.180 (-1.0%) |
| dom.textContent.2k | same | 0.157 | 0.148 | -5.8% | 0.1508 | 5 | 0.170 → 0.149 (-11.9%) |
| target.lifecycle | same | 2.567 | 2.406 | -6.2% | 0.2222 | 5 | 3.029 → 2.835 (-6.4%) |
| dom.querySelectorAll.2k | same | 0.344 | 0.319 | -7.2% | 0.0952 | 5 | 0.303 → 0.288 (-4.8%) |
| runtime.callFunctionOn.38KB | same | 0.161 | 0.149 | -7.6% | 0.0079 | 5 | 0.102 → 0.085 (-16.9%) |

0 regression(s), 3 improvement(s), 10 unclear, 18 unchanged, 0 failed.
