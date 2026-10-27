# lp-tos run output (full)

```
cdp-perf: warning: 1-minute load average 12.7 on 10 CPUs; expect noisy timings
  wv.flow.e2e                        [head] round 1/5: median 14.219 ms
  lp.flow.e2e                        [head] round 1/5: median 6.750 ms
  lp.run.flow                        [head] round 1/5: median 7.622 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 1/5: median 11.904 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 1/5: median 12.590 ms
      lightningpanda-test pages: {"prepared":2,"preparedBooting":0,"fresh":15}
  tos.close.run                      [head] round 1/5: median 11.171 ms
  pw.tos.close                       [head] round 1/5: median 46.263 ms
  wv.flow.e2e                        [head] round 2/5: median 13.418 ms
  lp.flow.e2e                        [head] round 2/5: median 6.613 ms
  lp.run.flow                        [head] round 2/5: median 6.686 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 2/5: median 12.936 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 2/5: median 12.833 ms
      lightningpanda-test pages: {"prepared":4,"preparedBooting":0,"fresh":13}
  tos.close.run                      [head] round 2/5: median 11.065 ms
  pw.tos.close                       [head] round 2/5: median 45.675 ms
  wv.flow.e2e                        [head] round 3/5: median 14.524 ms
  lp.flow.e2e                        [head] round 3/5: median 6.729 ms
  lp.run.flow                        [head] round 3/5: median 7.054 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 3/5: median 15.309 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 3/5: median 11.216 ms
      lightningpanda-test pages: {"prepared":2,"preparedBooting":0,"fresh":15}
  tos.close.run                      [head] round 3/5: median 11.112 ms
  pw.tos.close                       [head] round 3/5: median 39.726 ms
  wv.flow.e2e                        [head] round 4/5: median 13.037 ms
  lp.flow.e2e                        [head] round 4/5: median 7.921 ms
  lp.run.flow                        [head] round 4/5: median 6.723 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 4/5: median 11.292 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 4/5: median 10.993 ms
      lightningpanda-test pages: {"prepared":4,"preparedBooting":0,"fresh":13}
  tos.close.run                      [head] round 4/5: median 10.237 ms
  pw.tos.close                       [head] round 4/5: median 45.618 ms
  wv.flow.e2e                        [head] round 5/5: median 12.595 ms
  lp.flow.e2e                        [head] round 5/5: median 6.363 ms
  lp.run.flow                        [head] round 5/5: median 6.817 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 5/5: median 12.012 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 5/5: median 11.822 ms
      lightningpanda-test pages: {"prepared":2,"preparedBooting":0,"fresh":15}
  tos.close.run                      [head] round 5/5: median 10.938 ms
  pw.tos.close                       [head] round 5/5: median 43.601 ms
# CDP performance: head

2026-10-27T17:24:07.296Z on Apple M1 Max (10 CPUs, darwin 27.0.0), 5 round(s), scale 1.
Load average at start 12.70/9.62/8.07, at end 13.75/10.00/8.23.

- **head**: `/Users/xban1x/Povio/lp-driver-wt/target/release/lightningpanda` sha256 `9a0fdd328151656a`, startup median 7.758 ms

Times are milliseconds per sample (median, p90 over all rounds); `rsd` is the robust relative spread; `cpu/op` is browser CPU per operation.

| scenario | head median | p90 | rsd | ops/s | cpu/op |
|---|---:|---:|---:|---:|---:|
| wv.flow.e2e | 13.373 | 15.409 | 0.09 | 75 | 13.833 |
| lp.flow.e2e | 6.750 | 9.027 | 0.10 | 148 | 7.575 |
| lp.run.flow | 6.925 | 9.781 | 0.16 | 144 | 8.114 |
| tos.close.steps | 12.242 | 16.394 | 0.16 | 82 | 12.303 |
| tos.close.run.fresh | 11.943 | 14.481 | 0.16 | 84 | 12.266 |
| tos.close.run | 11.079 | 13.316 | 0.15 | 90 | 11.983 |
| pw.tos.close | 43.697 | 56.649 | 0.12 | 23 | 34.248 |

## Phase breakdown (median ms since the step started)

- wv.flow.e2e [head]: navigate 6.007, input 10.457, result 13.321, close 13.364
- lp.flow.e2e [head]: navigate 5.001, input 5.686, result 6.190, close 6.737
- lp.run.flow [head]: page 2.537, result 6.439, close 6.920
- tos.close.steps [head]: seed 0.284, page 3.294, open 9.275, run 12.215, close 12.235
- tos.close.run.fresh [head]: seed 0.278, page 3.204, run 11.913, close 11.937
- tos.close.run [head]: seed 0.265, page 2.797, run 11.046, close 11.073
- pw.tos.close [head]: seed 0.305, page 4.968, open 21.387, run 42.806, close 43.694
```

