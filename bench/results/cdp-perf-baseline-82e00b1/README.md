# CDP performance baseline at 82e00b1

- Date: 2026-10-27 (A/A run started 10:26:40Z, upstream run 10:28:48Z)
- Host: Apple M1 Max, 10 CPUs, 64 GB, macOS 27.0.1 (26A434). Bun 1.4.3, Node v24.16.0.
- Commit: `82e00b1589047f555c92224188545cab5881fbd1`. The binary was built at 3ee8884; 82e00b1 changes only
  `bench/`, so the Rust sources are identical. sha256 of `target/release/lightningpanda`: `a26f6a1f0cc4748b…`
  (full hash in `run.json`).
- Load: the host was busy (other builds). 1-minute load average was 14.6 at start, 15.3 to 18.9 per round and
  16.7 at end (15-minute ≈ 18.5) on 10 CPUs. Absolute wall times are therefore pessimistic; within-run
  comparisons (alternating order) are still fair.

## Files

- `run.json`, `run.md`: **A/A run of one binary** (labels `a` and `b` are the same file). It is the baseline,
  and it also documents run-to-run noise for each scenario.
- `aa-compare.md`: `a` vs `b` compare. 0 regressions, 0 improvements, 3 unclear (sub-0.15 ms round trips
  with high spread), 28 unchanged, 0 failed.
- `vs-upstream.json`, `vs-upstream.md`, `vs-upstream-compare.md`: upstream Lightpanda vs this binary, one
  interleaved run. The upstream binary is the **official nightly `1.1.0-nightly.10661+738f61e00`**
  (sha256 `f1156d20d4ef9b16…`, downloaded from lightpanda-io/browser releases on 2026-10-16), not the
  2ad38773f build in `../upstream-2ad38773f/`.

## Commands

```sh
cargo build --release -p lightningpanda
(cd bench/rss-sampler && cargo build --release)
(cd bench && bun install)
bun bench/scripts/cdp-perf.mjs --bin $PWD/target/release/lightningpanda --out /tmp/cdp-perf/quick.json --quick
bun bench/scripts/cdp-perf.mjs --bin a=$PWD/target/release/lightningpanda --bin b=$PWD/target/release/lightningpanda \
  --rounds 5 --out /tmp/cdp-perf/aa.json
bun bench/scripts/cdp-perf-compare.mjs /tmp/cdp-perf/aa.json --base a --head b --no-fail --md aa-compare.md
bun bench/scripts/cdp-perf.mjs --bin upstream=/path/to/lightpanda --bin rewrite=$PWD/target/release/lightningpanda \
  --rounds 5 --out /tmp/cdp-perf/vs-upstream.json
bun bench/scripts/cdp-perf-compare.mjs /tmp/cdp-perf/vs-upstream.json --base upstream --head rewrite --no-fail --md vs-upstream-compare.md
```

The runs exclude Playwright (it is opt-in with `--playwright`). The 31 scenarios are 21 raw-CDP ones plus 10
Bun.WebView ones, and all passed on both binaries.

## Notes for optimization

- `requestAnimationFrame` here is a 60 Hz timer (about 17 ms between frames; upstream ticks about every 5.7 ms).
  Bun.WebView's selector actionability check waits for two stable frames. That makes `wv.click.selector`
  about 17.7 ms of wall time against 0.7 ms of CPU, and it accounts for about 36 ms of the 47 ms in `wv.flow.e2e`.
- The biggest CPU gaps behind upstream are `dom.mutate.1k` (3.5×), `querySelectorAll` (1.5×),
  `target.lifecycle` (createTarget 1.5 ms vs 0.9 ms) and the navigate reply before parsing starts.
- The rewrite is ahead of upstream on HTML parse (`dom2k` −30%), result serialization, large `callFunctionOn`,
  pipelined throughput and connection setup.
