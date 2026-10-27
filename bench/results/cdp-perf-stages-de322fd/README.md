# Inner CDP stages and profile at de322fd

- Date: 2026-10-27, from 10:41Z. Host: Apple M1 Max, 10 CPUs, 64 GB, macOS 27.0.1 (26A434), Bun 1.4.3.
- Source: f9baf98 (perf-trace timers) plus de322fd (compile fix: the `LP.*` arms returned `()`). The plain
  binary (`target/`, sha256 `0c9413a63e236452…`) and the traced binary (`target-perf/`, `--features
  perf-trace`, sha256 `2a62c32ca3f66da0…`) were each built in their own target directory.
- Load: busy host. 1-minute load average was 9.3 → 8.3 during the overhead run and 8.0 → 7.1 during the
  traced run, and it rose to 64 near the end of profiling (other jobs on the Mac).
- Checks: `cargo test -p lp-cdp --features perf-trace --lib perf::` 2/2 passed, `cargo test -p lp-cdp
  --test integration` 127/127 passed, and both release builds produced no warnings. Clippy is not installed
  for this toolchain, so it was not run.

## Files

| file | what |
|---|---|
| `overhead-run.*`, `overhead-compare.md` | plain vs traced binary, interleaved, 3 rounds, no `--perf-trace`: what the timers cost |
| `stages.*` | traced binary with `--perf-trace`, 3 rounds: inclusive ns per operation for every stage ≥ 10 ns |
| `profile-summary.md` | where browser CPU goes per command (xctrace Time Profiler on the plain binary), hot spots, waste |
| `profile-buckets.md` | raw bucket and top-self output per profiled workload |
| `tools/` | load driver, xctrace wrapper and analyzers used for the profiles |

## Timer overhead

The plain vs traced compare shows 0 regressions, 7 unclear and 24 unchanged scenarios. The timer floor
measured in-process is 18–25 ns per clock pair and 54–67 ns per recorded stage. A simple command records
about 12–15 stages, so tracing adds roughly 0.8–1 µs per command. That is below the run-to-run noise of
the ~50 µs round trips, but it is visible in the stage numbers, so compare traced builds only with traced builds.

## Commands

```sh
cargo build --release -p lightningpanda
CARGO_TARGET_DIR=$PWD/target-perf cargo build --release -p lightningpanda --features perf-trace
bun bench/scripts/cdp-perf.mjs --bin plain=$PWD/target/release/lightningpanda \
  --bin traced=$PWD/target-perf/release/lightningpanda --rounds 3 --out /tmp/cdp-perf/overhead.json
bun bench/scripts/cdp-perf-compare.mjs /tmp/cdp-perf/overhead.json --base plain --head traced --no-fail --md overhead-compare.md
bun bench/scripts/cdp-perf.mjs --bin traced=$PWD/target-perf/release/lightningpanda --perf-trace --rounds 3 \
  --out /tmp/cdp-perf/stages.json
# profiles (Xcode's xctrace; DEVELOPER_DIR avoids changing xcode-select)
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer ROOT=$PWD OUT=/tmp/lp-prof
for w in getVersion evaluate cfo38 qsa mutate lifecycle navSimple; do
  PROF=xctrace bench/results/cdp-perf-stages-de322fd/tools/prof.sh $w 10
  xctrace export --input $OUT/prof/$w.trace \
    --xpath '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]' > $OUT/prof/$w.xml
  bun bench/results/cdp-perf-stages-de322fd/tools/xcat2.mjs $OUT/prof/$w.xml <cpu µs per op> 15
done
```

I profiled with xctrace instead of `sample`: on a 2.9 s CPU window, `sample` caught only ~180 busy samples,
too few for sub-µs attribution.

## Most promising optimizations

| # | change | evidence | expected gain |
|---|---|---|---|
| 1 | Do socket I/O on the browser thread for its connection, or at least write replies directly from the browser thread and post to it only while it sleeps (flag before the pipe write) | 11–15 µs CPU and ~15 µs of hop latency per command (`io.to_browser` 8–10 µs, `io.outbound_queue` ~5 µs) | −40–50% CPU and −25–35% latency on every small command (getVersion, evaluate, input, describe), plus more pipelined throughput |
| 2 | Cache compiled `callFunctionOn` functions per realm by declaration (fast hash + length), pass strings to JSC without the `encode_utf16` collect, and deserialize `Runtime.callFunctionOn` params in one pass | 38 KB call: JSC hash 45 + link 25 + UTF-16 22 + isolatedCopy 4 + double JSON scan ~25 µs | ≈ −110 µs of 238 µs (−45%) on Playwright-style utility calls |
| 3 | Cut JSC C-API lock churn in bindings: cache the realm per context instead of `JSContextGetGlobalContext` per call, avoid `JSValueProtect` for short-lived wrappers, and batch `drain_finalized` so it does not drain microtasks on every `JSWeakRelease` | `JSLockHolder` self 0.49 ms of 3.43 ms; protect/unprotect 0.23 ms; realm_for_ctx 93 µs; finalizer drain 204 µs | −20–30% on DOM-heavy JS (`dom.mutate.1k` 3.4 → ~2.5 ms), helping upstream parity (1.0 ms) |
| 4 | Make realm creation cheaper: install bindings without per-member JS calls (or snapshot/lazy interfaces), and do not arm the JSC watchdog for internal setup calls | ~0.8–1 ms of JS installs per realm, DFG recompiles, watchdog `thread_selfusage` 80 µs per realm | −0.5–1 ms per `createTarget` and per navigation (now 1.4 ms and ~1.5–2.6 ms) |
| 5 | Skip the per-task checkpoint work when nothing changed (dirty flags for frames and finalizers) and serialize by-value results directly to JSON | checkpoint 1.2–3 µs per command; largeValue describe+register 650 µs vs eval 99 µs | −1–3 µs per command; −0.4–0.5 ms per 1000-object by-value result |

Separately, `requestAnimationFrame` ticks every ~17 ms against upstream's ~5.7 ms
(`../cdp-perf-baseline-82e00b1/README.md`). It costs no CPU but dominates Bun.WebView click latency.
