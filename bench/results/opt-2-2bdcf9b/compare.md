# CDP performance: base → head

Base: base, head: head. Threshold +10.0%, alpha 0.05.

| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |
|---|---|---:|---:|---:|---:|---:|---:|
| wv.navigate.scripts | unclear | 4.849 | 5.600 | +15.5% | 0.3095 | 5 | 6.189 → 6.996 (+13.0%) |
| wv.evaluate | unclear | 0.097 | 0.087 | -10.6% | 0.6905 | 5 | 0.076 → 0.072 (-5.8%) |
| rtt.Browser.getVersion | unclear | 0.066 | 0.056 | -14.8% | 0.4206 | 5 | 0.033 → 0.029 (-13.4%) |
| wv.flow.e2e | improvement | 21.921 | 12.348 | -43.7% | 0.0079 | 5 | 13.512 → 12.280 (-9.1%) |
| wv.click.selector | improvement | 6.290 | 1.638 | -74.0% | 0.0079 | 5 | 0.523 → 0.385 (-26.4%) |
| wv.type | same | 0.110 | 0.117 | +7.1% | 1.0000 | 5 | 0.042 → 0.044 (+2.8%) |
| runtime.handle+release | same | 0.122 | 0.131 | +7.1% | 0.5296 | 5 | 0.068 → 0.071 (+5.0%) |
| wv.navigate.simple | same | 1.928 | 1.994 | +3.4% | 0.8413 | 5 | 3.227 → 3.323 (+3.0%) |
| wv.open+navigate+close | same | 4.263 | 4.404 | +3.3% | 0.2222 | 5 | 9.785 → 9.996 (+2.2%) |
| input.key.type | same | 0.124 | 0.128 | +3.0% | 0.4206 | 5 | 0.072 → 0.073 (+0.6%) |
| fetch.intercept.fulfill | same | 0.262 | 0.267 | +1.8% | 1.0000 | 5 | 0.219 → 0.233 (+6.4%) |
| page.navigate.about:blank | same | 1.653 | 1.680 | +1.6% | 0.8413 | 5 | 2.705 → 2.759 (+2.0%) |
| runtime.callFunctionOn.38KB | same | 0.187 | 0.190 | +1.4% | 0.6905 | 5 | 0.111 → 0.112 (+0.9%) |
| dom.textContent.2k | same | 0.186 | 0.188 | +1.1% | 0.6905 | 5 | 0.164 → 0.167 (+1.8%) |
| page.navigate.dom2k | same | 7.665 | 7.700 | +0.5% | 0.8413 | 5 | 8.807 → 8.838 (+0.4%) |
| throughput.evaluate.pipelined256 | same | 1.753 | 1.755 | +0.1% | 0.5476 | 5 | 0.009 → 0.009 (-1.1%) |
| page.navigate.scripts | same | 5.077 | 5.069 | -0.2% | 0.6905 | 5 | 6.023 → 6.071 (+0.8%) |
| runtime.evaluate.largeValue | same | 1.088 | 1.086 | -0.2% | 0.8413 | 5 | 0.999 → 1.004 (+0.4%) |
| dom.querySelectorAll.2k | same | 0.370 | 0.368 | -0.5% | 0.8413 | 5 | 0.330 → 0.331 (+0.2%) |
| page.navigate.simple | same | 1.631 | 1.620 | -0.7% | 1.0000 | 5 | 2.830 → 2.807 (-0.8%) |
| dom.mutate.1k | same | 3.639 | 3.607 | -0.9% | 0.8413 | 5 | 3.474 → 3.477 (+0.1%) |
| wv.navigate.dom2k | same | 7.768 | 7.664 | -1.3% | 0.8413 | 5 | 9.154 → 9.273 (+1.3%) |
| target.lifecycle | same | 2.875 | 2.829 | -1.6% | 0.5476 | 5 | 3.229 → 3.172 (-1.7%) |
| runtime.callFunctionOn.small | same | 0.060 | 0.059 | -2.2% | 0.5476 | 5 | 0.035 → 0.034 (-2.9%) |
| dom.handle.describe+quads | same | 0.328 | 0.320 | -2.5% | 0.6905 | 5 | 0.196 → 0.200 (+2.3%) |
| wv.evaluate.query2k | same | 0.373 | 0.360 | -3.7% | 0.5476 | 5 | 0.349 → 0.352 (+0.9%) |
| rtt.Runtime.evaluate | same | 0.062 | 0.059 | -3.9% | 0.8413 | 5 | 0.035 → 0.034 (-4.3%) |
| network.fetch20 | same | 1.232 | 1.183 | -4.0% | 0.4206 | 5 | 2.252 → 2.236 (-0.7%) |
| wv.cdp.Runtime.evaluate | same | 0.054 | 0.051 | -5.4% | 0.4206 | 5 | 0.033 → 0.031 (-6.0%) |
| connect.ws+getVersion | same | 1.506 | 1.391 | -7.6% | 0.0952 | 5 | 3.217 → 3.159 (-1.8%) |
| input.mouse.click | same | 0.169 | 0.154 | -8.4% | 0.2948 | 5 | 0.108 → 0.103 (-4.9%) |

0 regression(s), 2 improvement(s), 3 unclear, 26 unchanged, 0 failed.

## Idle CPU with a perpetual requestAnimationFrame loop

Browser CPU over 5 s, starting 2 s after the last CDP command (page runs `(function f(){ requestAnimationFrame(f) })()`),
two runs per binary, measured with `rss-sampler`:

| binary | CPU (% of one core) | frames/s |
|---|---:|---:|
| e31dfa9 (5 ms frames) | 1.05, 0.94 | 163, 160 |
| 2bdcf9b (1 ms while a client is active) | 1.04, 1.11 | 169, 167 |

With no client activity the cadence and the CPU stay where they were.
