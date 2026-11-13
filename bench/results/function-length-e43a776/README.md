# Function-length installation: affected-host comparison, 2026-10-17

All six matched processes passed. The candidate reduced median browser CPU by **1.82% for 50 navigations**
and **3.81% for 20 context cycles** on this host. Each candidate sample used less active CPU than each parent
sample in this set. This is a modest result from three samples per version on a limited local HTTP workload,
not a statistical confidence claim, broad speedup, full-browser parity or TOS benchmark. Retained RSS rose
slightly while physical footprint fell; there is no general memory-win claim.

This compares **two Rust rewrite builds**. The unchanged harness's historical JSON labels map as follows:
`upstream` = parent `f537c86ee1c31b49e221cc13ee0d7a41ac96e713`;
`rewrite` = candidate `e43a7766cf81ab634bb8744bd490a2f04fd4fa02`.
**Neither binary is Zig upstream Lightpanda.** Older upstream/rewrite evidence is preserved separately.

## Results

| Median of 3 samples | Parent f537c86 | Candidate e43a776 |
| --- | ---: | ---: |
| 50 navigations wall ms | 389.22 | 386.42 |
| 50 navigations browser user ms | 424.33 | 417.35 |
| 50 navigations browser system ms | 55.67 | 53.79 |
| 50 navigations browser total CPU ms | 479.99 | 471.26 |
| 20 context cycles wall ms | 280.33 | 270.05 |
| 20 context cycles browser user ms | 258.41 | 247.74 |
| 20 context cycles browser system ms | 40.92 | 40.11 |
| 20 context cycles browser total CPU ms | 299.26 | 287.85 |
| 10-second idle browser CPU ms | 1.529 | 1.561 |
| Idle CPU, percent of one core | 0.0153 | 0.0156 |
| 2-second retention browser CPU ms | 0.766 | 0.669 |
| Sampled peak resident KiB | 20,208 | 20,256 |
| Sampled peak physical footprint KiB | 9,377 | 9,121 |
| Retained resident KiB | 20,112 | 20,160 |
| Retained physical footprint KiB | 7,409 | 7,201 |
| Navigation Node harness user ms | 62.57 | 74.06 |
| Navigation Node harness system ms | 7.42 | 8.60 |
| Context Node harness user ms | 67.43 | 54.03 |
| Context Node harness system ms | 7.94 | 7.85 |

| Sample | Build | Navigation CPU ms | Context CPU ms | Retained RSS KiB |
| --- | --- | ---: | ---: | ---: |
| 1 | Parent | 481.019 | 297.958 | 20016 |
| 1 | Candidate | 471.264 | 287.851 | 20144 |
| 2 | Candidate | 474.304 | 283.989 | 20160 |
| 2 | Parent | 479.195 | 299.256 | 20112 |
| 3 | Parent | 479.987 | 303.379 | 20112 |
| 3 | Candidate | 467.740 | 291.213 | 20224 |

Medians are independent per column; user + system medians need not equal the total median. Browser CPU
includes all threads of its PID, excluding children. Node CPU includes Playwright and the local HTTP server,
separately from browser CPU. Native sampler total CPU was 39.07/41.01/38.79 ms for parent samples and
40.65/42.58/38.34 ms for candidate samples. Neither version showed material idle CPU; the tiny idle/retention
differences do not establish an advantage. Sampled peaks may miss short spikes; retained RSS is not live heap.

## Matched setup and provenance

- Same Apple M1, 8 cores / 8 GiB, macOS 26.7.1 (Darwin 25.6.0), arm64; Node 26.12.0,
  Playwright 1.59.1. Load average before/after: `[2.224, 1.750, 1.452]` / `[1.185, 1.533, 1.398]`.
  No concurrent Worker3 builds/tests or profiler ran during timing. Background host activity, DVFS and
  OS/file caches were uncontrolled; caches were likely warm.
- Fresh browser process per sample; order parent/candidate, candidate/parent, parent/candidate.
  One warmup navigation, 10 seconds idle, 50 navigations, 20 create/navigate/dispose context cycles,
  2 seconds quiet retention after disposal. No CPU spinner, forced GC, retries or discarded samples.
