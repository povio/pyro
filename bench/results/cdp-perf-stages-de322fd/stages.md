# CDP performance: traced

2026-10-27T10:42:39.330Z on Apple M1 Max (10 CPUs, darwin 27.0.0), 3 round(s), scale 1.
Load average at start 7.97/10.31/13.26, at end 7.08/10.00/13.10.

- **traced**: `/Users/xban1x/Povio/lp-cdp-perf-wt/target-perf/release/lightningpanda` sha256 `2a62c32ca3f66da0`, startup median 6.768 ms

Times are milliseconds per sample (median, p90 over all rounds); `rsd` is the robust relative spread; `cpu/op` is browser CPU per operation.

| scenario | traced median | p90 | rsd | ops/s | cpu/op |
|---|---:|---:|---:|---:|---:|
| connect.ws+getVersion | 1.029 | 1.334 | 0.13 | 972 | 1.100 |
| rtt.Browser.getVersion | 0.077 | 0.122 | 0.50 | 13021 | 0.029 |
| rtt.Runtime.evaluate | 0.053 | 0.088 | 0.24 | 18832 | 0.033 |
| throughput.evaluate.pipelined256 | 1.838 | 2.209 | 0.09 | 139274 | 0.012 |
| runtime.evaluate.largeValue | 1.007 | 1.383 | 0.11 | 993 | 0.921 |
| runtime.callFunctionOn.small | 0.060 | 0.110 | 0.29 | 16584 | 0.038 |
| runtime.callFunctionOn.38KB | 0.302 | 0.391 | 0.16 | 3315 | 0.240 |
| runtime.handle+release | 0.115 | 0.165 | 0.18 | 8711 | 0.070 |
| dom.querySelectorAll.2k | 0.332 | 0.401 | 0.12 | 3010 | 0.302 |
| dom.textContent.2k | 0.166 | 0.268 | 0.19 | 6039 | 0.160 |
| dom.mutate.1k | 3.496 | 3.882 | 0.08 | 286 | 3.419 |
| dom.handle.describe+quads | 0.320 | 0.524 | 0.30 | 3130 | 0.203 |
| page.navigate.about:blank | 1.518 | 2.161 | 0.14 | 659 | 2.487 |
| page.navigate.simple | 1.821 | 2.301 | 0.11 | 549 | 2.585 |
| page.navigate.scripts | 5.058 | 6.175 | 0.11 | 198 | 5.566 |
| page.navigate.dom2k | 7.672 | 8.174 | 0.03 | 130 | 8.400 |
| target.lifecycle | 2.476 | 3.616 | 0.11 | 404 | 2.943 |
| network.fetch20 | 1.266 | 1.574 | 0.13 | 790 | 2.291 |
| fetch.intercept.fulfill | 0.233 | 0.336 | 0.18 | 4301 | 0.198 |
| input.mouse.click | 0.144 | 0.207 | 0.20 | 6920 | 0.099 |
| input.key.type | 0.117 | 0.185 | 0.19 | 8540 | 0.071 |
| wv.open+navigate+close | 6.557 | 7.503 | 0.06 | 153 | 9.136 |
| wv.navigate.simple | 1.990 | 2.771 | 0.14 | 502 | 2.895 |
| wv.navigate.scripts | 5.296 | 6.283 | 0.13 | 189 | 5.690 |
| wv.navigate.dom2k | 7.664 | 8.279 | 0.05 | 130 | 8.734 |
| wv.evaluate | 0.117 | 0.180 | 0.37 | 8576 | 0.074 |
| wv.evaluate.query2k | 0.392 | 0.475 | 0.13 | 2553 | 0.334 |
| wv.cdp.Runtime.evaluate | 0.068 | 0.138 | 0.51 | 14749 | 0.034 |
| wv.click.selector | 18.845 | 23.500 | 0.08 | 53 | 1.066 |
| wv.type | 0.115 | 0.164 | 0.16 | 8696 | 0.051 |
| wv.flow.e2e | 48.419 | 50.236 | 0.02 | 21 | 14.556 |

## Phase breakdown (median ms since the step started)

- page.navigate.about:blank [traced]: navigateReply 1.491, frameNavigated 1.505, domContentEventFired 1.513, loadEventFired 1.515
- page.navigate.simple [traced]: navigateReply 1.795, frameNavigated 1.806, domContentEventFired 1.816, loadEventFired 1.819
- page.navigate.scripts [traced]: navigateReply 1.808, frameNavigated 1.820, domContentEventFired 4.888, loadEventFired 4.891
- page.navigate.dom2k [traced]: navigateReply 2.626, frameNavigated 2.639, domContentEventFired 7.665, loadEventFired 7.668
- target.lifecycle [traced]: createBrowserContext 0.136, createTarget 1.587, attachToTarget 1.735, enable 2.096, closeTarget 2.279, disposeBrowserContext 2.471
- wv.flow.e2e [traced]: navigate 8.913, input 45.531, result 48.363, close 48.401

## Inner CDP stages (perf-trace)

Inclusive nanoseconds per benchmark operation (stages nest: `cdp.handle_text` contains `cdp.parse` and `cdp.dispatch ...`, which contains `js.*`, `remote.*`, `json.serialize`, `outbox.push`). Stages under 10 ns/op are left out. `io.*` stages run on the socket thread; `io.to_browser` and `io.outbound_queue` are the cross-thread hops.

### connect.ws+getVersion [traced]

Timer floor: clock 17.8 ns, one record 56.6 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| io.to_browser | 11504.2 | 1.05 | 10956.4 | 10240 | 16384 |
| io.outbound_queue | 10872.9 | 1.05 | 10355.1 | 8192 | 32768 |
| io.ws_write | 9385.4 | 1.05 | 8938.4 | 8192 | 14336 |
| cdp.handle_text | 9281.3 | 1.05 | 8839.3 | 8192 | 12288 |
| cdp.dispatch Browser.getVersion | 6568.8 | 1 | 6568.8 | 6144 | 10240 |
| outbox.push | 1846 | 1.05 | 1758 | 1536 | 3072 |
| json.serialize | 1260.5 | 1.05 | 1200.4 | 896 | 4096 |
| cdp.parse | 1172.9 | 1.05 | 1117 | 896 | 2048 |
| io.ws_decode | 362.6 | 1.05 | 345.3 | 160 | 2048 |
| cdp.dispatch LP.perfReset | 320.9 | 0.05 | 6417 | 6144 | 6144 |
| host.flush | 39.6 | 1.05 | 37.7 | 40 | 80 |

