# Pyro.* native test driver: first run

One run, one binary, 5 rounds, every `wv.*` scenario plus the new `wv.pyro.*` and `pyro.*` ones, 2026-10-27T15:35:31Z.
Host: Apple M1 Max, 10 CPUs, macOS 27.0.1; Load average at start 12.70/12.07/10.28, at end 12.63/12.08/10.31
(busy machine: the script warns above 5, so treat differences under ~10% as noise).

- **head**: 055b1a7 (Pyro.act / Pyro.expect / Pyro.query prototype), release build; cpu/op from `bench/rss-sampler`.

```sh
cargo build --release -p pyro
(cd bench/rss-sampler && cargo build --release)
bun bench/scripts/cdp-perf.mjs --bin head=$PWD/target/release/pyro --filter '^(wv|pyro)\.' --rounds 5 --out /tmp/pyro-driver.json
```

Full table and phase breakdowns: [run.md](run.md).

`wv.pyro.flow.pipelined` could not run as written: Bun.WebView's `view.cdp()` allows one call in flight and
rejected the second with `Invalid state: a cdp() is already pending` in every round. It is replaced by
`pyro.flow.e2e` and `pyro.flow.pipelined`, the same form flow (CSS locators) on the script's raw CDP client,
sequential and with fill, click and expect sent together.

| flow | median ms | p90 | cpu/op | navigate | input | result | close |
|---|---:|---:|---:|---:|---:|---:|---:|
| wv.flow.e2e (evaluate + poll) | 12.599 | 13.331 | 12.556 | 5.117 | 9.612 | 12.552 | 12.595 |
| wv.pyro.flow.e2e | 6.185 | 7.071 | 11.317 | 5.130 | 5.768 | 6.118 | 6.179 |
| wv.pyro.flow.role | 6.173 | 6.713 | 11.146 | 5.078 | 5.745 | 6.144 | 6.168 |
| pyro.flow.e2e (raw CDP) | 6.013 | 7.341 | 7.103 | 4.563 | 5.079 | 5.532 | 6.007 |
| pyro.flow.pipelined (raw CDP) | 5.806 | 7.001 | 7.050 | 4.520 | 4.552 | 5.359 | 5.800 |

The Pyro flow halves wv.flow.e2e: input + result go from ~7.4 ms to ~1.0 ms after navigation, because the
browser answers Pyro.expect when the DOM changes instead of the client polling. Pipelining saves another
~0.2 ms (two round trips). Semantic locators (label / role / text) cost the same as CSS ids on this page.
