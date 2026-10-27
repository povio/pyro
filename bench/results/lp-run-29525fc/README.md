# LP.run: a whole scenario in one command

One run, one binary, 5 rounds, the form flow in every driver variant, 2026-10-27T16:12:50Z.
Host: Apple M1 Max, 10 CPUs, macOS 27.0.1; Load average at start 9.13/8.76/8.60, at end 8.64/8.66/8.56
(busy machine: the script warns above 5, so treat differences under ~10% as noise).

- **head**: 29525fc (LP.run) plus the step-index fix for unknown step kinds, release build; cpu/op from `bench/rss-sampler`.

```sh
cargo build --release -p lightningpanda
(cd bench/rss-sampler && cargo build --release)
bun bench/scripts/cdp-perf.mjs --bin head=$PWD/target/release/lightningpanda --filter '^(wv\.flow\.e2e|wv\.lp\.flow\.e2e|wv\.lp\.run\.flow|lp\.flow\.e2e|lp\.flow\.pipelined|lp\.run\.steps|lp\.run\.flow)$' --rounds 5 --out /tmp/lp-run.json
```

Full table and phase breakdowns: [run.md](run.md).

| flow | median ms | p90 | cpu/op | page | navigate | input | result | close |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| wv.flow.e2e (evaluate + poll) | 12.249 | 13.443 | 12.329 | | 5.119 | 9.371 | 12.189 | 12.239 |
| wv.lp.flow.e2e | 6.175 | 7.061 | 11.204 | | 5.059 | 5.755 | 6.133 | 6.169 |
| wv.lp.run.flow | 6.218 | 7.473 | 11.366 | | 5.133 | | 6.184 | 6.214 |
| lp.flow.e2e (raw CDP) | 6.240 | 7.411 | 7.226 | | 4.675 | 5.341 | 5.822 | 6.236 |
| lp.flow.pipelined (raw CDP) | 6.128 | 7.125 | 7.202 | | 4.670 | 4.691 | 5.622 | 6.123 |
| lp.run.steps (raw CDP) | 6.083 | 7.433 | 7.180 | 2.210 | 4.600 | | 5.603 | 6.077 |
| lp.run.flow (raw CDP, goto step) | 5.845 | 6.765 | 7.121 | 2.194 | | | 5.337 | 5.841 |

LP.run lands where pipelining already was: fill, click and expect as one command (lp.run.steps, 6.08 ms)
matches lp.flow.pipelined (6.13 ms), and folding the navigation in as a `goto` step (lp.run.flow) is the
fastest flow at 5.85 ms, ~0.4 ms under sequential lp.flow.e2e, though that is within the noise on this
machine. Every LP variant still halves wv.flow.e2e (12.25 ms). Through Bun.WebView, LP.run buys nothing over
separate LP calls (6.22 vs 6.18 ms): the WebView's own navigate and cdp() overhead dominates, and its
browser-side cpu/op (~11.3) stays well above the raw-CDP flows (~7.2). The remaining cost is mostly page
creation and navigation (~4.6 ms of ~5.8), not the driver steps.