### rtt.Browser.getVersion [traced]

Timer floor: clock 24.9 ns, one record 67.3 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| io.to_browser | 7967.1 | 1 | 7951.2 | 6144 | 24576 |
| io.ws_write | 5963.5 | 1 | 5951.6 | 5120 | 24576 |
| io.outbound_queue | 4739.7 | 1 | 4730.2 | 3584 | 24576 |
| cdp.handle_text | 3171.1 | 1 | 3164.8 | 2560 | 16384 |
| cdp.dispatch Browser.getVersion | 2323.1 | 1 | 2323.1 | 1536 | 10240 |
| outbox.push | 1624 | 1 | 1620.8 | 1024 | 10240 |
| json.serialize | 473.3 | 1 | 472.3 | 320 | 896 |
| io.ws_decode | 341.8 | 1 | 341.2 | 256 | 1280 |
| cdp.parse | 277.2 | 1 | 276.7 | 224 | 640 |
| host.flush | 31.3 | 1 | 31.3 | 40 | 80 |

### rtt.Runtime.evaluate [traced]

Timer floor: clock 18.4 ns, one record 56.4 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| io.to_browser | 9716.9 | 1 | 9697.5 | 8192 | 32768 |
| cdp.handle_text | 8097.2 | 1 | 8081 | 6144 | 24576 |
| cdp.dispatch Runtime.evaluate | 7084.3 | 1 | 7084.3 | 5120 | 20480 |
| io.ws_write | 5657.6 | 1 | 5646.3 | 4096 | 16384 |
| io.outbound_queue | 5331.7 | 1 | 5321 | 3584 | 28672 |
| js.eval | 2511 | 1 | 2511 | 1792 | 12288 |
| outbox.push | 1630.9 | 1 | 1627.6 | 1024 | 5120 |
| remote.register+reply | 847.6 | 1 | 847.6 | 640 | 1536 |
| cdp.parse | 432.9 | 1 | 432 | 320 | 896 |
| remote.describe | 335.9 | 1 | 335.9 | 320 | 512 |
| io.ws_decode | 326.3 | 1 | 325.7 | 224 | 896 |
| json.serialize | 252.8 | 1 | 252.3 | 192 | 512 |
| cdp.params | 126.9 | 1 | 126.9 | 112 | 256 |
| host.flush | 49 | 1 | 48.9 | 40 | 80 |
| cdp.dispatch LP.perfReset | 11 | 0 | 5500 | 5120 | 5120 |

### throughput.evaluate.pipelined256 [traced]

Timer floor: clock 18.4 ns, one record 56.3 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| io.to_browser | 212031.1 | 1 | 211989.7 | 163840 | 786432 |
| io.outbound_queue | 15755.9 | 1 | 15752.8 | 7168 | 98304 |
| cdp.handle_text | 4665.5 | 1 | 4664.6 | 3584 | 16384 |
| cdp.dispatch Runtime.evaluate | 4160.7 | 1 | 4160.7 | 3072 | 16384 |
| io.ws_write | 3334.9 | 1 | 3334.2 | 2560 | 14336 |
| js.eval | 1236 | 1 | 1236 | 640 | 7168 |
| remote.register+reply | 658.2 | 1 | 658.2 | 512 | 1280 |
| outbox.push | 496 | 1 | 495.9 | 192 | 3584 |
| json.serialize | 225.3 | 1 | 225.3 | 192 | 448 |
| cdp.parse | 215.8 | 1 | 215.7 | 192 | 448 |
| remote.describe | 212.5 | 1 | 212.5 | 160 | 448 |
| io.ws_decode | 100.9 | 1 | 100.9 | 40 | 448 |
| cdp.params | 88.1 | 1 | 88.1 | 80 | 192 |
| host.flush | 56.6 | 1 | 56.6 | 40 | 80 |

### runtime.evaluate.largeValue [traced]

Timer floor: clock 18.5 ns, one record 53.6 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 764276.3 | 1.01 | 756709.2 | 655360 | 1048576 |
| cdp.dispatch Runtime.evaluate | 760858.3 | 1 | 760858.3 | 655360 | 1048576 |
| remote.describe | 356010 | 1 | 356010 | 327680 | 458752 |
| remote.register+reply | 296775.9 | 1 | 296775.9 | 262144 | 393216 |
| js.eval | 98645.8 | 1 | 98645.8 | 81920 | 393216 |
| json.serialize | 73113.8 | 1.01 | 72389.9 | 65536 | 98304 |
| io.ws_write | 15740.4 | 1.01 | 15584.6 | 12288 | 49152 |
| io.to_browser | 15576.6 | 1.01 | 15422.4 | 12288 | 32768 |
| io.outbound_queue | 11039.2 | 1.01 | 10929.9 | 8192 | 32768 |
| outbox.push | 3310.4 | 1.01 | 3277.6 | 3072 | 6144 |
| cdp.parse | 1414.6 | 1.01 | 1400.6 | 896 | 3072 |
| cdp.params | 1042.5 | 1 | 1042.5 | 768 | 5120 |
| io.ws_decode | 882.6 | 1.01 | 873.8 | 512 | 7168 |
| cdp.dispatch LP.perfReset | 48.8 | 0.01 | 4875 | 4096 | 4096 |
| host.flush | 44.1 | 1.01 | 43.7 | 40 | 80 |

### runtime.callFunctionOn.small [traced]

Timer floor: clock 18.5 ns, one record 65.8 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 11841 | 1 | 11801.7 | 8192 | 32768 |
| cdp.dispatch Runtime.callFunctionOn | 10730.6 | 1 | 10730.6 | 8192 | 28672 |
| io.to_browser | 9757.5 | 1 | 9725 | 8192 | 28672 |
| io.outbound_queue | 6367.9 | 1 | 6346.8 | 4096 | 32768 |
| io.ws_write | 5945.6 | 1 | 5925.8 | 4096 | 20480 |
| js.call_function | 3771.4 | 1 | 3771.4 | 2560 | 14336 |
| outbox.push | 2009.9 | 1 | 2003.2 | 1280 | 14336 |
| remote.register+reply | 884.4 | 1 | 884.4 | 640 | 2048 |
| cdp.parse | 576.1 | 1 | 574.2 | 448 | 1280 |
| cdp.params | 376 | 1 | 376 | 256 | 1024 |
| io.ws_decode | 343 | 1 | 341.9 | 224 | 896 |
| remote.describe | 307.4 | 1 | 307.4 | 256 | 512 |
| json.serialize | 282.3 | 1 | 281.4 | 224 | 640 |
| host.flush | 49.2 | 1 | 49 | 40 | 80 |
| cdp.dispatch LP.perfReset | 14.6 | 0 | 4375 | 4096 | 4096 |

