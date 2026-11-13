# CDP performance: traced

2026-10-27T13:45:33.404Z on Apple M1 Max (10 CPUs, darwin 27.0.0), 3 round(s), scale 1.
Load average at start 18.16/19.78/16.39, at end 17.66/19.65/16.37.

- **traced**: `/Users/xban1x/Povio/pyro-cdp-perf-wt/target-perf/release/pyro` sha256 `dce293f375c99563`, startup median 6.756 ms

Times are milliseconds per sample (median, p90 over all rounds); `rsd` is the robust relative spread; `cpu/op` is browser CPU per operation.

| scenario | traced median | p90 | rsd | ops/s | cpu/op |
|---|---:|---:|---:|---:|---:|
| rtt.Browser.getVersion | 0.047 | 0.111 | 0.46 | 21277 | 0.015 |
| rtt.Runtime.evaluate | 0.042 | 0.070 | 0.25 | 23810 | 0.018 |
| throughput.evaluate.pipelined256 | 2.148 | 3.035 | 0.20 | 119203 | 0.008 |
| runtime.evaluate.largeValue | 0.544 | 0.927 | 0.16 | 1838 | 0.464 |
| runtime.callFunctionOn.small | 0.043 | 0.082 | 0.26 | 23148 | 0.021 |
| runtime.callFunctionOn.38KB | 0.158 | 0.229 | 0.17 | 6345 | 0.088 |
| runtime.handle+release | 0.089 | 0.148 | 0.22 | 11261 | 0.043 |
| dom.querySelectorAll.2k | 0.333 | 0.437 | 0.14 | 3003 | 0.301 |
| dom.textContent.2k | 0.168 | 0.232 | 0.19 | 5963 | 0.157 |
| dom.mutate.1k | 2.886 | 4.546 | 0.21 | 346 | 2.698 |
| dom.handle.describe+quads | 0.246 | 0.547 | 0.31 | 4062 | 0.140 |
| page.navigate.dom2k | 7.824 | 8.648 | 0.07 | 128 | 8.780 |
| input.mouse.click | 0.136 | 0.270 | 0.50 | 7348 | 0.079 |
| input.key.type | 0.089 | 0.137 | 0.19 | 11211 | 0.042 |
| wv.navigate.dom2k | 7.925 | 11.875 | 0.09 | 126 | 8.959 |

## Phase breakdown (median ms since the step started)

- page.navigate.dom2k [traced]: navigateReply 2.431, frameNavigated 2.452, domContentEventFired 7.784, loadEventFired 7.814

## Inner CDP stages (perf-trace)

Inclusive nanoseconds per benchmark operation (stages nest: `cdp.handle_text` contains `cdp.parse` and `cdp.dispatch ...`, which contains `js.*`, `remote.*`, `json.serialize`, `outbox.push`). Stages under 10 ns/op are left out. `io.*` stages are the socket I/O, done on the browser thread itself; `io.outbound_queue` is how long a frame waited for the socket.

### rtt.Browser.getVersion [traced]

Timer floor: clock 18.6 ns, one record 58.7 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| io.ws_read | 4914.5 | 1 | 4904.7 | 4096 | 14336 |
| io.ws_write | 3585.4 | 1 | 3578.3 | 2048 | 12288 |
| cdp.handle_text | 1789.4 | 1 | 1785.8 | 1280 | 8192 |
| cdp.dispatch Browser.getVersion | 880.1 | 1 | 880.1 | 512 | 3072 |
| json.serialize | 506.4 | 1 | 505.4 | 320 | 1280 |
| io.outbound_queue | 497.4 | 1 | 496.4 | 320 | 1280 |
| cdp.parse | 264.7 | 1 | 264.2 | 192 | 768 |
| io.ws_decode | 252.9 | 1 | 252.4 | 192 | 640 |
| outbox.push | 147.4 | 1 | 147.1 | 80 | 640 |
| host.flush | 32.7 | 1 | 32.6 | 40 | 80 |

### rtt.Runtime.evaluate [traced]

