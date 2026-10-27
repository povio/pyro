# TOS positions-close: lightningpanda-test vs Playwright

One run, one binary, 5 rounds, TOS's "closes an open Position from the Position overview" against the
TOS-shaped app in `cdp-perf-site.mjs`, 2026-10-27T17:24:07Z.
Host: Apple M1 Max, 10 CPUs, macOS 27.0.1; Load average at start 12.70/9.62/8.07, at end 13.75/10.00/8.23
(very busy machine: the script warns above 5, so treat differences under ~10-15% as noise).

- **head**: c984988 (LP.run ready signed-in pages, route/press/eval/request steps, lightningpanda-test client),
  release build; cpu/op from `bench/rss-sampler`. The bench script carries one fix on top: `tos.close.steps`
  sends each `check.request` in the same LP.run as the action that sent the request (a request step only sees
  its own run's log, as documented), instead of as a run of its own, which timed out.

```sh
cargo build --release -p lightningpanda
(cd bench/rss-sampler && cargo build --release)
(cd bench && bun install)
bun bench/scripts/cdp-perf.mjs --bin head=$PWD/target/release/lightningpanda --filter '^((pw\.)?tos\.|lp\.run\.flow|lp\.flow\.e2e|wv\.flow\.e2e)' --playwright --rounds 5 --out /tmp/lp-tos.json
```

Full table and per-round output: [run.md](run.md).

| scenario | median ms | p90 | cpu/op | seed | page | open | run | close |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| tos.close.steps (one LP.run per step) | 12.242 | 16.394 | 12.303 | 0.284 | 3.294 | 9.275 | 12.215 | 12.235 |
| tos.close.run.fresh (one LP.run, fresh page + goto) | 11.943 | 14.481 | 12.266 | 0.278 | 3.204 | | 11.913 | 11.937 |
| tos.close.run (one LP.run, prepared page + route) | 11.079 | 13.316 | 11.983 | 0.265 | 2.797 | | 11.046 | 11.073 |
| pw.tos.close (Playwright over CDP) | 43.697 | 56.649 | 34.248 | 0.305 | 4.968 | 21.387 | 42.806 | 43.694 |

Reference flows in the same run (form flow): lp.flow.e2e 6.750 ms, lp.run.flow 6.925 ms, wv.flow.e2e 13.373 ms.

## Client page stats (per round, 17 pages: 2 warmup + 15 samples)

| scenario | prepared | preparedBooting | fresh |
|---|---:|---:|---:|
| tos.close.steps | 0 | 0 | 17 |
| tos.close.run.fresh | 0 | 0 | 17 |
| tos.close.run | 2 / 4 / 2 / 4 / 2 | 0 | 15 / 13 / 15 / 13 / 15 |

## Interpretation

The whole TOS close flow (seed, signed-in page, Settings menu, confirm dialog, POST + GET checks, closed banner)
takes ~11-12 ms through lightningpanda-test, about 4x faster than the same flow driven by Playwright (43.7 ms) and
at roughly a third of its browser cpu/op. Playwright's cost is spread over both halves: 16 ms to open the page
(goto + waiting for the overview) and 21 ms for the clicks and response waits. Splitting the flow into one LP.run
per step costs only ~0.3 ms over one LP.run, so round trips are not where the time goes.

The prepared-page result is not representative yet: the client got a pre-booted page for only 2-4 of 17 pages per
round. The server boots a replacement only after the connection has been idle for `REFILL_QUIET` (15 ms,
`serve.rs`), and this benchmark issues the next acquire within a millisecond of closing the previous page, so the
pool drains after its first two pages and every later page falls back to a fresh one. The ~0.9 ms gap between
tos.close.run and tos.close.run.fresh is therefore mostly the route-vs-goto difference on the few prepared pages
plus noise. A real test suite with gaps between tests (or a larger pool, or a bench pause between samples) would
see the prepared path; measuring that needs a scenario that waits for `preparedPages` to report ready pages.