### runtime.callFunctionOn.38KB [traced]

Timer floor: clock 17.8 ns, one record 58.1 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 194652.1 | 1 | 193683.6 | 163840 | 262144 |
| cdp.dispatch Runtime.callFunctionOn | 168738.9 | 1 | 168738.9 | 131072 | 229376 |
| js.call_function | 132309.6 | 1 | 132309.6 | 114688 | 196608 |
| cdp.params | 27785.6 | 1 | 27785.6 | 24576 | 40960 |
| cdp.parse | 24900.4 | 1 | 24776.5 | 20480 | 40960 |
| io.to_browser | 11122.3 | 1 | 11067 | 8192 | 32768 |
| io.outbound_queue | 8239.2 | 1 | 8198.2 | 6144 | 24576 |
| io.ws_write | 6647.9 | 1 | 6614.8 | 4096 | 28672 |
| io.ws_decode | 4158.8 | 1 | 4138.1 | 3072 | 8192 |
| outbox.push | 2560.4 | 1 | 2547.7 | 1792 | 8192 |
| remote.register+reply | 1162.5 | 1 | 1162.5 | 896 | 4096 |
| remote.describe | 407.3 | 1 | 407.3 | 320 | 1024 |
| json.serialize | 346.7 | 1 | 345 | 256 | 896 |
| host.flush | 40.8 | 1 | 40.6 | 40 | 80 |
| cdp.dispatch LP.perfReset | 32.1 | 0.01 | 6417 | 6144 | 6144 |

### runtime.handle+release [traced]

Timer floor: clock 18.4 ns, one record 54.1 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 21535.3 | 2 | 10749.7 | 10240 | 32768 |
| io.to_browser | 18335.5 | 2 | 9152.5 | 8192 | 28672 |
| cdp.dispatch Runtime.evaluate | 16212.2 | 1 | 16212.2 | 12288 | 40960 |
| io.ws_write | 10927.4 | 2 | 5454.6 | 4096 | 16384 |
| io.outbound_queue | 10791.8 | 2 | 5386.9 | 4096 | 32768 |
| js.eval | 7040 | 1 | 7040 | 5120 | 20480 |
| remote.describe | 3600.3 | 1 | 3600.3 | 2560 | 16384 |
| cdp.dispatch Runtime.releaseObject | 3578.3 | 1 | 3578.3 | 2560 | 10240 |
| outbox.push | 3227.9 | 2 | 1611.3 | 1280 | 10240 |
| remote.register+reply | 1440.1 | 1 | 1440.1 | 1280 | 4096 |
| cdp.parse | 763.9 | 2 | 381.3 | 320 | 640 |
| json.serialize | 746.8 | 2 | 372.8 | 320 | 896 |
| io.ws_decode | 634.8 | 2 | 316.9 | 224 | 640 |
| cdp.params | 249.1 | 2 | 124.5 | 112 | 256 |
| host.flush | 99.4 | 2 | 49.6 | 40 | 112 |
| cdp.dispatch LP.perfReset | 17.2 | 0 | 5167 | 5120 | 5120 |

### dom.querySelectorAll.2k [traced]

Timer floor: clock 17.8 ns, one record 54.1 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 276095.4 | 1 | 274721.8 | 262144 | 327680 |
| cdp.dispatch Runtime.evaluate | 274342.8 | 1 | 274342.8 | 262144 | 327680 |
| js.eval | 266712.3 | 1 | 266712.3 | 229376 | 327680 |
| io.to_browser | 11816.7 | 1 | 11757.9 | 8192 | 32768 |
| io.outbound_queue | 7826.3 | 1 | 7787.3 | 5120 | 28672 |
| io.ws_write | 6484.4 | 1 | 6452.2 | 4096 | 24576 |
| outbox.push | 2357.5 | 1 | 2345.8 | 1792 | 5120 |
| remote.register+reply | 1070.6 | 1 | 1070.6 | 768 | 2560 |
| cdp.parse | 525 | 1 | 522.4 | 448 | 1536 |
| cdp.params | 496 | 1 | 496 | 320 | 1536 |
| io.ws_decode | 478.3 | 1 | 475.9 | 256 | 1280 |
| remote.describe | 371.8 | 1 | 371.8 | 256 | 1024 |
| json.serialize | 336.3 | 1 | 334.6 | 256 | 896 |
| host.flush | 44.9 | 1 | 44.6 | 40 | 80 |
| cdp.dispatch LP.perfReset | 20.4 | 0.01 | 4084 | 3584 | 3584 |

### dom.textContent.2k [traced]

Timer floor: clock 18.3 ns, one record 56.5 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 126967.5 | 1.01 | 125710.4 | 98304 | 327680 |
| cdp.dispatch Runtime.evaluate | 125650.8 | 1 | 125650.8 | 98304 | 327680 |
| js.eval | 118770.4 | 1 | 118770.4 | 98304 | 327680 |
| io.to_browser | 11476.7 | 1.01 | 11363 | 8192 | 32768 |
| io.outbound_queue | 7387.4 | 1.01 | 7314.3 | 5120 | 20480 |
| io.ws_write | 6043.3 | 1.01 | 5983.4 | 4096 | 20480 |
| outbox.push | 2125.4 | 1.01 | 2104.4 | 1536 | 5120 |
| remote.register+reply | 1276.7 | 1 | 1276.7 | 896 | 3584 |
| json.serialize | 592.5 | 1.01 | 586.6 | 320 | 1536 |
| cdp.parse | 538.4 | 1.01 | 533 | 384 | 1792 |
| io.ws_decode | 478.4 | 1.01 | 473.6 | 256 | 2048 |
| remote.describe | 418.3 | 1 | 418.3 | 320 | 1024 |
| cdp.params | 171.7 | 1 | 171.7 | 160 | 512 |
| cdp.dispatch LP.perfReset | 55.8 | 0.01 | 5583 | 5120 | 5120 |
| host.flush | 46.3 | 1.01 | 45.8 | 40 | 80 |

### dom.mutate.1k [traced]