Timer floor: clock 18.7 ns, one record 60.7 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 5502 | 1 | 5491 | 4096 | 16384 |
| cdp.dispatch Runtime.evaluate | 4474.4 | 1 | 4474.4 | 3072 | 14336 |
| io.ws_read | 4230.6 | 1 | 4222.2 | 3584 | 14336 |
| io.ws_write | 3933 | 1 | 3925.1 | 2560 | 12288 |
| js.eval | 2544.3 | 1 | 2544.3 | 1792 | 10240 |
| io.outbound_queue | 513.7 | 1 | 512.6 | 320 | 1280 |
| remote.register+reply | 433.7 | 1 | 433.7 | 384 | 1280 |
| cdp.parse | 399.8 | 1 | 399 | 320 | 1024 |
| remote.describe | 351.7 | 1 | 351.7 | 320 | 512 |
| json.serialize | 242 | 1 | 241.5 | 192 | 448 |
| io.ws_decode | 233.4 | 1 | 233 | 192 | 512 |
| cdp.params | 153.5 | 1 | 153.5 | 112 | 448 |
| outbox.push | 141.4 | 1 | 141.1 | 80 | 640 |
| host.flush | 40.2 | 1 | 40.1 | 40 | 80 |

### throughput.evaluate.pipelined256 [traced]

Timer floor: clock 17.9 ns, one record 59.2 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| io.ws_write | 3977.6 | 1 | 3976.8 | 3072 | 14336 |
| cdp.handle_text | 3418.6 | 1 | 3418 | 2048 | 16384 |
| cdp.dispatch Runtime.evaluate | 2834.1 | 1 | 2834.1 | 1792 | 16384 |
| js.eval | 1628.4 | 1 | 1628.4 | 640 | 8192 |
| io.outbound_queue | 424.5 | 1 | 424.5 | 320 | 1536 |
| remote.register+reply | 316.7 | 1 | 316.7 | 224 | 768 |
| io.ws_read | 271.3 | 0.04 | 6615.4 | 4096 | 40960 |
| cdp.parse | 245.5 | 1 | 245.4 | 192 | 640 |
| remote.describe | 233.6 | 1 | 233.6 | 192 | 512 |
| json.serialize | 193.4 | 1 | 193.4 | 160 | 448 |
| cdp.params | 91.7 | 1 | 91.7 | 80 | 256 |
| outbox.push | 75.2 | 1 | 75.1 | 40 | 512 |
| host.flush | 50.5 | 1 | 50.5 | 40 | 80 |
| io.ws_decode | 47.9 | 1 | 47.9 | 40 | 160 |

### runtime.evaluate.largeValue [traced]

Timer floor: clock 18.4 ns, one record 56.2 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 353412.1 | 1.01 | 349913 | 327680 | 655360 |
| cdp.dispatch Runtime.evaluate | 351027.1 | 1 | 351027.1 | 327680 | 655360 |
| remote.describe | 251506.2 | 1 | 251506.2 | 229376 | 458752 |
| js.eval | 93040.5 | 1 | 93040.5 | 65536 | 393216 |
| io.ws_write | 14318.8 | 1.01 | 14177 | 10240 | 57344 |
| io.ws_read | 6208.7 | 1.01 | 6147.3 | 4096 | 24576 |
| remote.register+reply | 2442.1 | 1 | 2442.1 | 1792 | 7168 |
| json.serialize | 1852.9 | 1.01 | 1834.5 | 1280 | 5120 |
| io.outbound_queue | 1257 | 1.01 | 1244.6 | 1024 | 4096 |
| cdp.parse | 1170.9 | 1.01 | 1159.3 | 896 | 3072 |
| cdp.params | 931.7 | 1 | 931.7 | 768 | 2048 |
| io.ws_decode | 618.4 | 1.01 | 612.2 | 384 | 5120 |
| outbox.push | 315.8 | 1.01 | 312.7 | 320 | 768 |
| host.flush | 39.6 | 1.01 | 39.2 | 40 | 80 |
| cdp.dispatch Pyro.perfReset | 30 | 0.01 | 3000 | 2560 | 2560 |

