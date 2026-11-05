# lp-tos run output (full)

```
load start: { 12.96 7.73 7.08 } 2026-10-27T17:55:25Z
cdp-perf: warning: 1-minute load average 13.0 on 10 CPUs; expect noisy timings
      lightning-panda pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 1/5: median 9.473 ms
      lightning-panda pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 1/5: median 8.677 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run                      [head] round 1/5: median 6.801 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run.busy                 [head] round 1/5: median 7.504 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run.conn4                [head] round 1/5: median 3.242 ms
  pw.tos.close                       [head] round 1/5: median 35.832 ms
      lightning-panda pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 2/5: median 8.833 ms
      lightning-panda pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 2/5: median 8.362 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run                      [head] round 2/5: median 6.518 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run.busy                 [head] round 2/5: median 7.678 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run.conn4                [head] round 2/5: median 2.566 ms
  pw.tos.close                       [head] round 2/5: median 34.706 ms
      lightning-panda pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 3/5: median 9.009 ms
      lightning-panda pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 3/5: median 8.846 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run                      [head] round 3/5: median 6.816 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run.busy                 [head] round 3/5: median 7.320 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run.conn4                [head] round 3/5: median 2.938 ms
  pw.tos.close                       [head] round 3/5: median 33.992 ms
      lightning-panda pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 4/5: median 8.845 ms
      lightning-panda pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 4/5: median 8.401 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run                      [head] round 4/5: median 6.818 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run.busy                 [head] round 4/5: median 7.090 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run.conn4                [head] round 4/5: median 2.802 ms
  pw.tos.close                       [head] round 4/5: median 33.609 ms
      lightning-panda pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 5/5: median 8.954 ms
      lightning-panda pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 5/5: median 8.921 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run                      [head] round 5/5: median 6.907 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run.busy                 [head] round 5/5: median 7.167 ms
      lightning-panda pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run.conn4                [head] round 5/5: median 2.573 ms
  pw.tos.close                       [head] round 5/5: median 33.666 ms
# CDP performance: head

2026-10-27T17:55:26.060Z on Apple M1 Max (10 CPUs, darwin 27.0.0), 5 round(s), scale 1.
Load average at start 12.96/7.73/7.08, at end 12.57/7.82/7.12.

- **head**: `/Users/xban1x/Povio/lp-driver-wt/target/release/lightningpanda` sha256 `2262a26b724c5493`, startup median 7.360 ms

Times are milliseconds per sample (median, p90 over all rounds); `rsd` is the robust relative spread; `cpu/op` is browser CPU per operation.

| scenario | head median | p90 | rsd | ops/s | cpu/op |
|---|---:|---:|---:|---:|---:|
| tos.close.steps | 9.174 | 11.123 | 0.10 | 109 | 10.448 |
| tos.close.run.fresh | 8.647 | 9.979 | 0.09 | 116 | 10.289 |
| tos.close.run | 6.801 | 8.687 | 0.15 | 147 | 10.546 |
| tos.close.run.busy | 7.366 | 9.442 | 0.09 | 136 | 10.537 |
| tos.close.run.conn4 | 2.748 | 3.442 | 0.22 | 364 | 13.391 |
| pw.tos.close | 34.157 | 36.982 | 0.04 | 29 | 27.787 |

## Phase breakdown (median ms since the step started)

- tos.close.steps [head]: seed 0.239, page 2.367, open 6.987, run 9.152, close 9.168
- tos.close.run.fresh [head]: seed 0.245, page 2.362, run 8.619, close 8.642
- tos.close.run [head]: seed 0.231, page 2.330, run 6.759, close 6.797
- tos.close.run.busy [head]: seed 0.231, page 2.796, run 7.333, close 7.359
- tos.close.run.conn4 [head]: seed 0.251, page 0.412, run 2.728, close 2.744
- pw.tos.close [head]: seed 0.246, page 3.494, open 16.652, run 33.449, close 34.153
exit 0
load end: { 12.57 7.82 7.12 } 2026-10-27T17:55:38Z
```

## Experiment (not committed): warm() waits for the full pool

`Lightningpanda.warm()` changed to return only when `pool` (2) pages of the profile are ready on each connection, so no replacement is booting when the timed sample starts. 3 rounds, load at start 9.70/7.63/7.08.

```
tos.close.run rounds: 2.505 / 2.483 / 2.355 ms; pages {"prepared":17,"preparedBooting":0,"fresh":0} each round
| tos.close.run | 2.483 | 2.814 | 0.09 | 403 | 10.443 |
- tos.close.run [head]: seed 0.298, page 0.450, run 2.460, close 2.477
```