Timer floor: clock 17.6 ns, one record 53.8 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 3366239.9 | 1.02 | 3300235.2 | 3145728 | 4194304 |
| cdp.dispatch Runtime.evaluate | 3362174.1 | 1 | 3362174.1 | 3145728 | 4194304 |
| js.eval | 3293486.7 | 1 | 3293486.7 | 3145728 | 3670016 |
| io.to_browser | 20635.8 | 1.02 | 20231.2 | 16384 | 40960 |
| io.ws_write | 14982.5 | 1.02 | 14688.7 | 10240 | 32768 |
| io.outbound_queue | 13066.7 | 1.02 | 12810.5 | 10240 | 28672 |
| outbox.push | 3944.9 | 1.02 | 3867.6 | 3072 | 6144 |
| cdp.params | 2684.9 | 1 | 2684.9 | 1792 | 16384 |
| cdp.parse | 1994.2 | 1.02 | 1955.1 | 1792 | 6144 |
| remote.register+reply | 1988.3 | 1 | 1988.3 | 1536 | 4096 |
| io.ws_decode | 1858.4 | 1.02 | 1821.9 | 1024 | 20480 |
| json.serialize | 705.1 | 1.02 | 691.2 | 512 | 1536 |
| remote.describe | 673.3 | 1 | 673.3 | 448 | 1792 |
| cdp.dispatch LP.perfReset | 188.3 | 0.02 | 9417 | 8192 | 8192 |
| host.flush | 44.1 | 1.02 | 43.3 | 40 | 80 |

### dom.handle.describe+quads [traced]

Timer floor: clock 18.5 ns, one record 56.4 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 86957.3 | 4 | 21712.2 | 16384 | 81920 |
| io.to_browser | 41049.9 | 4 | 10249.7 | 8192 | 32768 |
| cdp.dispatch Runtime.evaluate | 34711.4 | 1 | 34711.4 | 24576 | 114688 |
| io.outbound_queue | 26271.3 | 4 | 6559.6 | 4096 | 32768 |
| cdp.dispatch DOM.getContentQuads | 25764 | 1 | 25764 | 20480 | 57344 |
| io.ws_write | 23987.3 | 4 | 5989.3 | 4096 | 20480 |
| js.eval | 18406.5 | 1 | 18406.5 | 12288 | 65536 |
| cdp.dispatch DOM.describeNode | 17426.1 | 1 | 17426.1 | 14336 | 40960 |
| remote.describe | 9508.3 | 1 | 9508.3 | 6144 | 32768 |
| outbox.push | 7682.3 | 4 | 1918.2 | 1280 | 7168 |
| cdp.dispatch Runtime.releaseObject | 4213.3 | 1 | 4213.3 | 3072 | 14336 |
| json.serialize | 3232.4 | 4 | 807.1 | 512 | 2048 |
| remote.register+reply | 2008.1 | 1 | 2008.1 | 1536 | 5120 |
| cdp.parse | 1641.2 | 4 | 409.8 | 320 | 896 |
| io.ws_decode | 1375.2 | 4 | 343.4 | 256 | 896 |
| cdp.params | 819.2 | 5 | 163.8 | 112 | 512 |
| host.flush | 258.5 | 4 | 64.6 | 40 | 192 |
| cdp.dispatch LP.perfReset | 32.3 | 0.01 | 6458 | 6144 | 6144 |

### page.navigate.about:blank [traced]

Timer floor: clock 18.4 ns, one record 60.1 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 1451968 | 1.01 | 1437592.1 | 1310720 | 2621440 |
| cdp.dispatch Page.navigate | 1449418.4 | 1 | 1449418.4 | 1310720 | 2621440 |
| io.outbound_queue | 341974.2 | 9.01 | 37955 | 32768 | 81920 |
| io.ws_write | 41845.3 | 9.01 | 4644.3 | 2048 | 28672 |
| io.to_browser | 19650.9 | 1.01 | 19456.3 | 16384 | 40960 |
| outbox.push | 5149.6 | 9.01 | 571.5 | 80 | 6144 |
| json.serialize | 3926.3 | 9.01 | 435.8 | 320 | 1280 |
| cdp.parse | 1702.1 | 1.01 | 1685.3 | 1536 | 5120 |
| io.ws_decode | 995.1 | 1.01 | 985.2 | 640 | 4096 |
| cdp.params | 408.4 | 1 | 408.4 | 320 | 768 |
| cdp.dispatch LP.perfReset | 56.3 | 0.01 | 5625 | 5120 | 5120 |
| host.flush | 53.2 | 1.01 | 52.7 | 40 | 112 |

### page.navigate.simple [traced]

Timer floor: clock 18.4 ns, one record 55.5 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| io.outbound_queue | 245611.7 | 9.02 | 27229.7 | 24576 | 65536 |
| io.ws_write | 50552.5 | 9.02 | 5604.5 | 2560 | 32768 |
| cdp.handle_text | 26006.7 | 1.02 | 25496.7 | 20480 | 65536 |
| cdp.dispatch Page.navigate | 22900.7 | 1 | 22900.7 | 20480 | 65536 |
| io.to_browser | 18770.9 | 1.02 | 18402.8 | 16384 | 40960 |
| outbox.push | 7706.6 | 9.02 | 854.4 | 80 | 7168 |
| json.serialize | 5100.1 | 9.02 | 565.4 | 384 | 1280 |
| cdp.parse | 1797.6 | 1.02 | 1762.3 | 1536 | 3584 |
| io.ws_decode | 1084.1 | 1.02 | 1062.9 | 640 | 10240 |
| cdp.params | 515 | 1 | 515 | 448 | 1024 |
| cdp.dispatch LP.perfReset | 113.3 | 0.02 | 5667 | 5120 | 5120 |
| host.flush | 51.7 | 1.02 | 50.7 | 40 | 224 |

### page.navigate.scripts [traced]

Timer floor: clock 17.7 ns, one record 58.5 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| io.outbound_queue | 250414.1 | 10.03 | 24958.2 | 16384 | 81920 |
| io.ws_write | 62687.4 | 10.03 | 6247.9 | 3072 | 32768 |
| cdp.handle_text | 49687.4 | 2.03 | 24436.4 | 20480 | 81920 |
| io.to_browser | 28038.8 | 2.03 | 13789.6 | 10240 | 49152 |
| cdp.dispatch Runtime.evaluate | 24633.2 | 1 | 24633.2 | 16384 | 81920 |
| cdp.dispatch Page.navigate | 19409.9 | 1 | 19409.9 | 16384 | 32768 |
| outbox.push | 13042.7 | 10.03 | 1299.9 | 80 | 6144 |
| js.eval | 12665.3 | 1 | 12665.3 | 6144 | 65536 |
| json.serialize | 6383.3 | 10.03 | 636.2 | 512 | 1280 |
| cdp.parse | 4036.4 | 2.03 | 1985.1 | 1536 | 16384 |
| remote.register+reply | 2754 | 1 | 2754 | 2048 | 6144 |
| io.ws_decode | 1613.9 | 2.03 | 793.7 | 448 | 4096 |
| remote.describe | 1600 | 1 | 1600 | 1280 | 5120 |
| cdp.params | 1505.7 | 2 | 752.8 | 640 | 1024 |
| cdp.dispatch LP.perfReset | 166.7 | 0.03 | 5000 | 4096 | 4096 |
| host.flush | 90.2 | 2.03 | 44.3 | 40 | 112 |