### runtime.callFunctionOn.small [traced]

Timer floor: clock 17.9 ns, one record 56 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 6470.1 | 1 | 6448.6 | 4096 | 32768 |
| io.ws_read | 5097.9 | 1 | 5081 | 4096 | 20480 |
| cdp.dispatch Runtime.callFunctionOn | 4699 | 1 | 4699 | 3072 | 24576 |
| io.ws_write | 4630.8 | 1 | 4615.4 | 3072 | 28672 |
| js.call_function | 1320.7 | 1 | 1320.7 | 768 | 14336 |
| cdp.parse | 784.2 | 1 | 781.6 | 448 | 6144 |
| io.outbound_queue | 618.5 | 1 | 616.4 | 384 | 1280 |
| remote.register+reply | 540.6 | 1 | 540.6 | 320 | 1792 |
| cdp.params | 486.3 | 1 | 486.3 | 256 | 2048 |
| remote.describe | 362.6 | 1 | 362.6 | 320 | 1024 |
| json.serialize | 320 | 1 | 319 | 224 | 1024 |
| io.ws_decode | 273.3 | 1 | 272.4 | 192 | 768 |
| outbox.push | 164.8 | 1 | 164.3 | 80 | 512 |
| host.flush | 39.8 | 1 | 39.7 | 40 | 80 |
| cdp.dispatch Pyro.perfReset | 11.1 | 0 | 3333 | 3072 | 3072 |

### runtime.callFunctionOn.38KB [traced]

Timer floor: clock 18.5 ns, one record 56 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 66365 | 1 | 66034.9 | 57344 | 114688 |
| cdp.dispatch Runtime.callFunctionOn | 40297.9 | 1 | 40297.9 | 32768 | 81920 |
| cdp.params | 29050.6 | 1 | 29050.6 | 24576 | 65536 |
| cdp.parse | 25355.6 | 1 | 25229.4 | 20480 | 49152 |
| io.ws_read | 8518.7 | 1 | 8476.3 | 6144 | 28672 |
| js.call_function | 7959.8 | 1 | 7959.8 | 6144 | 28672 |
| io.ws_write | 5941.9 | 1 | 5912.3 | 3584 | 28672 |
| io.ws_decode | 4363.6 | 1 | 4341.9 | 3072 | 14336 |
| io.outbound_queue | 703.3 | 1 | 699.8 | 448 | 2048 |
| remote.register+reply | 509.2 | 1 | 509.2 | 384 | 1536 |
| remote.describe | 422.9 | 1 | 422.9 | 320 | 1280 |
| json.serialize | 322.7 | 1 | 321.1 | 224 | 1024 |
| outbox.push | 198.6 | 1 | 197.6 | 80 | 640 |
| host.flush | 41.9 | 1 | 41.6 | 40 | 80 |
| cdp.dispatch Pyro.perfReset | 16.9 | 0.01 | 3375 | 3072 | 3072 |

### runtime.handle+release [traced]

Timer floor: clock 18.5 ns, one record 59.2 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 15118.2 | 2 | 7546.5 | 6144 | 32768 |
| cdp.dispatch Runtime.evaluate | 11118.9 | 1 | 11118.9 | 8192 | 40960 |
| io.ws_read | 9846 | 2 | 4914.8 | 4096 | 20480 |
| io.ws_write | 9065.1 | 2 | 4525 | 3072 | 20480 |
| js.eval | 7906.8 | 1 | 7906.8 | 5120 | 28672 |
| cdp.dispatch Runtime.releaseObject | 1642.8 | 1 | 1642.8 | 1024 | 8192 |
| io.outbound_queue | 1193.1 | 2 | 595.6 | 384 | 1536 |
| remote.register+reply | 1103.5 | 1 | 1103.5 | 768 | 3072 |
| cdp.parse | 816.2 | 2 | 407.4 | 320 | 1024 |
| json.serialize | 810 | 2 | 404.3 | 320 | 1024 |
| remote.describe | 747.9 | 1 | 747.9 | 512 | 2560 |
| io.ws_decode | 537.6 | 2 | 268.4 | 192 | 896 |
| cdp.params | 392.6 | 2 | 196.3 | 112 | 640 |
| outbox.push | 292.5 | 2 | 146 | 80 | 512 |
| host.flush | 97.5 | 2 | 48.7 | 40 | 80 |

