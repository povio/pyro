# CDP performance: head

2026-10-27T16:12:50.413Z on Apple M1 Max (10 CPUs, darwin 27.0.0), 5 round(s), scale 1.
Load average at start 9.13/8.76/8.60, at end 8.64/8.66/8.56.

- **head**: `/Users/xban1x/Povio/pyro-driver-wt/target/release/pyro` sha256 `061d54f2d0779474`, startup median 7.043 ms

Times are milliseconds per sample (median, p90 over all rounds); `rsd` is the robust relative spread; `cpu/op` is browser CPU per operation.

| scenario | head median | p90 | rsd | ops/s | cpu/op |
|---|---:|---:|---:|---:|---:|
| wv.flow.e2e | 12.249 | 13.443 | 0.06 | 82 | 12.329 |
| wv.pyro.flow.e2e | 6.175 | 7.061 | 0.08 | 162 | 11.204 |
| pyro.flow.e2e | 6.240 | 7.411 | 0.10 | 160 | 7.226 |
| pyro.flow.pipelined | 6.128 | 7.125 | 0.11 | 163 | 7.202 |
| pyro.run.steps | 6.083 | 7.433 | 0.13 | 164 | 7.180 |
| pyro.run.flow | 5.845 | 6.765 | 0.10 | 171 | 7.121 |
| wv.pyro.run.flow | 6.218 | 7.473 | 0.07 | 161 | 11.366 |

## Phase breakdown (median ms since the step started)

- wv.flow.e2e [head]: navigate 5.119, input 9.371, result 12.189, close 12.239
- wv.pyro.flow.e2e [head]: navigate 5.059, input 5.755, result 6.133, close 6.169
- pyro.flow.e2e [head]: navigate 4.675, input 5.341, result 5.822, close 6.236
- pyro.flow.pipelined [head]: navigate 4.670, input 4.691, result 5.622, close 6.123
- pyro.run.steps [head]: page 2.210, navigate 4.600, result 5.603, close 6.077
- pyro.run.flow [head]: page 2.194, result 5.337, close 5.841
- wv.pyro.run.flow [head]: navigate 5.133, result 6.184, close 6.214
