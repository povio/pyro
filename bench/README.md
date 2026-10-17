# Benchmarks and CDP recording

Reproducible harness for measuring any CDP browser binary (upstream Lightpanda or `lightningpanda`) with the
same Playwright version TOS pins (`playwright-core@1.59.1`). Three result classes are kept strictly apart:

1. **Upstream micro-smoke** (`results/upstream-2ad38773f/`): original Lightpanda on a local static site. It
   characterizes the reference binary only. It is **not** a TOS E2E baseline, and it is **not** a TOS parity result.
2. **TOS E2E parity/baseline**: real `tos-monorepo` FE E2E lanes. Pending; they run on a host with native
   PostgreSQL (see `docs/automation/TOS_BASELINE.md` once recorded).
3. **Rewrite performance**: the same harness against `lightningpanda serve`. Pending until the rewrite can serve
   pages.

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
