# Matched local HTTP CPU and memory, 2026-10-17

All six processes passed the same assertions: exact HTTP body/status/URL and inline-script checksum,
50 repeated navigations and 20 create/navigate/dispose context cycles. Three fresh-process samples per binary,
alternating upstream/rewrite then rewrite/upstream then upstream/rewrite. One warmup navigation, 10-second
idle phase before work, 2-second quiet retention phase after disposal. No CPU spinner or forced GC.

This is a small HTTP/JavaScript workload, **not full browser parity or TOS compatibility/performance**.
DOM interaction, subresources, TLS, route interception, authentication, and actual TOS flows are absent.
The TOS reference remains unmodified f9fde4aa0d with its documented direct-lane version bypass and
backend-token localStorage authentication; this run neither replaces nor executes that baseline.

| Median of 3 samples | Upstream | Rewrite |
| --- | ---: | ---: |
| 50 navigations wall ms | 219.24 | 123.24 |
| 50 navigations browser user ms | 259.07 | 60.00 |
| 50 navigations browser system ms | 24.30 | 17.88 |
| 50 navigations browser total CPU ms | 283.53 | 77.88 |
| 20 context cycles wall ms | 97.44 | 88.21 |
| 20 context cycles browser user ms | 61.73 | 35.17 |
| 20 context cycles browser system ms | 11.85 | 14.36 |
| 20 context cycles browser total CPU ms | 73.20 | 49.54 |
| 10-second idle browser CPU ms | 3.58 | 1.21 |
| Idle CPU, percent of one core | 0.0357% | 0.0121% |
| Sampled peak resident KiB | 32,032 | 15,904 |
| Sampled peak physical footprint KiB | 10,641 | 7,297 |
| Retained resident KiB after disposal + 2s | 31,184 | 15,856 |
| Retained physical footprint KiB | 9,905 | 6,545 |

Medians are calculated independently for each column, so median user + median system need not equal median
total. CPU may exceed wall time because it includes all browser threads. Very small idle CPU differences
should not be interpreted as a durable advantage. Rewrite post-disposal CPU was slightly higher in the
2-second retention window (median 0.562 ms versus 0.208 ms); neither showed material idle CPU in this workload.

## Exact provenance

- Host: Apple M1, 8 cores, 8 GiB, macOS 26.7.1 (Darwin 25.6.0), aarch64; Node 26.12.0,
  Playwright 1.59.1. Same host for both binaries; background activity and DVFS uncontrolled.
- Upstream source `2ad38773fd674d3c9cea26a263b814ee46f6ba84`, binary SHA-256
  `cf1545dac0239e2ff18165583bd8c6983de030a9adfb2dfabb688929b519590b`.
  Zig 0.17.0 `-Doptimize=fast`, snapshot, prebuilt V8; full recipe in
  [upstream provenance](../upstream-2ad38773f/PROVENANCE.md). Existing binary was reused unmodified.
- Rewrite source `d673567624446930b68e800bbf4c0b8fa6659ccd`, binary SHA-256
  `f73a686f451545b786b43054e5fe9eb75b2bada0e1e2865f9ae8e0f02623516c`.
  `cargo build --locked --release -p pyro`, rustc 1.99.1, thin LTO/one codegen unit,
  SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk.
  Dependencies include runtime cba2671/f90494e, Network 9b8476c, pyro-net 5cac939 (observer 621711c,
  request-start 25d8567), and Fetch integration d673567; exact Cargo.lock in that commit.
- Harness/sampler source `e125473b43f3df26006ca1bbd33e1fdeedf2c1ba`. Sampler built with
  `cargo build --locked --release --manifest-path bench/rss-sampler/Cargo.toml` and the same SDKROOT.
  Binary/harness/fixture hashes are in [resources.json](resources.json).
- Cleanup sent SIGTERM after assertions: upstream handled it and exited 0 in all 3 processes; rewrite
  exited with SIGTERM in all 3 processes. No SIGSEGV or SIGTRAP. Release Playwright integration smoke also passed 15/15 before these measurements.

Raw phase snapshots and all individual samples are retained in resources.json, including browser versus
Node harness CPU. The local server runs in that Node process; no TOS harness costs are included. Sampler
CPU is separate. Native 20ms traces and stderr are retained locally at
`/private/tmp/pyro-w3-cpu-matched-3/`. A short pilot was used only to validate the harness, not in this table.
OS/file caches were not flushed and were likely warm. The 20ms peak samples may miss short spikes; retained
RSS is not a live-heap measurement. Snapshot helper overhead slightly extends CPU observation windows.

Sampler CPU conversion was checked against `getrusage(RUSAGE_SELF)` over a 250 ms workload:
250.7795 ms sampled versus 250.004 ms reference (0.7755 ms difference including helper-launch accounting).
Host Mach timebase 125/3 proves raw ticks cannot be treated directly as nanoseconds.

The owned protocol loop waits on notifications and coalesces network wakeups; no periodic browser polling
was introduced. CPU and allocation work on broader workloads remains unmeasured.
