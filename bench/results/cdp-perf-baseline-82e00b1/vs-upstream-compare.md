# CDP performance: upstream → rewrite

Base: upstream, head: rewrite. Threshold +10.0%, alpha 0.05.

| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |
|---|---|---:|---:|---:|---:|---:|---:|
| dom.mutate.1k | **REGRESSION** | 0.987 | 3.522 | +256.8% | 0.0079 | 5 | 0.954 → 3.417 (+258.3%) |
| wv.click.selector | **REGRESSION** | 6.259 | 18.393 | +193.9% | 0.0079 | 5 | 0.424 → 1.015 (+139.4%) |
| wv.flow.e2e | **REGRESSION** | 19.421 | 49.469 | +154.7% | 0.0079 | 5 | 4.856 → 15.812 (+225.6%) |
| wv.open+navigate+close | **REGRESSION** | 3.339 | 6.665 | +99.6% | 0.0079 | 5 | 2.986 → 9.189 (+207.7%) |
| wv.evaluate.query2k | **REGRESSION** | 0.217 | 0.351 | +61.6% | 0.0079 | 5 | 0.185 → 0.327 (+76.4%) |
| dom.querySelectorAll.2k | **REGRESSION** | 0.213 | 0.324 | +51.7% | 0.0079 | 5 | 0.182 → 0.303 (+66.8%) |
| fetch.intercept.fulfill | **REGRESSION** | 0.160 | 0.230 | +43.4% | 0.0317 | 5 | 0.101 → 0.191 (+90.0%) |
| page.navigate.scripts | **REGRESSION** | 3.661 | 5.023 | +37.2% | 0.0079 | 5 | 4.551 → 5.511 (+21.1%) |
| target.lifecycle | **REGRESSION** | 1.753 | 2.359 | +34.5% | 0.0079 | 5 | 1.622 → 2.864 (+76.5%) |
| input.key.type | **REGRESSION** | 0.086 | 0.111 | +28.5% | 0.0079 | 5 | 0.040 → 0.066 (+63.3%) |
| wv.navigate.scripts | **REGRESSION** | 3.837 | 4.899 | +27.7% | 0.0079 | 5 | 4.596 → 5.601 (+21.9%) |
| wv.navigate.simple | **REGRESSION** | 1.809 | 2.079 | +14.9% | 0.0079 | 5 | 2.865 → 3.005 (+4.9%) |
| page.navigate.about:blank | **REGRESSION** | 1.315 | 1.484 | +12.8% | 0.0079 | 5 | 2.640 → 2.473 (-6.4%) |
| page.navigate.simple | **REGRESSION** | 1.613 | 1.805 | +11.9% | 0.0317 | 5 | 2.840 → 2.580 (-9.1%) |
| input.mouse.click | unclear | 0.095 | 0.140 | +47.6% | 0.0556 | 5 | 0.050 → 0.093 (+85.1%) |
| wv.type | unclear | 0.082 | 0.108 | +31.3% | 0.0952 | 5 | 0.027 → 0.045 (+66.8%) |
| dom.handle.describe+quads | unclear | 0.218 | 0.282 | +29.3% | 0.0952 | 5 | 0.115 → 0.187 (+62.4%) |
| dom.textContent.2k | unclear | 0.135 | 0.166 | +23.4% | 0.2222 | 5 | 0.109 → 0.158 (+44.2%) |
| wv.evaluate | unclear | 0.101 | 0.081 | -19.5% | 0.9166 | 5 | 0.044 → 0.066 (+49.4%) |
| runtime.callFunctionOn.small | unclear | 0.092 | 0.069 | -24.8% | 0.0556 | 5 | 0.043 → 0.038 (-12.0%) |
| wv.cdp.Runtime.evaluate | unclear | 0.075 | 0.051 | -31.9% | 0.0952 | 5 | 0.031 → 0.032 (+1.6%) |
| throughput.evaluate.pipelined256 | improvement | 2.428 | 1.812 | -25.4% | 0.0079 | 5 | 0.012 → 0.011 (-0.9%) |
| connect.ws+getVersion | improvement | 1.472 | 1.059 | -28.0% | 0.0079 | 5 | 1.362 → 1.173 (-13.9%) |
| page.navigate.dom2k | improvement | 10.756 | 7.566 | -29.7% | 0.0079 | 5 | 11.941 → 8.539 (-28.5%) |
| runtime.evaluate.largeValue | improvement | 1.497 | 1.034 | -30.9% | 0.0088 | 5 | 1.488 → 0.953 (-35.9%) |
| wv.navigate.dom2k | improvement | 10.804 | 7.445 | -31.1% | 0.0079 | 5 | 11.930 → 8.710 (-27.0%) |
| runtime.callFunctionOn.38KB | improvement | 0.527 | 0.337 | -36.0% | 0.0079 | 5 | 0.413 → 0.243 (-41.1%) |
| runtime.handle+release | same | 0.112 | 0.111 | -1.1% | 1.0000 | 5 | 0.060 → 0.069 (+15.0%) |
| network.fetch20 | same | 1.131 | 1.111 | -1.7% | 1.0000 | 5 | 1.067 → 2.264 (+112.1%) |
| rtt.Runtime.evaluate | same | 0.051 | 0.049 | -5.4% | 0.8413 | 5 | 0.027 → 0.032 (+16.4%) |
| rtt.Browser.getVersion | same | 0.051 | 0.047 | -7.9% | 0.6905 | 5 | 0.020 → 0.024 (+20.0%) |

14 regression(s), 6 improvement(s), 7 unclear, 4 unchanged, 0 failed.
