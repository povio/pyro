# Lazy interface installation, final PR #37 head: affected-host comparison, 2026-10-17

All six matched runs passed. On this host, candidate `0e52072` used 45.8% less browser CPU than parent `4168ac9`
for 50 navigations, and 60.8% less for 20 context cycles. Every candidate sample used less CPU than every parent
sample. This result comes from 3 samples per build on a limited local HTTP workload. It is not a statistical
confidence claim, a full-browser result or a TOS benchmark.

This compares **two Rust rewrite builds**; neither is Zig upstream Lightpanda. The harness's historical labels
map as follows:
- `upstream` = parent `4168ac99eb68cab56fc9f733acde8cc8ecf9f218`;
- `rewrite` = candidate `0e52072bcb476469790d18a56c6e44fc2a70eae4`. This is PR #37's head after `08ad9e2` →
  `9c9b853` → `0e52072`, which adds disposed-realm consistency, member installation before `preventExtensions`,
  and disposed-realm interfaces keeping the eager API.

**This is a separate measurement from [`../lazy-interfaces-08ad9e2/`](../lazy-interfaces-08ad9e2/README.md).**
Both runs used the same parent binary and harness on the same host, but they ran at different times. Their
candidate numbers differ: navigation CPU was 249.3 ms then and 261.4 ms now. With 3 samples each and uncontrolled
CPU frequency, that difference is not evidence of a regression. No regression or improvement between the two
candidates is claimed.

If PR #37's final merged head differs from `0e52072`, these numbers apply only to `0e52072`. Whether the merged
production code is equivalent must be shown separately, for example with `bench/scripts/macho-section-hash.py`
on a fresh build. That check is subject to its own limits.

## Results

| Median of 3 samples | Parent 4168ac9 | Candidate 0e52072 | Change |
| --- | ---: | ---: | ---: |
| 50 navigations, wall ms | 390.07 | 250.98 | −35.7% |
| 50 navigations, browser CPU ms | 481.93 | 261.38 | −45.8% |
| 20 context cycles, wall ms | 284.66 | 154.10 | −45.9% |
| 20 context cycles, browser CPU ms | 318.13 | 124.73 | −60.8% |
| 10-second idle, browser CPU ms | 1.37 | 1.51 | negligible for both |
| 2-second retention, browser CPU ms | 0.76 | 0.82 | negligible for both |
| Sampled peak resident KiB | 20,320 | 18,688 | |
| Sampled peak physical footprint KiB | 9,633 | 8,417 | |
| Retained resident KiB | 20,224 | 18,592 | |
| Retained physical footprint KiB | 7,297 | 7,329 | flat |

| Sample | Build | Navigation CPU ms | Context CPU ms |
| --- | --- | ---: | ---: |
| 1 | Parent | 466.7 | 321.5 |
| 1 | Candidate | 261.4 | 123.3 |
| 2 | Candidate | 265.1 | 124.8 |
| 2 | Parent | 481.9 | 315.9 |
| 3 | Parent | 491.3 | 318.1 |
| 3 | Candidate | 261.0 | 124.7 |

Medians are computed per column. Browser CPU includes all threads of the browser process. Sampled peaks can miss
short spikes, and retained RSS is not a live-heap figure.

## Setup and provenance

- **Host:** Apple M1, 8 GiB, macOS 26.7.1, Node 26.12.0, Playwright 1.59.1. The load average was 2.42/1.76/1.73
  before the run and 1.32/1.58/1.66 after. No other Worker 3 build, test or profiler ran during timing. DVFS and
  OS caches were not controlled.
- **Candidate build:** the candidate was built from a clean detached worktree into a **fresh, empty per-SHA target
  directory** (all 131 crates compiled), with `cargo build --locked --release -p lightningpanda`, rustc 1.99.1,
  LLVM 23.1.1 and `SDKROOT=…/MacOSX26.5.sdk`.
  - Binary SHA-256: `773b253676e980bba9113d5ba8c1d49bba16ff0a0c61ff100344fdf8d0732083`.
  - Before timing it ran `cargo test --locked --release -p lp-js --test lazy_interfaces` and passed 7/7.
- **Parent binary:** the same immutable binary as the `08ad9e2` comparison, also built in its own fresh target
  directory. Binary SHA-256: `47eac6a5a20f832b8beef9cc2a60e5e846e71ff8af2b2e840f4cff8b364a86ef`.
- **Section hashes:** `provenance.json` records the per-section hashes of both binaries. The candidate's
  `__TEXT,__text` (`960f5dea…`) differs from the parent's (`6dfc4c45…`) and from `08ad9e2`'s (`55d922e8…`),
  as expected.
- **Harness:** runner `compare-resources.mjs` SHA-256 `c70d5f2d…` and sampler `7d31b30c…`, both unchanged.
  Order AB/BA/AB, 50 navigations, 20 context cycles, 10 s idle, 2 s retention.
- **Run outcome:** the runner exited 0, all 6 assertion runs passed and every browser exited on cleanup SIGTERM.
  No crash markers appeared in stderr.
- **Raw data:** `resources.json` is a byte-identical copy of the raw output. Per-run samples and stderr are at
  `/private/tmp/lp-w3-cpu-lazy-0e52072-3/` on the measurement host. The immutable binaries and their logs are at
  `/private/tmp/lp-w3-lazy-{baseline-4168ac9,candidate-0e52072}-bin/`.
