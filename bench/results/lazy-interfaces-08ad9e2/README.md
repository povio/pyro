# Lazy interface installation: affected-host comparison, 2026-10-17

All six matched runs passed. On this host, the candidate halved browser CPU for 50 navigations and used about 60%
less CPU for 20 context cycles. Every candidate sample used less CPU than every parent sample. This result comes
from 3 samples per build on a limited local HTTP workload. It is not a statistical confidence claim, a
full-browser result or a TOS benchmark.

This compares **two Rust rewrite builds**; neither is Zig upstream Lightpanda. The harness's historical labels
map as follows:
- `upstream` = parent `4168ac99eb68cab56fc9f733acde8cc8ecf9f218` (main before PR #35);
- `rewrite` = candidate `08ad9e28d1a3b1a6ebf29f845e18a3f566f5becf` (`perf/runtime-lazy-interfaces`, a direct
  child of the parent).

**Scope note:** PR #37's head later moved to `9c9b853f63d6eb80c8661d78f585cbab21a669b6`, which fixes
disposed-realm consistency and `preventExtensions` before first use. That head was **not** measured here.

## Results

| Median of 3 samples | Parent 4168ac9 | Candidate 08ad9e2 | Change |
| --- | ---: | ---: | ---: |
| 50 navigations, wall ms | 388.43 | 243.45 | −37.3% |
| 50 navigations, browser CPU ms | 483.03 | 249.25 | −48.4% |
| 20 context cycles, wall ms | 286.08 | 153.09 | −46.5% |
| 20 context cycles, browser CPU ms | 320.31 | 123.57 | −61.4% |
| 10-second idle, browser CPU ms | 1.47 | 1.58 | negligible for both |
| Sampled peak resident KiB | 20,272 | 18,656 | |
| Sampled peak physical footprint KiB | 9,537 | 8,225 | |
| Retained resident KiB | 20,176 | 18,560 | |
| Retained physical footprint KiB | 7,377 | 7,217 | |

| Sample | Build | Navigation CPU ms | Context CPU ms |
| --- | --- | ---: | ---: |
| 1 | Parent | 485.7 | 320.8 |
| 1 | Candidate | 249.2 | 123.6 |
| 2 | Candidate | 250.9 | 122.4 |
| 2 | Parent | 483.0 | 318.7 |
| 3 | Parent | 480.6 | 320.3 |
| 3 | Candidate | 247.1 | 126.9 |

The runtime owner measured the same change on their own host: −43.8% navigation CPU and −46.6% context CPU. The
direction agrees; the magnitudes differ between hosts.

## Setup and provenance

- **Host:** Apple M1, 8 GiB, macOS 26.7.1, Node 26.12.0, Playwright 1.59.1. The load average was 2.13/2.13/1.65
  before the run and 1.39/1.88/1.60 after. No other Worker 3 build, test or profiler ran during timing. DVFS and
  OS caches were not controlled.
- **Builds:** both SHAs were built with `cargo build --locked --release -p pyro` (rustc 1.99.1,
  LLVM 23.1.1, `SDKROOT=…/MacOSX26.5.sdk`). Each SHA was built from its own clean worktree into a **fresh, empty
  per-SHA target directory**, and each build compiled all 131 crates.
  - Binary SHA-256: parent `47eac6a5a20f832b8beef9cc2a60e5e846e71ff8af2b2e840f4cff8b364a86ef`, candidate
    `0b714171604a25183832b95175add7b951a26b2eeb1fbc405b513dee3fb96ce6`.
  - The immutable copies are on the measurement host, with their build logs.
- **Focused test:** before timing, the candidate ran `cargo test --locked --release -p pyro-js --test lazy_interfaces`
  and passed 6/6.
- **Harness:** runner `compare-resources.mjs` SHA-256 `c70d5f2d…` and sampler `7d31b30c…`, both unchanged from
  earlier measurements. Order AB/BA/AB, 50 navigations, 20 context cycles, 10 s idle, 2 s retention.
- **Run outcome:** the runner exited 0, all 6 assertion runs passed and every browser exited on cleanup SIGTERM.
  No SIGSEGV or SIGTRAP occurred.
- **Raw data:** `resources.json` here is a byte-identical copy of the raw output. Per-run samples and stderr are
  at `/private/tmp/pyro-w3-cpu-lazy-08ad9e2-3/` on the measurement host. Commands and hashes are in `provenance.json`.

## Discarded first build (never timed)

The first attempt built both worktrees into the shared target directory used by earlier comparisons. Cargo
considered the newly checked-out candidate sources older than the existing artifacts: the candidate build
finished in 0.10 s, and both binaries had SHA-256 `d0474f54…`. Their `__TEXT,__text` sections are identical,
and both equal the parent's code. This was caught before timing; both binaries are preserved and marked invalid.
Comparisons now build each SHA in an isolated fresh target directory. The earlier shared-target comparisons were
audited separately (`../build-provenance-audit/`); all of them were built from their stated sources.