- Unchanged assertions: HTTP status, exact response body, response URL and inline JavaScript checksum.
  No DOM interaction, subresources, TLS, authentication, interception or TOS application in the fixture.
  CPU windows bracket snapshot-helper launches; memory sampling interval is 20 ms.
- Parent source `f537c86ee1c31b49e221cc13ee0d7a41ac96e713`, clean checkout;
  binary SHA-256 `73323263e912e4c1060d6466705e235d96c4135b02b3fd403fb561a773569a50`.
- Candidate source `e43a7766cf81ab634bb8744bd490a2f04fd4fa02`, clean checkout, direct child of parent;
  binary SHA-256 `8b1a0c95e3dca3019be2fff9de564fad54155dd4f92eb947f9bb26c1e72c053e`.
- Both built with `cargo build --locked --release -p pyro`, rustc 1.99.1
  (`58cf8f9c35b355199727627d07ae34a4e2846d67`), LLVM 23.1.1, thin LTO/one codegen unit;
  `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk` and the same Cargo target directory.
  Exact lockfile/build settings are unchanged. Each executable was copied to a separate immutable artifact
  path before the next build. [Provenance](provenance.json) records build commands, sizes and labels.
- Runner SHA-256 `c70d5f2de867c5f39881a8606996a01c975eeaf377f9ee96391d686a8b550fc7`;
  sampler SHA-256 `7d31b30cab16622eda1333be012d1aa8fba2bbed6669b2e269c681725d198569`;
  fixture SHA-256 `58c40aa1d87e1769634f6826650ab772d75ddcc008d213514341a43a2fc61697`.
  Runner/sampler source is unchanged from the [earlier measurement](../http-cpu-41356eb/README.md).
- Six successful assertion runs, runner exit 0. Every browser exited on deliberate cleanup SIGTERM;
  no SIGSEGV or SIGTRAP occurred. This does not resolve the separately preserved raw-JSC SIGTRAP.

The only source changes are in `pyro-js` helpers/class installation plus its regression test. Zero-arity
native methods/statics and constructor wrappers keep their existing length descriptor rather than
redefining it. Definitions remain eager and the nonzero-arity path remains intact.
Before timing, this host ran `cargo test --locked --release -p pyro-js --test class_lengths`: **1/1 passed**,
checking zero/nonzero descriptors/names, eager globals, inheritance/brands, subclassing, cross-realm mutation
and GC. The runtime owner separately reported 406 workspace tests, 97 selected release tests, strict
workspace clippy/fmt and 27/27 release Playwright checks; those are owner-reported, not reruns on this host.
No independent correctness-review approval is inferred from this validation.

## Reproduction and retained artifacts

```sh
NODE_PATH=/path/to/bench/node_modules node bench/scripts/compare-resources.mjs \
  --upstream /path/to/parent/pyro \
  --upstream-sha f537c86ee1c31b49e221cc13ee0d7a41ac96e713 \
  --rewrite /path/to/candidate/pyro \
  --rewrite-sha e43a7766cf81ab634bb8744bd490a2f04fd4fa02 \
  --sampler /path/to/rss-sampler --out /path/to/new-causal-run \
  --samples 3 --navigations 50 --contexts 20 --idle-ms 10000 --retained-ms 2000
```

[resources.json](resources.json) is a byte-for-byte copy of all six raw results, including phase snapshots,
harness costs and cleanup signals. Native traces/stderr remain on the measurement host at
`/private/tmp/pyro-w3-cpu-function-length-e43a776-3/`. Immutable binaries/build provenance remain at
`/private/tmp/pyro-w3-cpu-baseline-f537c86-bin/` and `/private/tmp/pyro-w3-cpu-candidate-e43a776-bin/`;
the latter includes the focused test log. No source or assertions were changed by the measurement worker.

The earlier unpromoted batching variants were not rerun here. This result is separate from the actual TOS
profile/settings 2/2 smoke and representative batch 0/9 at the parent-equivalent source. No TOS test or
performance assertion was made for this candidate by this measurement.