### page.navigate.dom2k [traced]

Timer floor: clock 18.5 ns, one record 56.1 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| io.outbound_queue | 293908.2 | 9.03 | 32536 | 28672 | 65536 |
| io.ws_write | 72844.5 | 9.03 | 8064 | 2560 | 32768 |
| cdp.handle_text | 37451.4 | 1.03 | 36243.3 | 32768 | 57344 |
| cdp.dispatch Page.navigate | 32584.7 | 1 | 32584.7 | 28672 | 49152 |
| io.to_browser | 24079.1 | 1.03 | 23302.4 | 20480 | 32768 |
| outbox.push | 11601.7 | 9.03 | 1284.3 | 80 | 8192 |
| json.serialize | 5830.4 | 9.03 | 645.4 | 512 | 1536 |
| cdp.parse | 2870.7 | 1.03 | 2778.1 | 2560 | 3584 |
| io.ws_decode | 1672.4 | 1.03 | 1618.5 | 1280 | 4096 |
| cdp.params | 711.1 | 1 | 711.1 | 640 | 896 |
| cdp.dispatch LP.perfReset | 282 | 0.03 | 8459 | 8192 | 8192 |
| host.flush | 48.6 | 1.03 | 47 | 40 | 160 |

### target.lifecycle [traced]

Timer floor: clock 18.5 ns, one record 55.7 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 1616698.5 | 8.03 | 201248.8 | 20480 | 2097152 |
| cdp.dispatch Target.createTarget | 1401711.2 | 1 | 1401711.2 | 1048576 | 2097152 |
| io.outbound_queue | 118645.9 | 11.03 | 10753.4 | 8192 | 40960 |
| io.to_browser | 87890.5 | 8.03 | 10940.7 | 8192 | 32768 |
| cdp.dispatch Target.closeTarget | 85730.5 | 1 | 85730.5 | 81920 | 131072 |
| io.ws_write | 80151.3 | 11.03 | 7264.5 | 6144 | 24576 |
| cdp.dispatch Target.disposeBrowserContext | 60676.3 | 1 | 60676.3 | 57344 | 98304 |
| cdp.dispatch Target.createBrowserContext | 29993.1 | 1 | 29993.1 | 24576 | 57344 |
| outbox.push | 20118.1 | 11.03 | 1823.4 | 1280 | 8192 |
| cdp.dispatch Page.getFrameTree | 9834.5 | 1 | 9834.5 | 8192 | 28672 |
| cdp.dispatch Runtime.enable | 8501.4 | 1 | 8501.4 | 7168 | 14336 |
| cdp.dispatch Target.attachToTarget | 8026.3 | 1 | 8026.3 | 7168 | 12288 |
| json.serialize | 6047.1 | 11.03 | 548.1 | 320 | 1792 |
| cdp.parse | 4827.9 | 8.03 | 601 | 448 | 2048 |
| io.ws_decode | 4365.5 | 8.03 | 543.4 | 320 | 3584 |
| cdp.dispatch Page.enable | 4291.6 | 1 | 4291.6 | 3584 | 10240 |
| cdp.params | 2409.4 | 5 | 481.9 | 384 | 2560 |
| host.flush | 527.4 | 8.03 | 65.7 | 40 | 256 |
| cdp.dispatch LP.perfReset | 230.6 | 0.03 | 6917 | 6144 | 6144 |

### network.fetch20 [traced]

Timer floor: clock 17.9 ns, one record 61.6 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| io.outbound_queue | 1262182.1 | 81.03 | 15576.1 | 10240 | 65536 |
| io.ws_write | 345937.9 | 81.03 | 4269.1 | 3072 | 16384 |
| cdp.handle_text | 307702.7 | 1.03 | 297776.8 | 262144 | 458752 |
| cdp.dispatch Runtime.evaluate | 304197.2 | 1 | 304197.2 | 262144 | 458752 |
| js.eval | 132151.5 | 1 | 132151.5 | 114688 | 229376 |
| json.serialize | 67820 | 81.03 | 836.9 | 768 | 2560 |
| outbox.push | 60847.5 | 81.03 | 750.9 | 160 | 4096 |
| io.to_browser | 14813.9 | 1.03 | 14336 | 10240 | 40960 |
| cdp.parse | 1540.2 | 1.03 | 1490.5 | 1024 | 2560 |
| remote.register+reply | 1409.7 | 1 | 1409.7 | 1280 | 1792 |
| io.ws_decode | 833.5 | 1.03 | 806.6 | 512 | 4096 |
| cdp.params | 740.3 | 1 | 740.3 | 512 | 1536 |
| remote.describe | 740.3 | 1 | 740.3 | 640 | 1280 |
| host.flush | 487.5 | 1.03 | 471.8 | 112 | 10240 |
| cdp.dispatch LP.perfReset | 158.3 | 0.03 | 4750 | 4096 | 4096 |

### fetch.intercept.fulfill [traced]

Timer floor: clock 18.6 ns, one record 55.1 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 45844.2 | 2.02 | 22695.2 | 20480 | 81920 |
| cdp.dispatch Runtime.evaluate | 36716.6 | 1 | 36716.6 | 28672 | 114688 |
| io.outbound_queue | 27245.1 | 3.02 | 9021.5 | 7168 | 32768 |
| js.eval | 24715.9 | 1 | 24715.9 | 20480 | 81920 |
| io.ws_write | 21261.6 | 3.02 | 7040.3 | 5120 | 24576 |
| io.to_browser | 20793.4 | 2.02 | 10293.8 | 8192 | 32768 |
| cdp.dispatch Fetch.fulfillRequest | 6405.7 | 1 | 6405.7 | 5120 | 24576 |
| outbox.push | 5145.2 | 3.02 | 1703.7 | 1280 | 8192 |
| json.serialize | 1683.3 | 3.02 | 557.4 | 256 | 4096 |
| cdp.parse | 1254.2 | 2.02 | 620.9 | 512 | 1536 |
| cdp.params | 1164.9 | 2 | 582.5 | 384 | 3072 |
| remote.register+reply | 1070 | 1 | 1070 | 896 | 2048 |
| io.ws_decode | 770.8 | 2.02 | 381.6 | 256 | 1280 |
| remote.describe | 671.6 | 1 | 671.6 | 512 | 2048 |
| host.flush | 231.7 | 2.02 | 114.7 | 112 | 448 |
| cdp.dispatch LP.perfReset | 93.3 | 0.02 | 4667 | 4096 | 4096 |