### dom.querySelectorAll.2k [traced]

Timer floor: clock 17.9 ns, one record 57.1 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 298413.3 | 1 | 296928.7 | 262144 | 524288 |
| cdp.dispatch Runtime.evaluate | 296512.2 | 1 | 296512.2 | 262144 | 524288 |
| js.eval | 292540.2 | 1 | 292540.2 | 262144 | 524288 |
| io.ws_write | 7385.9 | 1 | 7349.1 | 5120 | 28672 |
| io.ws_read | 5837.9 | 1 | 5808.9 | 4096 | 28672 |
| io.outbound_queue | 945.2 | 1 | 940.5 | 640 | 2560 |
| cdp.parse | 802.7 | 1 | 798.7 | 512 | 3072 |
| remote.register+reply | 745.4 | 1 | 745.4 | 512 | 3072 |
| cdp.params | 730.8 | 1 | 730.8 | 448 | 3072 |
| remote.describe | 504.8 | 1 | 504.8 | 320 | 1792 |
| io.ws_decode | 489.8 | 1 | 487.4 | 256 | 1536 |
| json.serialize | 442.9 | 1 | 440.7 | 320 | 1536 |
| outbox.push | 212.3 | 1 | 211.3 | 112 | 512 |
| host.flush | 38.1 | 1 | 37.9 | 40 | 40 |
| cdp.dispatch Pyro.perfReset | 19 | 0.01 | 3792 | 3584 | 3584 |

### dom.textContent.2k [traced]

Timer floor: clock 18.8 ns, one record 56 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 129358.8 | 1.01 | 128078.1 | 98304 | 262144 |
| cdp.dispatch Runtime.evaluate | 127573.3 | 1 | 127573.3 | 98304 | 262144 |
| js.eval | 123819.7 | 1 | 123819.7 | 98304 | 229376 |
| io.ws_write | 6392.1 | 1.01 | 6328.8 | 4096 | 20480 |
| io.ws_read | 5012.9 | 1.01 | 4963.3 | 3584 | 16384 |
| remote.register+reply | 970.4 | 1 | 970.4 | 640 | 3072 |
| io.outbound_queue | 868.3 | 1.01 | 859.7 | 512 | 1792 |
| cdp.parse | 839.2 | 1.01 | 830.9 | 448 | 7168 |
| json.serialize | 724.2 | 1.01 | 717 | 512 | 2048 |
| remote.describe | 587.1 | 1 | 587.1 | 320 | 1792 |
| io.ws_decode | 404.3 | 1.01 | 400.2 | 256 | 2048 |
| outbox.push | 212.1 | 1.01 | 210 | 80 | 512 |
| cdp.params | 203.3 | 1 | 203.3 | 160 | 640 |
| host.flush | 37 | 1.01 | 36.7 | 40 | 80 |
| cdp.dispatch Pyro.perfReset | 30 | 0.01 | 3000 | 2560 | 2560 |

### dom.mutate.1k [traced]

