# CDP performance: base → head

Base: base, head: head. Threshold +10.0%, alpha 0.05.

| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |
|---|---|---:|---:|---:|---:|---:|---:|
| rtt.Browser.getVersion | unclear | 0.058 | 0.052 | -9.8% | 0.3095 | 5 | 0.030 → 0.028 (-6.0%) |
| dom.textContent.2k | unclear | 0.195 | 0.176 | -9.9% | 0.0952 | 5 | 0.173 → 0.169 (-2.4%) |
| wv.evaluate | unclear | 0.107 | 0.092 | -14.6% | 0.5476 | 5 | 0.078 → 0.080 (+3.0%) |
| runtime.evaluate.largeValue | improvement | 1.099 | 0.583 | -46.9% | 0.0079 | 5 | 0.990 → 0.500 (-49.5%) |
| connect.ws+getVersion | same | 1.467 | 1.560 | +6.3% | 0.3095 | 5 | 3.302 → 3.339 (+1.1%) |
| wv.navigate.scripts | same | 5.341 | 5.583 | +4.5% | 0.0952 | 5 | 6.453 → 6.703 (+3.9%) |
| throughput.evaluate.pipelined256 | same | 1.726 | 1.802 | +4.4% | 0.8413 | 5 | 0.009 → 0.009 (-1.1%) |
| runtime.callFunctionOn.small | same | 0.058 | 0.060 | +2.7% | 0.6752 | 5 | 0.036 → 0.034 (-4.2%) |
| wv.flow.e2e | same | 12.576 | 12.876 | +2.4% | 0.1508 | 5 | 13.201 → 13.221 (+0.2%) |
| wv.click.selector | same | 1.557 | 1.588 | +2.0% | 1.0000 | 5 | 0.369 → 0.367 (-0.8%) |
| dom.mutate.1k | same | 2.732 | 2.783 | +1.9% | 0.4206 | 5 | 2.631 → 2.661 (+1.2%) |
| input.mouse.click | same | 0.155 | 0.158 | +1.6% | 0.6905 | 5 | 0.102 → 0.106 (+3.7%) |
| page.navigate.scripts | same | 5.653 | 5.731 | +1.4% | 1.0000 | 5 | 6.296 → 6.407 (+1.8%) |
| wv.navigate.dom2k | same | 8.227 | 8.328 | +1.2% | 0.2222 | 5 | 9.328 → 9.338 (+0.1%) |
| target.lifecycle | same | 2.856 | 2.882 | +0.9% | 0.8413 | 5 | 3.266 → 3.247 (-0.6%) |
| input.key.type | same | 0.120 | 0.121 | +0.5% | 1.0000 | 5 | 0.072 → 0.070 (-3.9%) |
| dom.querySelectorAll.2k | same | 0.361 | 0.362 | +0.2% | 0.9166 | 5 | 0.321 → 0.320 (-0.2%) |
| dom.handle.describe+quads | same | 0.304 | 0.303 | -0.2% | 0.8413 | 5 | 0.182 → 0.179 (-1.6%) |
| page.navigate.about:blank | same | 1.710 | 1.704 | -0.3% | 0.6905 | 5 | 2.647 → 2.731 (+3.2%) |
| wv.evaluate.query2k | same | 0.388 | 0.386 | -0.5% | 0.8413 | 5 | 0.370 → 0.361 (-2.3%) |
| runtime.callFunctionOn.38KB | same | 0.181 | 0.180 | -1.0% | 0.5296 | 5 | 0.109 → 0.107 (-1.3%) |
| page.navigate.dom2k | same | 7.797 | 7.693 | -1.3% | 0.0952 | 5 | 8.833 → 8.835 (+0.0%) |
| runtime.handle+release | same | 0.129 | 0.126 | -2.2% | 0.4206 | 5 | 0.072 → 0.071 (-2.2%) |
| page.navigate.simple | same | 1.721 | 1.675 | -2.7% | 0.0465 | 5 | 2.983 → 2.818 (-5.5%) |
| fetch.intercept.fulfill | same | 0.268 | 0.258 | -3.6% | 0.4206 | 5 | 0.230 → 0.221 (-3.9%) |
| wv.open+navigate+close | same | 4.960 | 4.776 | -3.7% | 0.5476 | 5 | 10.772 → 10.733 (-0.4%) |
| wv.cdp.Runtime.evaluate | same | 0.058 | 0.055 | -3.8% | 1.0000 | 5 | 0.034 → 0.032 (-5.0%) |
| wv.navigate.simple | same | 2.153 | 2.030 | -5.7% | 0.0317 | 5 | 3.333 → 3.293 (-1.2%) |
| network.fetch20 | same | 1.253 | 1.178 | -6.0% | 0.6905 | 5 | 2.219 → 2.218 (-0.0%) |
| wv.type | same | 0.121 | 0.114 | -6.2% | 0.2222 | 5 | 0.046 → 0.044 (-3.5%) |
| rtt.Runtime.evaluate | same | 0.059 | 0.054 | -8.8% | 0.1508 | 5 | 0.033 → 0.032 (-2.7%) |

0 regression(s), 1 improvement(s), 3 unclear, 27 unchanged, 0 failed.