### input.mouse.click [traced]

Timer floor: clock 18.4 ns, one record 56.2 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 46363 | 2 | 23123.7 | 20480 | 57344 |
| cdp.dispatch Input.dispatchMouseEvent | 44219.3 | 2 | 22109.7 | 20480 | 49152 |
| io.to_browser | 19342.8 | 2 | 9647.3 | 8192 | 32768 |
| io.outbound_queue | 12780.8 | 2 | 6374.5 | 4096 | 28672 |
| io.ws_write | 10366.2 | 2 | 5170.2 | 4096 | 14336 |
| outbox.push | 3860.6 | 2 | 1925.5 | 1280 | 8192 |
| cdp.parse | 910.6 | 2 | 454.1 | 384 | 1024 |
| io.ws_decode | 652.5 | 2 | 325.4 | 256 | 640 |
| cdp.params | 523.7 | 2 | 261.8 | 224 | 512 |
| json.serialize | 385.4 | 2 | 192.2 | 160 | 384 |
| host.flush | 93.8 | 2 | 46.8 | 40 | 224 |
| cdp.dispatch LP.perfReset | 23.5 | 0.01 | 4709 | 4096 | 4096 |

### input.key.type [traced]

Timer floor: clock 17.6 ns, one record 54.6 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 23748.8 | 2 | 11844.8 | 10240 | 32768 |
| cdp.dispatch Input.dispatchKeyEvent | 21573.1 | 2 | 10786.6 | 10240 | 28672 |
| io.to_browser | 19870.7 | 2 | 9910.5 | 8192 | 32768 |
| io.outbound_queue | 11435.8 | 2 | 5703.6 | 3584 | 32768 |
| io.ws_write | 11356.5 | 2 | 5664.1 | 4096 | 20480 |
| outbox.push | 4099.4 | 2 | 2044.6 | 1024 | 7168 |
| cdp.parse | 988.8 | 2 | 493.2 | 384 | 1280 |
| io.ws_decode | 717.5 | 2 | 357.8 | 224 | 1280 |
| cdp.params | 591.9 | 2 | 295.9 | 224 | 896 |
| json.serialize | 337.7 | 2 | 168.4 | 160 | 448 |
| host.flush | 113.9 | 2 | 56.8 | 40 | 80 |
| cdp.dispatch LP.perfReset | 27.9 | 0.01 | 5584 | 5120 | 5120 |

### wv.open+navigate+close [traced]

Timer floor: clock 18.1 ns, one record 58 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 2652706.1 | 6.2 | 423074.6 | 16384 | 2097152 |
| cdp.dispatch Target.createTarget | 2314539.6 | 1 | 2314539.6 | 2097152 | 3145728 |
| io.outbound_queue | 347329.3 | 16.05 | 21640.5 | 14336 | 65536 |
| cdp.dispatch Runtime.evaluate | 249958.2 | 1 | 249958.2 | 229376 | 327680 |
| js.eval | 226000 | 1 | 226000 | 196608 | 262144 |
| io.ws_write | 111343.9 | 16.05 | 6937.3 | 4096 | 32768 |
| io.to_browser | 95937.3 | 6.2 | 15320.8 | 12288 | 40960 |
| cdp.dispatch Page.navigate | 26433.3 | 1 | 26433.3 | 24576 | 40960 |
| outbox.push | 21587.7 | 16.35 | 1304.4 | 160 | 8192 |
| cdp.dispatch Target.closeTarget | 15614.5 | 0.15 | 79591.6 | 81920 | 81920 |
| json.serialize | 11676.7 | 17.2 | 678.9 | 512 | 2048 |
| cdp.dispatch Runtime.enable | 9862.5 | 1 | 9862.5 | 8192 | 24576 |
| cdp.dispatch Target.attachToTarget | 9498 | 1 | 9498 | 8192 | 12288 |
| cdp.parse | 7824.6 | 6.2 | 1262 | 1024 | 3072 |
| cdp.dispatch Page.enable | 4991.8 | 1 | 4991.8 | 3584 | 12288 |
| io.ws_decode | 4574.9 | 7.05 | 648.9 | 320 | 4096 |
| cdp.params | 2907.9 | 4.15 | 689.3 | 640 | 1024 |
| remote.register+reply | 2060.5 | 1 | 2060.5 | 1792 | 2560 |
| remote.describe | 1235.5 | 1 | 1235.5 | 1024 | 1536 |
| host.flush | 373.4 | 6.2 | 60.5 | 40 | 112 |
| cdp.dispatch LP.perfReset | 341.7 | 0.05 | 6834 | 6144 | 6144 |

### wv.navigate.simple [traced]

Timer floor: clock 17.8 ns, one record 63.3 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| io.outbound_queue | 259522.5 | 10.02 | 25900.4 | 20480 | 65536 |
| cdp.handle_text | 218218.2 | 2.02 | 108028.8 | 40960 | 262144 |
| cdp.dispatch Runtime.evaluate | 191104.2 | 1 | 191104.2 | 163840 | 262144 |
| js.eval | 180024.1 | 1 | 180024.1 | 163840 | 262144 |
| io.ws_write | 56660.9 | 10.02 | 5654.8 | 2560 | 32768 |
| io.to_browser | 30943.4 | 2.02 | 15318.5 | 12288 | 32768 |
| cdp.dispatch Page.navigate | 21346.6 | 1 | 21346.6 | 20480 | 40960 |
| outbox.push | 10081.8 | 10.02 | 1006.2 | 80 | 6144 |
| json.serialize | 5338.1 | 10.02 | 532.7 | 448 | 1280 |
| cdp.parse | 2861.6 | 2.02 | 1416.6 | 1280 | 2560 |
| remote.register+reply | 1672.4 | 1 | 1672.4 | 1536 | 3072 |
| io.ws_decode | 1395.8 | 2.02 | 691 | 448 | 3584 |
| remote.describe | 1120.8 | 1 | 1120.8 | 896 | 4096 |
| cdp.params | 1055 | 2 | 527.5 | 448 | 896 |
| cdp.dispatch LP.perfReset | 147.5 | 0.02 | 7375 | 7168 | 7168 |
| host.flush | 85.8 | 2.02 | 42.5 | 40 | 80 |

