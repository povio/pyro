# Integrated browser CPU and memory, 2026-10-17

The rewrite used **more active CPU and wall time** than upstream on this matched local HTTP workload,
while retaining less resident memory. All six processes passed the unchanged assertions. These measurements
replace no earlier artifacts and establish no full-browser or TOS performance claim.

Three fresh-process samples per binary ran in AB, BA, AB order. Each performed one warmup navigation,
10 seconds idle, 50 repeated navigations, 20 create/navigate/dispose context cycles, then 2 seconds quiet
retention after disposal. Every navigation checked HTTP status, URL, exact body bytes and inline-script
checksum. No CPU spinner or forced GC was used. The fixture contains no DOM interaction, subresources,
TLS, authentication, route interception or TOS application code.

| Median of 3 samples | Upstream | Rewrite |
| --- | ---: | ---: |
| 50 navigations wall ms | 213.14 | 377.73 |
| 50 navigations browser user ms | 255.99 | 408.18 |
| 50 navigations browser system ms | 23.40 | 54.78 |
| 50 navigations browser total CPU ms | 279.71 | 462.95 |
| 20 context cycles wall ms | 99.05 | 256.53 |
| 20 context cycles browser user ms | 64.44 | 225.89 |
| 20 context cycles browser system ms | 11.92 | 38.04 |
| 20 context cycles browser total CPU ms | 76.48 | 264.01 |
| 10-second idle browser CPU ms | 3.378 | 1.411 |
| Idle CPU, percent of one core | 0.0337 | 0.0141 |
| 2-second retention browser CPU ms | 0.201 | 0.786 |
| Sampled peak resident KiB | 32,032 | 19,712 |
| Sampled peak physical footprint KiB | 10,961 | 9,089 |
| Retained resident KiB after disposal + 2s | 31,328 | 19,616 |
| Retained physical footprint KiB | 9,985 | 6,977 |
| 50 navigations Node harness user ms | 65.05 | 70.11 |
| 50 navigations Node harness system ms | 9.97 | 8.22 |
| 20 context cycles Node harness user ms | 57.84 | 53.70 |
| 20 context cycles Node harness system ms | 7.09 | 7.41 |

Column medians are independent, so median user + median system need not equal median total. Browser CPU
includes all threads of the browser PID, excluding children. It can exceed wall time. Node measurements
include Playwright and the local HTTP server, separately from browser CPU; no TOS harness was run.
The native sampler also has separate costs: total CPU was 36.20/34.33/39.36 ms for upstream samples and
41.75/37.75/38.91 ms for rewrite samples. Neither browser showed material idle CPU here; the tiny difference
is not evidence of a durable idle advantage. Sampled peaks can miss short spikes, and retained RSS is not
live heap size. CPU windows include snapshot-helper launch overhead.

## Provenance and conditions

- Same Apple M1 host, 8 cores / 8 GiB, macOS 26.7.1 (Darwin 25.6.0), arm64;
  Node 26.12.0 and Playwright 1.59.1. Host load averages before/after were
  `[1.589, 1.875, 1.964]` / `[1.115, 1.624, 1.854]`. Background activity and DVFS were uncontrolled.
  OS/file caches were not flushed and were likely warm. No simultaneous Worker3 build or test ran.
- Upstream source `2ad38773fd674d3c9cea26a263b814ee46f6ba84`; reused unmodified binary SHA-256
  `cf1545dac0239e2ff18165583bd8c6983de030a9adfb2dfabb688929b519590b`.
  Zig 0.17.0 `-Doptimize=fast`, snapshot/prebuilt V8;
  [build provenance](../upstream-2ad38773f/PROVENANCE.md).
- Rewrite source **`41356ebf27909caf3039b8d6d2b2702122d629cd`**, clean checkout;
  binary SHA-256 **`2bbe638d5a31b40008b7458836d5aedc6347fd7789e386cca1868c97aff4f7bf`**.
  Fresh `cargo build --locked --release -p pyro`, rustc 1.99.1
  (`58cf8f9c35b355199727627d07ae34a4e2846d67`), LLVM 23.1.1, thin LTO / one codegen unit;
  `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk`.
  Exact dependencies are in that commit's Cargo.lock. Integrated heads include browser bindings `7551f073`,
  stylesheet/checkpoint `339b714`, native UA `2c79ea0`, browser UA `0f456ec`, callback hook `66a5026`,
  protocol DOM/UA `166277a`, MutationObserver `f800bc1`, browser viewport `39373a6`, and CDP viewport `df51723`.
  Later navigation repair `bcb20691` and URLSearchParams `26c496c` are **not included**.