Timer floor: clock 18.5 ns, one record 63.9 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 2901336.7 | 1.02 | 2844447.7 | 2621440 | 5242880 |
| cdp.dispatch Runtime.evaluate | 2895524.1 | 1 | 2895524.1 | 2621440 | 5242880 |
| js.eval | 2771248.2 | 1 | 2771248.2 | 2097152 | 5242880 |
| io.ws_write | 23658.3 | 1.02 | 23194.5 | 20480 | 49152 |
| io.ws_read | 8253.4 | 1.02 | 8091.5 | 6144 | 40960 |
| cdp.parse | 3200.8 | 1.02 | 3138 | 2560 | 14336 |
| io.outbound_queue | 2816.6 | 1.02 | 2761.4 | 2048 | 6144 |
| cdp.params | 2534.2 | 1 | 2534.2 | 2048 | 8192 |
| remote.register+reply | 2037.4 | 1 | 2037.4 | 1792 | 6144 |
| json.serialize | 1275.1 | 1.02 | 1250.1 | 1024 | 5120 |
| io.ws_decode | 1198.3 | 1.02 | 1174.8 | 640 | 16384 |
| remote.describe | 1156.6 | 1 | 1156.6 | 1024 | 2560 |
| outbox.push | 339.2 | 1.02 | 332.5 | 256 | 1024 |
| cdp.dispatch Pyro.perfReset | 98.3 | 0.02 | 4917 | 4096 | 4096 |
| host.flush | 43.3 | 1.02 | 42.5 | 40 | 80 |

### dom.handle.describe+quads [traced]

Timer floor: clock 18.5 ns, one record 56.2 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 112544.8 | 4 | 28101.1 | 12288 | 98304 |
| io.ws_write | 32017.5 | 4 | 7994.4 | 4096 | 32768 |
| cdp.dispatch Runtime.evaluate | 30150 | 1 | 30150 | 16384 | 229376 |
| cdp.dispatch DOM.describeNode | 29002.5 | 1 | 29002.5 | 12288 | 49152 |
| cdp.dispatch DOM.getContentQuads | 27251.9 | 1 | 27251.9 | 20480 | 81920 |
| js.eval | 24566.9 | 1 | 24566.9 | 12288 | 131072 |
| io.ws_read | 21872.2 | 4 | 5461.2 | 4096 | 20480 |
| io.outbound_queue | 3880 | 4 | 968.8 | 512 | 2560 |
| json.serialize | 3822 | 4 | 954.3 | 640 | 3072 |
| cdp.dispatch Runtime.releaseObject | 2183 | 1 | 2183 | 1024 | 8192 |
| cdp.parse | 2133.6 | 4 | 532.7 | 320 | 2560 |
| remote.describe | 1975.8 | 1 | 1975.8 | 768 | 12288 |
| remote.register+reply | 1460.2 | 1 | 1460.2 | 1024 | 4096 |
| io.ws_decode | 1212.3 | 4 | 302.7 | 192 | 896 |
| cdp.params | 1105 | 5 | 221 | 112 | 768 |
| outbox.push | 767.6 | 4 | 191.7 | 80 | 640 |
| host.flush | 192.2 | 4 | 48 | 40 | 80 |
| cdp.dispatch Pyro.perfReset | 23.3 | 0.01 | 4666 | 4096 | 4096 |

### page.navigate.dom2k [traced]

Timer floor: clock 18.3 ns, one record 60.1 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 1463745.8 | 1.03 | 1416528.2 | 1048576 | 2621440 |
| cdp.dispatch Page.navigate | 1456082 | 1 | 1456082 | 1048576 | 2621440 |
| io.outbound_queue | 144063.7 | 9.03 | 15948 | 12288 | 49152 |
| io.ws_write | 66436.1 | 9.03 | 7354.5 | 2560 | 32768 |
| io.ws_read | 8316.7 | 1.03 | 8048.5 | 7168 | 24576 |
| json.serialize | 6668 | 9.03 | 738.2 | 640 | 2560 |
| cdp.parse | 4548.6 | 1.03 | 4401.9 | 3072 | 28672 |
| io.ws_decode | 1398.6 | 1.03 | 1353.5 | 768 | 4096 |
| outbox.push | 1238.7 | 9.03 | 137.1 | 80 | 512 |
| cdp.params | 791.7 | 1 | 791.7 | 640 | 1536 |
| cdp.dispatch Pyro.perfReset | 137.5 | 0.03 | 4125 | 4096 | 4096 |
| host.flush | 38.8 | 1.03 | 37.6 | 40 | 80 |

### input.mouse.click [traced]