# Follow-up run (d58d909)

```
cdp-perf: warning: 1-minute load average 12.1 on 10 CPUs; expect noisy timings
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 1/5: median 10.565 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 1/5: median 10.051 ms
      lightningpanda-test pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run                      [head] round 1/5: median 4.784 ms
      lightningpanda-test pages: {"prepared":2,"preparedBooting":0,"fresh":15}
  tos.close.run.busy                 [head] round 1/5: median 10.766 ms
  pw.tos.close                       [head] round 1/5: median 40.664 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 2/5: median 11.982 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 2/5: median 10.393 ms
      lightningpanda-test pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run                      [head] round 2/5: median 2.941 ms
      lightningpanda-test pages: {"prepared":2,"preparedBooting":0,"fresh":15}
  tos.close.run.busy                 [head] round 2/5: median 10.089 ms
  pw.tos.close                       [head] round 2/5: median 42.679 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 3/5: median 11.528 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 3/5: median 9.761 ms
      lightningpanda-test pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run                      [head] round 3/5: median 4.029 ms
      lightningpanda-test pages: {"prepared":2,"preparedBooting":0,"fresh":15}
  tos.close.run.busy                 [head] round 3/5: median 10.960 ms
  pw.tos.close                       [head] round 3/5: median 39.284 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 4/5: median 10.487 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 4/5: median 10.270 ms
      lightningpanda-test pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run                      [head] round 4/5: median 2.948 ms
      lightningpanda-test pages: {"prepared":2,"preparedBooting":0,"fresh":15}
  tos.close.run.busy                 [head] round 4/5: median 10.268 ms
  pw.tos.close                       [head] round 4/5: median 38.764 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.steps                    [head] round 5/5: median 10.401 ms
      lightningpanda-test pages: {"prepared":0,"preparedBooting":0,"fresh":17}
  tos.close.run.fresh                [head] round 5/5: median 9.422 ms
      lightningpanda-test pages: {"prepared":17,"preparedBooting":0,"fresh":0}
  tos.close.run                      [head] round 5/5: median 4.015 ms
      lightningpanda-test pages: {"prepared":2,"preparedBooting":0,"fresh":15}
  tos.close.run.busy                 [head] round 5/5: median 11.538 ms
  pw.tos.close                       [head] round 5/5: median 40.121 ms
# CDP performance: head

2026-10-27T17:26:49.912Z on Apple M1 Max (10 CPUs, darwin 27.0.0), 5 round(s), scale 1.
Load average at start 12.12/11.40/9.13, at end 11.91/11.39/9.15.

- **head**: `/Users/xban1x/Povio/lp-driver-wt/target/release/lightningpanda` sha256 `9a0fdd328151656a`, startup median 8.187 ms

Times are milliseconds per sample (median, p90 over all rounds); `rsd` is the robust relative spread; `cpu/op` is browser CPU per operation.

| scenario | head median | p90 | rsd | ops/s | cpu/op |
|---|---:|---:|---:|---:|---:|
| tos.close.steps | 11.044 | 13.031 | 0.11 | 91 | 11.620 |
| tos.close.run.fresh | 10.031 | 11.461 | 0.10 | 100 | 11.179 |
| tos.close.run | 4.055 | 6.922 | 0.50 | 247 | 11.507 |
| tos.close.run.busy | 10.707 | 12.630 | 0.13 | 93 | 11.763 |
| pw.tos.close | 40.134 | 47.843 | 0.07 | 25 | 32.115 |

## Phase breakdown (median ms since the step started)

- tos.close.steps [head]: seed 0.263, page 2.670, open 8.198, run 11.004, close 11.038
- tos.close.run.fresh [head]: seed 0.263, page 2.667, run 9.976, close 9.994
- tos.close.run [head]: seed 0.292, page 0.726, run 4.033, close 4.048
- tos.close.run.busy [head]: seed 0.267, page 2.823, run 10.682, close 10.703
- pw.tos.close [head]: seed 0.280, page 4.226, open 19.259, run 39.094, close 40.130
```