### wv.navigate.scripts [traced]

Timer floor: clock 17.8 ns, one record 58.1 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| io.outbound_queue | 270384.8 | 11.03 | 24506.2 | 16384 | 81920 |
| cdp.handle_text | 104088.8 | 3.03 | 34315 | 28672 | 98304 |
| cdp.dispatch Runtime.evaluate | 76319.5 | 2 | 38159.8 | 32768 | 98304 |
| io.ws_write | 69489.1 | 11.03 | 6298.1 | 3072 | 28672 |
| js.eval | 38954.1 | 2 | 19477.1 | 16384 | 81920 |
| io.to_browser | 34844.6 | 3.03 | 11487.2 | 8192 | 57344 |
| cdp.dispatch Page.navigate | 19858.3 | 1 | 19858.3 | 16384 | 40960 |
| outbox.push | 14337.6 | 11.03 | 1299.5 | 112 | 6144 |
| json.serialize | 6816.8 | 11.03 | 617.8 | 512 | 1536 |
| cdp.parse | 4326.5 | 3.03 | 1426.3 | 768 | 6144 |
| remote.register+reply | 3658.5 | 2 | 1829.3 | 1536 | 4096 |
| cdp.params | 2093.1 | 3 | 697.7 | 512 | 3072 |
| remote.describe | 1887.5 | 2 | 943.7 | 896 | 1792 |
| io.ws_decode | 1852.8 | 3.03 | 610.8 | 384 | 8192 |
| cdp.dispatch LP.perfReset | 145.8 | 0.03 | 4375 | 4096 | 4096 |
| host.flush | 134.5 | 3.03 | 44.3 | 40 | 112 |

### wv.navigate.dom2k [traced]

Timer floor: clock 18.5 ns, one record 59.9 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| io.outbound_queue | 304744.4 | 10.03 | 30373.2 | 28672 | 65536 |
| cdp.handle_text | 297729.2 | 2.03 | 146424.2 | 49152 | 327680 |
| cdp.dispatch Runtime.evaluate | 259481.9 | 1 | 259481.9 | 262144 | 327680 |
| js.eval | 245068.1 | 1 | 245068.1 | 229376 | 262144 |
| io.ws_write | 79850.3 | 10.03 | 7958.5 | 3072 | 28672 |
| io.to_browser | 38048.5 | 2.03 | 18712.4 | 16384 | 40960 |
| cdp.dispatch Page.navigate | 31586.1 | 1 | 31586.1 | 28672 | 49152 |
| outbox.push | 14482.3 | 10.03 | 1443.4 | 80 | 7168 |
| json.serialize | 6694.5 | 10.03 | 667.2 | 512 | 1536 |
| cdp.parse | 3822.3 | 2.03 | 1879.8 | 1792 | 3584 |
| remote.register+reply | 3056.9 | 1 | 3056.9 | 2560 | 7168 |
| io.ws_decode | 2338.9 | 2.03 | 1150.3 | 896 | 6144 |
| cdp.params | 1676.3 | 2 | 838.2 | 768 | 3072 |
| remote.describe | 1648.6 | 1 | 1648.6 | 1280 | 5120 |
| cdp.dispatch LP.perfReset | 163.9 | 0.03 | 4917 | 4096 | 4096 |
| host.flush | 88.9 | 2.03 | 43.7 | 40 | 112 |

### wv.evaluate [traced]

Timer floor: clock 17.8 ns, one record 54.6 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 38795.8 | 1 | 38666.9 | 28672 | 114688 |
| cdp.dispatch Runtime.evaluate | 37561.7 | 1 | 37561.7 | 28672 | 114688 |
| js.eval | 22835.8 | 1 | 22835.8 | 16384 | 65536 |
| io.to_browser | 10518.6 | 1 | 10483.7 | 8192 | 32768 |
| io.outbound_queue | 7457.1 | 1 | 7432.3 | 5120 | 28672 |
| io.ws_write | 6052.1 | 1 | 6032 | 5120 | 14336 |
| outbox.push | 2375.2 | 1 | 2367.3 | 1536 | 8192 |
| remote.register+reply | 910 | 1 | 910 | 768 | 2048 |
| cdp.parse | 501.5 | 1 | 499.8 | 384 | 1792 |
| remote.describe | 364.2 | 1 | 364.2 | 320 | 896 |
| io.ws_decode | 343.4 | 1 | 342.3 | 224 | 1024 |
| json.serialize | 281.8 | 1 | 280.9 | 224 | 640 |
| cdp.params | 222.1 | 1 | 222.1 | 160 | 512 |
| host.flush | 41.4 | 1 | 41.2 | 40 | 80 |
| cdp.dispatch LP.perfReset | 16 | 0 | 4791 | 4096 | 4096 |

### wv.evaluate.query2k [traced]

Timer floor: clock 18.5 ns, one record 74.4 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 289858.3 | 1 | 288416.3 | 262144 | 393216 |
| cdp.dispatch Runtime.evaluate | 288291.5 | 1 | 288291.5 | 262144 | 393216 |
| js.eval | 272443.1 | 1 | 272443.1 | 262144 | 393216 |
| io.to_browser | 12693.1 | 1 | 12630 | 8192 | 40960 |
| io.outbound_queue | 8900 | 1 | 8855.7 | 7168 | 28672 |
| io.ws_write | 7649.6 | 1 | 7611.6 | 6144 | 20480 |
| outbox.push | 2666.2 | 1 | 2652.9 | 2048 | 7168 |
| remote.register+reply | 1305 | 1 | 1305 | 896 | 3584 |
| cdp.params | 751.7 | 1 | 751.7 | 448 | 5120 |
| cdp.parse | 676.9 | 1 | 673.6 | 512 | 2048 |
| io.ws_decode | 495.6 | 1 | 493.1 | 320 | 2560 |
| remote.describe | 440.2 | 1 | 440.2 | 320 | 1280 |
| json.serialize | 409 | 1 | 406.9 | 320 | 896 |
| host.flush | 43.1 | 1 | 42.9 | 40 | 80 |
| cdp.dispatch LP.perfReset | 21.5 | 0.01 | 4292 | 4096 | 4096 |

### wv.cdp.Runtime.evaluate [traced]

