# CDP performance: base → head

Base: base, head: head. Threshold +10.0%, alpha 0.05.

| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |
|---|---|---:|---:|---:|---:|---:|---:|
| connect.ws+getVersion | **REGRESSION** | 1.169 | 1.659 | +41.8% | 0.0159 | 5 | 1.236 → 3.257 (+163.4%) |
| dom.textContent.2k | unclear | 0.173 | 0.196 | +13.6% | 0.6905 | 5 | 0.162 → 0.170 (+5.3%) |
| page.navigate.about:blank | unclear | 1.566 | 1.746 | +11.5% | 0.8413 | 5 | 2.552 → 2.785 (+9.1%) |
| network.fetch20 | unclear | 1.357 | 1.227 | -9.5% | 0.6905 | 5 | 2.415 → 2.273 (-5.9%) |
| wv.navigate.scripts | unclear | 5.593 | 5.012 | -10.4% | 0.5476 | 5 | 6.432 → 6.456 (+0.4%) |
| wv.cdp.Runtime.evaluate | unclear | 0.062 | 0.055 | -12.0% | 0.5476 | 5 | 0.035 → 0.032 (-9.1%) |
| rtt.Browser.getVersion | unclear | 0.072 | 0.063 | -12.1% | 1.0000 | 5 | 0.029 → 0.031 (+4.1%) |
| runtime.callFunctionOn.small | unclear | 0.062 | 0.054 | -12.3% | 0.0952 | 5 | 0.039 → 0.031 (-20.4%) |
| wv.evaluate | unclear | 0.110 | 0.096 | -13.1% | 0.6905 | 5 | 0.077 → 0.075 (-3.4%) |
| dom.handle.describe+quads | unclear | 0.343 | 0.293 | -14.4% | 0.5476 | 5 | 0.201 → 0.172 (-14.4%) |
| page.navigate.simple | unclear | 1.903 | 1.624 | -14.7% | 0.0952 | 5 | 2.653 → 2.696 (+1.6%) |
| wv.open+navigate+close | unclear | 7.495 | 4.756 | -36.5% | 0.0952 | 5 | 9.915 → 10.080 (+1.7%) |
| wv.flow.e2e | improvement | 26.071 | 22.206 | -14.8% | 0.0079 | 5 | 13.149 → 13.612 (+3.5%) |
| runtime.callFunctionOn.38KB | improvement | 0.329 | 0.170 | -48.3% | 0.0079 | 5 | 0.259 → 0.101 (-61.0%) |
| target.lifecycle | same | 2.694 | 2.764 | +2.6% | 1.0000 | 5 | 3.199 → 3.227 (+0.9%) |
| dom.mutate.1k | same | 3.476 | 3.557 | +2.3% | 0.4206 | 5 | 3.424 → 3.429 (+0.1%) |
| wv.click.selector | same | 6.395 | 6.485 | +1.4% | 0.0079 | 5 | 0.661 → 0.744 (+12.4%) |
| runtime.handle+release | same | 0.121 | 0.122 | +1.0% | 0.8413 | 5 | 0.073 → 0.068 (-6.9%) |
| rtt.Runtime.evaluate | same | 0.057 | 0.057 | +0.2% | 0.5476 | 5 | 0.033 → 0.032 (-2.7%) |
| dom.querySelectorAll.2k | same | 0.339 | 0.339 | -0.2% | 0.8413 | 5 | 0.307 → 0.306 (-0.2%) |
| throughput.evaluate.pipelined256 | same | 1.788 | 1.784 | -0.2% | 0.6905 | 5 | 0.011 → 0.010 (-11.6%) |
| wv.type | same | 0.115 | 0.114 | -0.3% | 1.0000 | 5 | 0.048 → 0.044 (-9.1%) |
| page.navigate.scripts | same | 5.420 | 5.385 | -0.6% | 0.5476 | 5 | 6.343 → 6.139 (-3.2%) |
| runtime.evaluate.largeValue | same | 1.094 | 1.054 | -3.7% | 0.8413 | 5 | 1.000 → 0.941 (-6.0%) |
| fetch.intercept.fulfill | same | 0.269 | 0.259 | -3.8% | 0.8413 | 5 | 0.244 → 0.227 (-7.2%) |
| page.navigate.dom2k | same | 8.289 | 7.903 | -4.6% | 0.2222 | 5 | 8.860 → 9.023 (+1.8%) |
| input.key.type | same | 0.138 | 0.131 | -4.9% | 0.5476 | 5 | 0.082 → 0.076 (-7.4%) |
| wv.navigate.simple | same | 2.258 | 2.115 | -6.3% | 0.5476 | 5 | 3.323 → 3.356 (+1.0%) |
| wv.evaluate.query2k | same | 0.396 | 0.371 | -6.4% | 0.4206 | 5 | 0.372 → 0.353 (-5.1%) |
| wv.navigate.dom2k | same | 8.414 | 7.798 | -7.3% | 0.5476 | 5 | 9.159 → 9.347 (+2.0%) |
| input.mouse.click | same | 0.171 | 0.157 | -7.8% | 0.5296 | 5 | 0.114 → 0.106 (-6.8%) |

1 regression(s), 2 improvement(s), 11 unclear, 17 unchanged, 0 failed.
