# Benchmarks and CDP recording

Reproducible harness for measuring any CDP browser binary (upstream Lightpanda or `lightningpanda`) with the
same Playwright version TOS pins (`playwright-core@1.59.1`). Three result classes are kept strictly apart:

1. **Upstream micro-smoke** (`results/upstream-2ad38773f/`): original Lightpanda on a local static site. It
   characterizes the reference binary only. It is **not** a TOS E2E baseline, and it is **not** a TOS parity result.
2. **TOS E2E parity/baseline**: real `tos-monorepo` FE E2E lanes. Pending; they run on a host with native
   PostgreSQL (see `docs/automation/TOS_BASELINE.md` once recorded).
3. **Rewrite performance**: matched, explicitly scoped workloads against release binaries. The HTTP CPU/memory
   runner below covers the integrated navigation path; full TOS comparisons remain dependent on compatibility.

## Setup

```sh
cd bench && bun install                                           # playwright-core 1.59.1, ws 8.21.0
(cd rss-sampler && cargo build --release)                         # macOS proc_pid_rusage sampler
```

The upstream binary is built from source; its provenance and exact commands are in
`results/upstream-2ad38773f/PROVENANCE.md`. Binaries are not committed.

## Run

```sh
node scripts/run-baseline.mjs --bin /abs/lightpanda --results /abs/out [--startup-runs 10] [--smoke-runs 5] [--warm-cpu] [--no-proxy]
node scripts/cdp-record-proxy.mjs --upstream ws://127.0.0.1:<port> --out /abs/cdp.jsonl [--listen <port>]
node scripts/cdp-summarize.mjs /abs/cdp.jsonl /abs/inventory.json [/abs/inventory.md]
```

The browser is started with the exact flags and environment TOS uses (`apps/fe/e2e/fixtures/lightpanda.ts`).
`serve --host 127.0.0.1 --port N --log-level error --experimental-features cors --load-resources stylesheet
--load-resources iframe`, with `LIGHTPANDA_DISABLE_TELEMETRY=true`.

## Exit status

`smoke.mjs` exits 1 unless every step passed (steps blocked by an earlier failure are reported as skipped);
`run-baseline.mjs` exits 1 if any smoke run or startup run failed. Results are written either way.

## Methodology and limitations

- Startup = spawn to the first `200` from `GET /json/version`, polled every ~2 ms with fresh connections
  (the TOS fixture polls every 10 ms, so its observed readiness can be up to ~10 ms later).
- RSS is sampled at 20 ms by `rss-sampler` (kqueue `NOTE_CRITICAL` timer; on the reference host plain sleeps woke
  after 40–170 ms), cross-checked against `ps` sampling and `/usr/bin/time -l` max RSS.
- Small samples (n=10 startup, n=5 smoke): p95 equals the max. Single host, single build.
- **CPU frequency scaling is not controlled.** On the reference M1, startup median was 108 ms with an idle machine
  and 60 ms with one core kept busy (`--warm-cpu`). Treat this as a limitation of the measurement. Report both
  series and compare engines only under the same mode.

## Matched CPU and retained-memory workload (macOS)

`compare-resources.mjs` alternates upstream/rewrite release binaries on one host. Each fresh process warms
one HTTP page, idles for 10 seconds, navigates 50 times, creates/navigates/disposes 20 contexts, then waits
2 seconds after disposal. Every navigation asserts status, URL, exact response bytes and an inline-script
checksum. This deliberately limited workload excludes DOM interaction, subresources, TLS and TOS flows.
It supports measuring the integrated HTTP path; it cannot establish whole-browser performance parity.

```sh
node scripts/compare-resources.mjs \
  --upstream /abs/upstream-release --upstream-sha FULL_40_CHAR_SHA \
  --rewrite /abs/rewrite-release --rewrite-sha FULL_40_CHAR_SHA \
  --sampler /abs/rss-sampler --out /abs/new-results-directory \
  --samples 3 --navigations 50 --contexts 20 --idle-ms 10000 --retained-ms 2000
```

Use source SHAs corresponding to the actual builds and retain their build recipes. Results include executable
SHA-256, source SHA declarations, sampler/harness/fixture hashes, host, flags, version response, sample order,
raw phase snapshots, native traces, errors and direct exit/signal status. Output directories must be new.
Any failed behavioral assertion makes the run fail and keeps its evidence; do not aggregate failures as successes.

- CPU is measured for the browser PID, including all its threads, excluding harness and helper processes.
  Node CPU (Playwright plus the local HTTP fixture) is reported separately for each phase; sampler cumulative
  CPU is also retained. There is no TOS harness in this workload.
- `proc_pid_rusage` supplies Mach-time CPU counters. The sampler converts with `mach_timebase_info`, using
  wide intermediate arithmetic. Apple's [task accounting](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/kern/task.c)
  and [rusage export](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/kern/bsd_kern.c) define this path.
  Do not assume a 1:1 nanosecond timebase on Apple Silicon. A local cross-check against `getrusage(RUSAGE_SELF)`
  during a 250 ms CPU workload is part of sampler validation.
- `browserOneCorePercent` = (user + system CPU time) / actual sampled wall time × 100, without dividing by
  host core count. Phase CPU windows bracket snapshot helper launches; `workWallMs` separately measures the
  actual workload. Snapshot helper overhead is outside reported Node CPU, and may slightly lengthen browser
  observation windows.
- Native memory is sampled every 20 ms; reported peaks can miss shorter spikes and are not lifetime maxima.
  Retained resident/physical-footprint values are measured after context disposal and a fixed quiet interval,
  without forced GC. They are not live-heap measurements or proof of a leak.
- Each sample is a fresh browser process but OS/file caches are uncontrolled and likely warm. One page warms
  the engine before phases. No busy-spin CPU warmer is used. Host contention and DVFS remain uncontrolled;
  alternate binary order, retain individual samples and report sample count. Never compare debug to release.

The sampler's six columns are epoch milliseconds, resident KiB, physical footprint KiB, cumulative user
nanoseconds, cumulative system nanoseconds and monotonic nanoseconds. Existing memory readers use the first
three columns. `rss-sampler PID - --once` emits one snapshot and fails explicitly if the PID cannot be sampled.
