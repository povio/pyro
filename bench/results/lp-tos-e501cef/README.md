# TOS positions-close: refill on close, four connections (e501cef)

One run, one binary, 5 rounds, TOS's "closes an open Position from the Position overview" against the
TOS-shaped app in `cdp-perf-site.mjs`, 2026-10-27T17:55:26Z.
Host: Apple M1 Max, 10 CPUs, macOS 27.0.1; Load average at start 12.96/7.73/7.08, at end 12.57/7.82/7.12
(very busy machine: the script warns above 5, so treat differences under ~10-15% as noise).

- **head**: e501cef (the serve loop boots a taken page's replacement as soon as the connection has no open page,
  instead of only after 15 ms without commands; lightning-panda can open several connections and takes each
  page from the one with the fewest open pages), release build, sha256 `2262a26b724c5493`; cpu/op from
  `bench/rss-sampler`. `cargo test -p lp-cdp`: 212 passed, 0 failed.

```sh
cargo build --release -p lightningpanda
(cd bench/rss-sampler && cargo build --release)
(cd bench && bun install)
bun bench/scripts/cdp-perf.mjs --bin head=$PWD/target/release/lightningpanda --filter '^(pw\.)?tos\.' --playwright --rounds 5 --out /tmp/lp-tos3.json
```

Full table and per-round output: [run.md](run.md).

| scenario | median ms | p90 | cpu/op | seed | page | open | run | close | d58d909 median |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| tos.close.steps | 9.174 | 11.123 | 10.448 | 0.239 | 2.367 | 6.987 | 9.152 | 9.168 | 11.044 |
| tos.close.run.fresh | 8.647 | 9.979 | 10.289 | 0.245 | 2.362 | | 8.619 | 8.642 | 10.031 |
| tos.close.run (booted page) | 6.801 | 8.687 | 10.546 | 0.231 | 2.330 | | 6.759 | 6.797 | 4.055 |
| tos.close.run.busy (back-to-back, 1 connection) | 7.366 | 9.442 | 10.537 | 0.231 | 2.796 | | 7.333 | 7.359 | 10.707 |
| tos.close.run.conn4 (back-to-back, 4 connections) | 2.748 | 3.442 | 13.391 | 0.251 | 0.412 | | 2.728 | 2.744 | new |
| pw.tos.close (Playwright over CDP) | 34.157 | 36.982 | 27.787 | 0.246 | 3.494 | 16.652 | 33.449 | 34.153 | 40.134 |

## Client page stats (per round, 17 pages: 2 warmup + 15 samples)

Identical in all 5 rounds:

| scenario | prepared | preparedBooting | fresh |
|---|---:|---:|---:|
| tos.close.steps | 0 | 0 | 17 |
| tos.close.run.fresh | 0 | 0 | 17 |
| tos.close.run | 17 | 0 | 0 |
| tos.close.run.busy | 17 | 0 | 0 |
| tos.close.run.conn4 | 17 | 0 | 0 |

## Interpretation

The refill rule works: back to back on one connection (tos.close.run.busy) every page now comes from the pool
(17 prepared, was 2 prepared + 15 fresh), and none was still booting when taken. Over four connections the whole
close flow takes 2.75 ms (page acquire 0.16 ms after seed), ~12x faster than Playwright (34.2 ms) and ~3x faster
than a fresh page (8.6 ms). cpu/op is higher there (13.4) because four browser threads boot replacements in parallel.

On one connection, though, a prepared page alone is not enough: tos.close.run.busy (7.37 ms) is only ~15% faster
than the fresh path, and tos.close.run, which waits for a booted page before each sample, got slower than at
d58d909 (6.80 vs 4.06 ms; page phase 2.33 vs 0.73). Cause: the pool is 2 and `warm()` returns as soon as one
page is ready. With the new rule the replacement for the page the previous sample closed starts booting right
away, on the same browser thread, so the next sample's acquire and steps queue behind that boot. Before e501cef
the refill waited for 15 ms of quiet, which the warm hook's status polls did not break, so it usually happened
outside the timed sample. Checked with an uncommitted change that makes `warm()` wait until the whole pool (2 pages)
is ready on each connection: tos.close.run then takes 2.48 ms (page 0.45, p90 2.81; 3 rounds, see run.md). So a
booted page on an idle thread costs ~2.5 ms for the whole flow, and the four-connection run gets the same by
letting a refill run on one thread while the next test runs on another.

Everything else is faster than in the d58d909 run by ~10-15% across the board (fresh paths and Playwright alike),
which matches the lower 5- and 15-minute load averages rather than a code change.
