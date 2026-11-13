# TOS positions-close: pyro vs Playwright

One run, one binary, 5 rounds, TOS's "closes an open Position from the Position overview" against the
TOS-shaped app in `cdp-perf-site.mjs`, 2026-10-27T17:24:07Z.
Host: Apple M1 Max, 10 CPUs, macOS 27.0.1; Load average at start 12.70/9.62/8.07, at end 13.75/10.00/8.23
(very busy machine: the script warns above 5, so treat differences under ~10-15% as noise).

- **head**: c984988 (Pyro.run ready signed-in pages, route/press/eval/request steps, pyro client),
  release build; cpu/op from `bench/rss-sampler`. The bench script carries one fix on top: `tos.close.steps`
  sends each `check.request` in the same Pyro.run as the action that sent the request (a request step only sees
  its own run's log, as documented), instead of as a run of its own, which timed out.

```sh
cargo build --release -p pyro
(cd bench/rss-sampler && cargo build --release)
(cd bench && bun install)
bun bench/scripts/cdp-perf.mjs --bin head=$PWD/target/release/pyro --filter '^((pw\.)?tos\.|pyro\.run\.flow|pyro\.flow\.e2e|wv\.flow\.e2e)' --playwright --rounds 5 --out /tmp/pyro-tos.json
```

Full table and per-round output: [run.md](run.md).

| scenario | median ms | p90 | cpu/op | seed | page | open | run | close |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| tos.close.steps (one Pyro.run per step) | 12.242 | 16.394 | 12.303 | 0.284 | 3.294 | 9.275 | 12.215 | 12.235 |
| tos.close.run.fresh (one Pyro.run, fresh page + goto) | 11.943 | 14.481 | 12.266 | 0.278 | 3.204 | | 11.913 | 11.937 |
| tos.close.run (one Pyro.run, prepared page + route) | 11.079 | 13.316 | 11.983 | 0.265 | 2.797 | | 11.046 | 11.073 |
| pw.tos.close (Playwright over CDP) | 43.697 | 56.649 | 34.248 | 0.305 | 4.968 | 21.387 | 42.806 | 43.694 |

Reference flows in the same run (form flow): pyro.flow.e2e 6.750 ms, pyro.run.flow 6.925 ms, wv.flow.e2e 13.373 ms.

## Client page stats (per round, 17 pages: 2 warmup + 15 samples)

| scenario | prepared | preparedBooting | fresh |
|---|---:|---:|---:|
| tos.close.steps | 0 | 0 | 17 |
| tos.close.run.fresh | 0 | 0 | 17 |
| tos.close.run | 2 / 4 / 2 / 4 / 2 | 0 | 15 / 13 / 15 / 13 / 15 |

## Interpretation

The whole TOS close flow (seed, signed-in page, Settings menu, confirm dialog, POST + GET checks, closed banner)
takes ~11-12 ms through pyro, about 4x faster than the same flow driven by Playwright (43.7 ms) and
at roughly a third of its browser cpu/op. Playwright's cost is spread over both halves: 16 ms to open the page
(goto + waiting for the overview) and 21 ms for the clicks and response waits. Splitting the flow into one Pyro.run
per step costs only ~0.3 ms over one Pyro.run, so round trips are not where the time goes.

The prepared-page result is not representative yet: the client got a pre-booted page for only 2-4 of 17 pages per
round. The server boots a replacement only after the connection has been idle for `REFILL_QUIET` (15 ms,
`serve.rs`), and this benchmark issues the next acquire within a millisecond of closing the previous page, so the
pool drains after its first two pages and every later page falls back to a fresh one. The ~0.9 ms gap between
tos.close.run and tos.close.run.fresh is therefore mostly the route-vs-goto difference on the few prepared pages
plus noise. A real test suite with gaps between tests (or a larger pool, or a bench pause between samples) would
see the prepared path; measuring that needs a scenario that waits for `preparedPages` to report ready pages.

## Follow-up: tos.close.run on a booted page (d58d909)

Same binary (sha256 `9a0fdd328151656a`), bench JS and client at d58d909: the harness has an untimed per-sample
`before` hook, `tos.close.run` waits for a booted page before each sample (`pyro.warm()`), and `tos.close.run.busy`
is the earlier back-to-back variant. 5 rounds, 2026-10-27T17:26Z.
Load average at start 12.12/11.40/9.13, at end 11.91/11.39/9.15 (still very busy).

```sh
bun bench/scripts/cdp-perf.mjs --bin head=$PWD/target/release/pyro --filter '^(pw\.)?tos\.' --playwright --rounds 5 --out /tmp/pyro-tos2.json
```

| scenario | median ms | p90 | cpu/op | seed | page | open | run | close |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| tos.close.steps | 11.044 | 13.031 | 11.620 | 0.263 | 2.670 | 8.198 | 11.004 | 11.038 |
| tos.close.run.fresh | 10.031 | 11.461 | 11.179 | 0.263 | 2.667 | | 9.976 | 9.994 |
| tos.close.run (booted page) | 4.055 | 6.922 | 11.507 | 0.292 | 0.726 | | 4.033 | 4.048 |
| tos.close.run.busy (back-to-back) | 10.707 | 12.630 | 11.763 | 0.267 | 2.823 | | 10.682 | 10.703 |
| pw.tos.close | 40.134 | 47.843 | 32.115 | 0.280 | 4.226 | 19.259 | 39.094 | 40.130 |

Client page stats, identical in all 5 rounds (17 pages each):

| scenario | prepared | preparedBooting | fresh |
|---|---:|---:|---:|
| tos.close.steps | 0 | 0 | 17 |
| tos.close.run.fresh | 0 | 0 | 17 |
| tos.close.run | 17 | 0 | 0 |
| tos.close.run.busy | 2 | 0 | 15 |

With a booted, signed-in page waiting, the whole close flow takes 4.06 ms: acquiring the page is 0.46 ms
(page 0.73 minus seed) instead of ~2.4 ms, and the route change plus all steps take 3.3 ms instead of ~7.3 ms
for a fresh page that has to navigate and boot the app. That is ~2.5x faster than the fresh path and ~10x faster
than Playwright (40.1 ms). The spread is wide (rsd 0.50, p90 6.9 ms) on this loaded host. cpu/op stays ~11.5
because the booting of the replacement page in the untimed `before` hook is still browser CPU charged to the
operation: the work moves out of the test's wall time, it does not disappear. tos.close.run.busy reproduces the
first run (2 prepared, 15 fresh, 10.7 ms), confirming that back-to-back acquires outrun the 15 ms refill quiet
period.