Timer floor: clock 18.6 ns, one record 59.8 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| io.to_browser | 9240.6 | 1 | 9210 | 8192 | 24576 |
| cdp.handle_text | 8350.7 | 1 | 8322.9 | 6144 | 28672 |
| cdp.dispatch Runtime.evaluate | 7245.3 | 1 | 7245.3 | 5120 | 24576 |
| io.ws_write | 5882.9 | 1 | 5863.4 | 5120 | 16384 |
| io.outbound_queue | 5794.3 | 1 | 5775 | 4096 | 24576 |
| js.eval | 2606.8 | 1 | 2606.8 | 1792 | 16384 |
| outbox.push | 1590.2 | 1 | 1584.9 | 1024 | 6144 |
| remote.register+reply | 836.2 | 1 | 836.2 | 640 | 1792 |
| cdp.parse | 447.2 | 1 | 445.7 | 320 | 1536 |
| remote.describe | 381.7 | 1 | 381.7 | 320 | 896 |
| io.ws_decode | 306.8 | 1 | 305.8 | 224 | 1280 |
| json.serialize | 293.2 | 1 | 292.2 | 224 | 640 |
| cdp.params | 157 | 1 | 157 | 112 | 320 |
| host.flush | 41 | 1 | 40.8 | 40 | 80 |
| cdp.dispatch LP.perfReset | 18.3 | 0 | 5500 | 5120 | 5120 |

### wv.click.selector [traced]

Timer floor: clock 20.9 ns, one record 69.7 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 770740.1 | 3.01 | 256059.8 | 131072 | 2621440 |
| io.to_browser | 412726.8 | 3.01 | 137118.5 | 32768 | 1310720 |
| io.outbound_queue | 381201.7 | 3.01 | 126645.1 | 24576 | 2621440 |
| cdp.dispatch Input.dispatchMouseEvent | 342179.2 | 2 | 171089.6 | 98304 | 2097152 |
| cdp.dispatch Runtime.evaluate | 340283.3 | 1 | 340283.3 | 196608 | 2621440 |
| js.eval | 274936.7 | 1 | 274936.7 | 131072 | 2097152 |
| io.ws_write | 225609.1 | 3.01 | 74953.2 | 24576 | 655360 |
| remote.describe | 59676.6 | 1 | 59676.6 | 28672 | 196608 |
| outbox.push | 47911.6 | 3.01 | 15917.5 | 4096 | 163840 |
| cdp.params | 18965.5 | 3 | 6321.8 | 3072 | 10240 |
| remote.register+reply | 13532.1 | 1 | 13532.1 | 10240 | 65536 |
| cdp.parse | 12574.5 | 3.01 | 4177.6 | 2560 | 24576 |
| json.serialize | 7051.3 | 3.01 | 2342.6 | 768 | 6144 |
| io.ws_decode | 3707.1 | 3.01 | 1231.6 | 896 | 4096 |
| host.flush | 498.4 | 3.01 | 165.6 | 160 | 384 |
| cdp.dispatch LP.perfReset | 81.7 | 0.01 | 8167 | 7168 | 7168 |

### wv.type [traced]

Timer floor: clock 22.9 ns, one record 69.7 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 13832.9 | 1.01 | 13696 | 10240 | 32768 |
| cdp.dispatch Input.insertText | 12327.9 | 1 | 12327.9 | 10240 | 32768 |
| io.to_browser | 11350.4 | 1.01 | 11238 | 8192 | 28672 |
| io.ws_write | 7022.5 | 1.01 | 6953 | 6144 | 16384 |
| io.outbound_queue | 6966.6 | 1.01 | 6897.7 | 5120 | 20480 |
| outbox.push | 1983.8 | 1.01 | 1964.1 | 1536 | 4096 |
| cdp.parse | 602.1 | 1.01 | 596.1 | 448 | 2048 |
| io.ws_decode | 438.3 | 1.01 | 434 | 320 | 1792 |
| json.serialize | 210.4 | 1.01 | 208.3 | 192 | 768 |
| cdp.params | 116.3 | 1 | 116.3 | 112 | 320 |
| cdp.dispatch LP.perfReset | 59.2 | 0.01 | 5917 | 5120 | 5120 |
| host.flush | 52.9 | 1.01 | 52.4 | 40 | 80 |

### wv.flow.e2e [traced]

Timer floor: clock 21 ns, one record 64.2 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 4471824.9 | 15.2 | 298121.7 | 57344 | 3145728 |
| cdp.dispatch Target.createTarget | 2917452.7 | 1 | 2917452.7 | 2621440 | 3145728 |
| cdp.dispatch Runtime.evaluate | 988100.5 | 5 | 199555.8 | 131072 | 524288 |
| js.eval | 830966.9 | 5 | 167446.5 | 114688 | 458752 |
| io.outbound_queue | 516752.7 | 25.07 | 20670.1 | 14336 | 81920 |
| cdp.dispatch Input.dispatchMouseEvent | 447133.3 | 4 | 111783.3 | 49152 | 524288 |
| io.to_browser | 385857.9 | 15.2 | 25274.5 | 16384 | 114688 |
| io.ws_write | 302080.5 | 25.07 | 12051.1 | 7168 | 40960 |
| remote.describe | 183841.4 | 5 | 36768.3 | 1280 | 196608 |
| outbox.push | 58391.3 | 25.33 | 2304.9 | 1792 | 8192 |
| cdp.dispatch Page.navigate | 28827.7 | 1 | 28827.7 | 24576 | 40960 |
| cdp.parse | 25717 | 15.2 | 1714.5 | 1280 | 5120 |
| json.serialize | 24197.7 | 26.2 | 921.2 | 512 | 3584 |
| cdp.params | 21905.1 | 13.13 | 1659.5 | 1024 | 8192 |
| remote.register+reply | 20966.3 | 5 | 4193.3 | 3072 | 10240 |
| cdp.dispatch Input.insertText | 16850.1 | 1 | 16850.1 | 14336 | 24576 |
| cdp.dispatch Target.closeTarget | 16407 | 0.17 | 98677.3 | 81920 | 98304 |
| cdp.dispatch Runtime.enable | 13183.3 | 1 | 13183.3 | 10240 | 32768 |
| io.ws_decode | 13027.3 | 16.07 | 810.8 | 448 | 3072 |
| cdp.dispatch Target.attachToTarget | 12628.1 | 1 | 12628.1 | 10240 | 20480 |
| cdp.dispatch Page.enable | 4483.3 | 1 | 4483.3 | 4096 | 8192 |
| host.flush | 1138.5 | 15.2 | 74.6 | 40 | 320 |
| cdp.dispatch LP.perfReset | 497.3 | 0.07 | 7459 | 7168 | 7168 |

