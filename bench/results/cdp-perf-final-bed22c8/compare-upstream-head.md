# CDP performance: upstream → head

Base: upstream, head: head. Threshold +10.0%, alpha 0.05.

| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |
|---|---|---:|---:|---:|---:|---:|---:|
| dom.mutate.1k | **REGRESSION** | 0.966 | 2.701 | +179.8% | 0.0079 | 5 | 0.932 → 2.622 (+181.3%) |
| dom.querySelectorAll.2k | **REGRESSION** | 0.204 | 0.318 | +55.5% | 0.0079 | 5 | 0.176 → 0.286 (+62.5%) |
| wv.evaluate.query2k | **REGRESSION** | 0.249 | 0.367 | +47.3% | 0.0079 | 5 | 0.196 → 0.321 (+64.4%) |
| page.navigate.scripts | **REGRESSION** | 3.563 | 4.804 | +34.8% | 0.0079 | 5 | 4.352 → 5.383 (+23.7%) |
| target.lifecycle | **REGRESSION** | 1.815 | 2.434 | +34.1% | 0.0079 | 5 | 1.657 → 2.809 (+69.5%) |
| wv.navigate.scripts | **REGRESSION** | 4.076 | 5.394 | +32.3% | 0.0159 | 5 | 4.798 → 6.021 (+25.5%) |
| wv.open+navigate+close | **REGRESSION** | 3.328 | 4.305 | +29.4% | 0.0079 | 5 | 3.034 → 10.181 (+235.6%) |
| dom.textContent.2k | **REGRESSION** | 0.128 | 0.147 | +15.1% | 0.0159 | 5 | 0.106 → 0.150 (+42.2%) |
| page.navigate.about:blank | **REGRESSION** | 1.310 | 1.495 | +14.1% | 0.0079 | 5 | 2.576 → 2.406 (-6.6%) |
| fetch.intercept.fulfill | unclear | 0.162 | 0.205 | +26.6% | 0.0952 | 5 | 0.109 → 0.153 (+40.6%) |
| input.mouse.click | unclear | 0.093 | 0.110 | +17.2% | 0.0952 | 5 | 0.049 → 0.065 (+32.9%) |
| wv.evaluate | unclear | 0.064 | 0.073 | +13.8% | 0.6905 | 5 | 0.042 → 0.057 (+35.2%) |
| wv.navigate.simple | unclear | 1.801 | 2.021 | +12.2% | 0.0556 | 5 | 2.874 → 3.044 (+5.9%) |
| rtt.Browser.getVersion | unclear | 0.041 | 0.035 | -15.6% | 0.1508 | 5 | 0.018 → 0.013 (-29.9%) |
| rtt.Runtime.evaluate | unclear | 0.047 | 0.039 | -16.8% | 0.0556 | 5 | 0.026 → 0.017 (-36.1%) |
| runtime.handle+release | improvement | 0.108 | 0.083 | -23.8% | 0.0079 | 5 | 0.060 → 0.038 (-35.7%) |
| wv.navigate.dom2k | improvement | 10.794 | 7.986 | -26.0% | 0.0079 | 5 | 11.927 → 9.092 (-23.8%) |
| page.navigate.dom2k | improvement | 10.599 | 7.555 | -28.7% | 0.0079 | 5 | 11.841 → 8.541 (-27.9%) |
| wv.flow.e2e | improvement | 19.696 | 11.901 | -39.6% | 0.0079 | 5 | 4.946 → 12.266 (+148.0%) |
| runtime.callFunctionOn.small | improvement | 0.068 | 0.040 | -41.0% | 0.0079 | 5 | 0.040 → 0.017 (-57.0%) |
| throughput.evaluate.pipelined256 | improvement | 2.541 | 1.193 | -53.0% | 0.0079 | 5 | 0.012 → 0.004 (-67.7%) |
| wv.cdp.Runtime.evaluate | improvement | 0.083 | 0.037 | -54.8% | 0.0079 | 5 | 0.033 → 0.018 (-43.7%) |
| runtime.evaluate.largeValue | improvement | 1.488 | 0.519 | -65.1% | 0.0079 | 5 | 1.484 → 0.452 (-69.6%) |
| runtime.callFunctionOn.38KB | improvement | 0.492 | 0.160 | -67.6% | 0.0079 | 5 | 0.406 → 0.087 (-78.6%) |
| wv.click.selector | improvement | 6.166 | 1.497 | -75.7% | 0.0079 | 5 | 0.331 → 0.247 (-25.3%) |
| page.navigate.simple | same | 1.594 | 1.595 | +0.0% | 0.4206 | 5 | 2.738 → 2.606 (-4.8%) |
| input.key.type | same | 0.088 | 0.085 | -3.8% | 0.5476 | 5 | 0.041 → 0.041 (-2.2%) |
| connect.ws+getVersion | same | 1.570 | 1.494 | -4.9% | 0.4206 | 5 | 1.378 → 3.163 (+129.6%) |
| dom.handle.describe+quads | same | 0.229 | 0.210 | -8.0% | 0.0952 | 5 | 0.120 → 0.106 (-11.9%) |
| wv.type | same | 0.104 | 0.095 | -8.6% | 1.0000 | 5 | 0.028 → 0.028 (+0.0%) |
| network.fetch20 | same | 1.135 | 1.035 | -8.8% | 0.6905 | 5 | 1.081 → 1.691 (+56.4%) |

9 regression(s), 10 improvement(s), 6 unclear, 6 unchanged, 0 failed.
