# CDP performance: head

2026-10-27T15:35:31.550Z on Apple M1 Max (10 CPUs, darwin 27.0.0), 5 round(s), scale 1.
Load average at start 12.70/12.07/10.28, at end 12.63/12.08/10.31.

- **head**: `/Users/xban1x/Povio/pyro-driver-wt/target/release/pyro` sha256 `08ad7da73521f64b`, startup median 7.432 ms

Times are milliseconds per sample (median, p90 over all rounds); `rsd` is the robust relative spread; `cpu/op` is browser CPU per operation.

| scenario | head median | p90 | rsd | ops/s | cpu/op |
|---|---:|---:|---:|---:|---:|
| wv.open+navigate+close | 4.778 | 5.757 | 0.11 | 209 | 10.129 |
| wv.navigate.simple | 2.078 | 2.930 | 0.14 | 481 | 3.092 |
| wv.navigate.scripts | 5.502 | 6.670 | 0.14 | 182 | 6.032 |
| wv.navigate.dom2k | 8.341 | 9.739 | 0.06 | 120 | 9.389 |
| wv.evaluate | 0.109 | 0.166 | 0.42 | 9158 | 0.063 |
| wv.evaluate.query2k | 0.380 | 0.544 | 0.16 | 2632 | 0.327 |
| wv.cdp.Runtime.evaluate | 0.086 | 0.131 | 0.23 | 11669 | 0.022 |
| wv.click.selector | 1.699 | 1.879 | 0.07 | 589 | 0.277 |
| wv.type | 0.088 | 0.138 | 0.18 | 11338 | 0.027 |
| wv.flow.e2e | 12.599 | 13.331 | 0.04 | 79 | 12.556 |
| wv.pyro.click | 0.136 | 0.221 | 0.22 | 7358 | 0.070 |
| wv.pyro.fill | 0.096 | 0.152 | 0.25 | 10471 | 0.033 |
| wv.pyro.flow.e2e | 6.185 | 7.071 | 0.08 | 162 | 11.317 |
| pyro.flow.e2e | 6.013 | 7.341 | 0.14 | 166 | 7.103 |
| pyro.flow.pipelined | 5.806 | 7.001 | 0.12 | 172 | 7.050 |
| wv.pyro.flow.role | 6.173 | 6.713 | 0.07 | 162 | 11.146 |
| wv.pyro.locate.css | 0.350 | 0.442 | 0.12 | 2854 | 0.278 |
| wv.pyro.locate.testId | 0.190 | 0.250 | 0.14 | 5274 | 0.120 |
| wv.pyro.locate.text | 1.677 | 1.937 | 0.06 | 596 | 1.555 |
| wv.pyro.locate.role | 0.863 | 1.056 | 0.12 | 1159 | 0.783 |

## Phase breakdown (median ms since the step started)

- wv.flow.e2e [head]: navigate 5.117, input 9.612, result 12.552, close 12.595
- wv.pyro.flow.e2e [head]: navigate 5.130, input 5.768, result 6.118, close 6.179
- pyro.flow.e2e [head]: navigate 4.563, input 5.079, result 5.532, close 6.007
- pyro.flow.pipelined [head]: navigate 4.520, input 4.552, result 5.359, close 5.800
- wv.pyro.flow.role [head]: navigate 5.078, input 5.745, result 6.144, close 6.168