Timer floor: clock 18.6 ns, one record 56.1 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 44452.9 | 2 | 22171 | 16384 | 65536 |
| cdp.dispatch Input.dispatchMouseEvent | 41391.3 | 2 | 20695.6 | 16384 | 65536 |
| io.ws_write | 10444.5 | 2 | 5209.2 | 4096 | 20480 |
| io.ws_read | 10306.8 | 2 | 5140.5 | 5120 | 16384 |
| cdp.parse | 1183.3 | 2 | 590.2 | 384 | 2560 |
| io.outbound_queue | 926.3 | 2 | 462 | 320 | 1280 |
| cdp.params | 550.9 | 2 | 275.4 | 224 | 1024 |
| io.ws_decode | 478.6 | 2 | 238.7 | 192 | 768 |
| json.serialize | 347.7 | 2 | 173.4 | 160 | 768 |
| outbox.push | 222.3 | 2 | 110.9 | 80 | 512 |
| host.flush | 78.9 | 2 | 39.4 | 40 | 80 |
| cdp.dispatch Pyro.perfReset | 17.5 | 0.01 | 3500 | 3072 | 3072 |

### input.key.type [traced]

Timer floor: clock 18.2 ns, one record 53.7 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 17096.7 | 2 | 8527 | 8192 | 24576 |
| cdp.dispatch Input.dispatchKeyEvent | 14933.7 | 2 | 7466.9 | 7168 | 24576 |
| io.ws_read | 9675 | 2 | 4825.4 | 4096 | 24576 |
| io.ws_write | 8793.9 | 2 | 4386 | 3072 | 16384 |
| io.outbound_queue | 1160.2 | 2 | 578.7 | 320 | 2560 |
| cdp.parse | 959.6 | 2 | 478.6 | 384 | 1280 |
| cdp.params | 589.6 | 2 | 294.8 | 224 | 640 |
| io.ws_decode | 451.9 | 2 | 225.4 | 192 | 640 |
| json.serialize | 326.3 | 2 | 162.7 | 160 | 448 |
| outbox.push | 247.8 | 2 | 123.6 | 80 | 512 |
| host.flush | 79.6 | 2 | 39.7 | 40 | 80 |
| cdp.dispatch Pyro.perfReset | 17.5 | 0.01 | 3500 | 3072 | 3072 |

### wv.navigate.dom2k [traced]

Timer floor: clock 17.8 ns, one record 56.9 ns.

| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |
|---|---:|---:|---:|---:|---:|
| cdp.handle_text | 2002690.3 | 2.03 | 984929.7 | 917504 | 3670016 |
| cdp.dispatch Page.navigate | 1488630.6 | 1 | 1488630.6 | 1048576 | 3670016 |
| cdp.dispatch Runtime.evaluate | 509502.8 | 1 | 509502.8 | 327680 | 1310720 |
| js.eval | 327243 | 1 | 327243 | 327680 | 393216 |
| io.outbound_queue | 125156.8 | 10.03 | 12474.1 | 10240 | 40960 |
| io.ws_write | 65143 | 10.03 | 6492.7 | 2560 | 28672 |
| io.ws_read | 15341.6 | 2.03 | 7545.1 | 6144 | 40960 |
| json.serialize | 7015.4 | 10.03 | 699.2 | 512 | 2048 |
| cdp.parse | 5386 | 2.03 | 2648.9 | 2048 | 6144 |
| remote.register+reply | 2048.7 | 1 | 2048.7 | 1792 | 5120 |
| remote.describe | 1962.5 | 1 | 1962.5 | 1536 | 5120 |
| cdp.params | 1677.8 | 2 | 838.9 | 768 | 1280 |
| io.ws_decode | 1641.5 | 2.03 | 807.3 | 512 | 2048 |
| outbox.push | 1342.6 | 10.03 | 133.8 | 80 | 512 |
| cdp.dispatch Pyro.perfReset | 100 | 0.03 | 3000 | 2560 | 2560 |
| host.flush | 82 | 2.03 | 40.3 | 40 | 80 |