- Runner `bench/scripts/compare-resources.mjs` SHA-256
  `c70d5f2de867c5f39881a8606996a01c975eeaf377f9ee96391d686a8b550fc7`;
  native sampler SHA-256 `7d31b30cab16622eda1333be012d1aa8fba2bbed6669b2e269c681725d198569`.
  Both are unchanged from the earlier run. The release sampler uses Mach timebase conversion;
  its existing getrusage cross-check is recorded in the [earlier methodology](../http-cpu-d673567/README.md).
- Fixture SHA-256 `58c40aa1d87e1769634f6826650ab772d75ddcc008d213514341a43a2fc61697`.
  Complete binary sizes, launch flags, raw phase snapshots and individual samples are in
  [resources.json](resources.json), copied byte-for-byte from the completed run.
- Runner exited 0. All three upstream processes handled cleanup SIGTERM and exited 0;
  all three rewrite processes exited on cleanup SIGTERM. No SIGSEGV or SIGTRAP occurred in this run.
  This does not resolve the separately preserved raw-JSC test SIGTRAP.

The measured source tree equals the PR20 tree that passed 373 workspace tests, 115 protocol tests in each
profile, strict workspace clippy/fmt, and 27/27 real Playwright checks in each profile. This benchmark used
a fresh release binary built at the exact merged source SHA, not either older validation executable.

Reproduction (substitute local executable/module paths; preserve the source and binary hashes):

```sh
NODE_PATH=/path/to/bench/node_modules node bench/scripts/compare-resources.mjs \
  --upstream /path/to/upstream/lightpanda \
  --upstream-sha 2ad38773fd674d3c9cea26a263b814ee46f6ba84 \
  --rewrite /path/to/pinned/pyro \
  --rewrite-sha 41356ebf27909caf3039b8d6d2b2702122d629cd \
  --sampler /path/to/rss-sampler --out /path/to/new-run \
  --samples 3 --navigations 50 --contexts 20 --idle-ms 10000 --retained-ms 2000
```

Raw native 20 ms traces and stderr remain at `/private/tmp/pyro-w3-cpu-41356eb-3/` on the measurement host;
the immutable rewrite binary and build provenance remain at `/private/tmp/pyro-w3-bench-41356eb-bin/`.
The earlier `d673567` result measured an earlier partial implementation. Its lower active CPU cannot be
presented as the current integrated browser's performance or as equivalent full-browser behavior.

## Separate diagnostic profiles

After all benchmark samples, the same pinned rewrite binary completed 2,000 navigation assertions and
1,000 context-cycle assertions. Each phase ran `/usr/bin/sample <browser-pid> 5 1 -file <phase>.sample.txt`.
Both sampler exits were 0; browser cleanup was SIGTERM, with empty stderr. These diagnostic processes are
**excluded from all benchmark medians**. The profiler observes stacks, including waits and GC; its inclusive
counts are not CPU percentages and do not establish the cause of the upstream/rewrite difference.

| Inclusive browser-thread stack snapshots | Navigations | Context cycles |
| --- | ---: | ---: |
| Total browser-thread snapshots | 1,986 | 3,659 |
| Engine::create_realm | 1,313 | 2,511 |
| realm::install_classes (within realm creation) | 1,188 | 2,201 |

Repeated native function creation and calls to setLength/accessor/makeInterface are visible within
`install_classes` (`crates/pyro-js/src/realm.rs:965-983`). During the navigation sample, the main transport
thread was in kevent in 1,959 of 1,986 snapshots. This points to realm/class setup as a useful runtime-owner
investigation, with no evidence here of transport spinning. It is not a demonstrated optimization or a
reason to remove APIs/isolation. Runtime ownership received the findings; no runtime source was changed.
Raw profiles and result are preserved at `/private/tmp/pyro-w3-profile-41356eb/`; diagnostic driver is
`/private/tmp/pyro-w3-profile-41356eb.cjs`. Artifact hashes are in [profile.json](profile.json).

The actual TOS baseline remains unmodified `f9fde4aa0d`, documented direct-lane version-gate bypass,
backend-token localStorage authentication, and upstream profile-settings 2/2 on its own baseline host.
The latest separate rewrite TOS attempt at `26c496c` still passed 0/2 tests (missing URL constructor),
although both URL assertions progressed. Failed TOS runs are not comparable performance evidence.
