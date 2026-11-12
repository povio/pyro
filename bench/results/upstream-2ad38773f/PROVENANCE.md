# Pyro-AUTOMATION baseline provenance

Host: Apple M1 (8 cores), 8 GB RAM, macOS 26.7.1 (25G241), user povioserver. No sudo/brew/docker.
Started: 2026-10-17T07:51Z (UTC). All PATH changes are per-command only:
`export PATH=$HOME/toolchains/zig-0.17.0:$HOME/.cargo/bin:$PATH`

## 1. Zig 0.17.0 (user-local)

```
mkdir ~/toolchains/zig-dl && cd ~/toolchains/zig-dl
curl -fsSL -o zig-aarch64-macos-0.17.0.tar.xz https://ziglang.org/download/0.17.0/zig-aarch64-macos-0.17.0.tar.xz   # 5 s, 53,985,220 bytes
shasum -a 256 zig-aarch64-macos-0.17.0.tar.xz
# b607e9b9234790a008116ae5bdb71c6243b84b9fb42a53a9e70fde41c06c536a  == expected -> OK
mkdir ~/toolchains/zig-x && tar -xJf ~/toolchains/zig-dl/zig-aarch64-macos-0.17.0.tar.xz -C ~/toolchains/zig-x
mv ~/toolchains/zig-x/zig-aarch64-macos-0.17.0 ~/toolchains/zig-0.17.0 && rmdir ~/toolchains/zig-x
~/toolchains/zig-0.17.0/zig version   # 0.17.0
```
Note: a pre-existing ~/toolchains/dl (rustup-init.sh, zig-index.json) was left untouched.

## Rust (pre-installed)
- rustc 1.99.1 (58cf8f9c3 2026-10-13), cargo 1.99.1 (5f94df478 2026-08-27), at ~/.cargo/bin
- node v26.12.0 at /usr/local/bin/node

## 2. Upstream reference clone
```
git clone --no-hardlinks ~/Documents/GitHub/browser ~/pyro-baseline/browser
git -C ~/pyro-baseline/browser checkout --detach 2ad38773fd674d3c9cea26a263b814ee46f6ba84
```
- Commit: 2ad38773fd674d3c9cea26a263b814ee46f6ba84 (2026-10-17 14:36:55 +0800, "Merge pull request #4012 from orezraey/rsa-pkcs1")
- Source checkout ~/Documents/GitHub/browser was clean (git status empty) and at the same HEAD; not modified.
- build.zig.zon version: 1.1.0-dev, minimum_zig_version 0.17.0

### Upstream release recipe (.github/workflows/release.yml, job build-macos, macos-14 aarch64)
```
zig build -Dprebuilt_v8_path=v8/libc_v8.a -Doptimize=fast snapshot_creator -- src/snapshot.bin
zig build -Dsnapshot_path=../../snapshot.bin -Dprebuilt_v8_path=v8/libc_v8.a -Doptimize=fast ${VERSION_FLAG}   # -Dversion=nightly or -Dversion=<tag>
```
- Zig 0.17 names the optimize mode `fast` (std.lang.Optimize .fast == former ReleaseFast).
- `-Dorderfile=orderfile/lightpanda.ld` is passed ONLY in build-linux (ELF/LLD linker script; orderfile/README.md: "Linux/LLD only"). Not applicable on macOS -> not used.
- `-Dcpu=` is only passed for Linux; macOS build uses native default target.
- Locally, build.zig auto-discovers the `make download-v8` archive in .pyro-cache (findPrebuiltV8), so -Dprebuilt_v8_path is equivalent to the auto path; we pass it explicitly to mirror CI.

### Prebuilt V8
```
cd ~/pyro-baseline/browser && make download-v8     # 6 s
```
- zig-v8 tag v0.5.9, V8 15.5.35.13 (from .github/actions/install/action.yml defaults)
- URL: https://github.com/lightpanda-io/zig-v8-fork/releases/download/v0.5.9/libc_v8_15.5.35.13_macos_aarch64.a
- Path: ~/pyro-baseline/browser/.pyro-cache/prebuilt-v8/v0.5.9/libc_v8_15.5.35.13_macos_aarch64.a
- Size 116,436,968 bytes; sha256 dc14027e53df891399522f6089296acfdf37207afaca1e49dd05c446d3fc475f
- Cross-checked against GitHub release API asset digest (sha256:dc14027e...c475f, asset updated 2026-10-04T23:37:21Z): MATCH

### Host toolchain quirk (SDKROOT)
Default `xcrun --show-sdk-path` = /Library/Developer/CommandLineTools/SDKs/MacOSX27.0.sdk; its .tbd stubs contain arch
`arm64e.x1` which the installed CLT linker (ld-1267, built Jun 8 2026) rejects ("tapi error: malformed file ... unknown
architecture") — reported by coordinator, verified for cargo linking. All cargo/rustc/zig builds below therefore run with
`SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk` (per-command env, nothing persisted).

### Build environment (per command)
```
export PATH=$HOME/toolchains/zig-0.17.0:$HOME/.cargo/bin:$PATH
export SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk
export CARGO_BUILD_JOBS=4      # modest parallelism, 8 GB host
```
Zig build parallelism limited with `-j4`. Zig package deps (build.zig.zon) were fetched by zig into ~/pyro-baseline/browser/zig-pkg
and ~/.cache/zig (global cache); cargo crates into ~/.cargo/registry.

### Step A: V8 snapshot (mirrors release.yml "v8 snapshot")
```
cd ~/pyro-baseline/browser
/usr/bin/time -l zig build -j4 -Dprebuilt_v8_path=.pyro-cache/prebuilt-v8/v0.5.9/libc_v8_15.5.35.13_macos_aarch64.a \
    -Doptimize=fast snapshot_creator -- src/snapshot.bin
```
- 07:54:33Z -> 07:59:14Z UTC: 280.9 s real, 589.3 s user, 58.0 s sys; max RSS (largest child) 2,830,123,008 bytes; rc=0
- Version string printed by configure: `Lightpanda 1.1.0-dev.10716+2ad38773f`
- Output src/snapshot.bin: 1,586,800 bytes, sha256 dc796fe8da0a2dbcf53dd0ba92150b4d56946fd347935846569b130ef192cb2d
- Log: ~/pyro-baseline/results/build-snapshot.log
- `-Dsnapshot_path=../../snapshot.bin` is consumed by `@embedFile` in src/browser/js/Snapshot.zig, i.e. resolves to src/snapshot.bin.

### Step B: main build (mirrors release.yml build-macos "zig build"; nightly version flag)
```
cd ~/pyro-baseline/browser
/usr/bin/time -l zig build -j4 -Dsnapshot_path=../../snapshot.bin \
    -Dprebuilt_v8_path=.pyro-cache/prebuilt-v8/v0.5.9/libc_v8_15.5.35.13_macos_aarch64.a \
    -Doptimize=fast -Dversion=nightly
```
- No `-Dorderfile` (Linux/LLD-only per orderfile/README.md and release.yml; macOS job does not pass it). No `-Dcpu` (macOS job does not pass it; native aarch64 target).
- 08:02:12Z -> 08:06:28Z UTC: 256.8 s real, 246.2 s user, 9.3 s sys; max RSS (largest child) 3,450,404,864 bytes; rc=0
  (C/Rust deps were already cached from step A, so this is mostly the Zig/LLVM compile+link of the exe.)
- Log: ~/pyro-baseline/results/build-main.log
- `git -C ~/pyro-baseline/browser status --short` after build: clean (zig-out/, zig-pkg/, .pyro-cache/, src/snapshot.bin are ignored).

### Binary
- Path: ~/pyro-baseline/browser/zig-out/bin/lightpanda (Mach-O 64-bit arm64, ad-hoc linker signature, not stripped)
- `lightpanda version` -> `1.1.0-nightly.10716+2ad38773f`
- Size: 92,895,000 bytes
- sha256: cf1545dac0239e2ff18165583bd8c6983de030a9adfb2dfabb688929b519590b
- Dynamic deps: CoreFoundation, Security, SystemConfiguration, libSystem, libobjc (system only)
- Not reproducibility-checked (single build). Note upstream ships the macOS artifact unstripped too (no split-debug step for macOS).

## 3. Smoke test and measurements

### Scripts (~/pyro-baseline/scripts)
- testsite.mjs — throwaway node http server: `/` (form#f, input#name, button#submit, button#routed, fetch() to JSON), `/api/data.json`, `/api/routed.json`.
- smoke.mjs — Playwright 1.59.1 (`require` by absolute path of tos-monorepo/node_modules/.bun/playwright-core@1.59.1/node_modules/playwright-core; nothing installed there).
  Steps: connectOverCDP(ws://127.0.0.1:PORT) -> newContext -> newPage -> goto -> locator.fill -> click+waitForResponse -> evaluate -> page.route fulfill (+click+waitForResponse+DOM check) -> close (page, context, browser).
- run-baseline.mjs — orchestrator: 10 startup runs, 5 direct smoke runs, 1 smoke via the CDP recording proxy. Spawns lightpanda exactly like
  TOS apps/fe/e2e/fixtures/lightpanda.ts: `serve --host 127.0.0.1 --port <free> --log-level error --experimental-features cors --load-resources stylesheet --load-resources iframe`, env LIGHTPANDA_DISABLE_TELEMETRY=true.
- rss-sampler-rs/main.rs (+ built `rss-sampler`, `rustc -O`, SDKROOT=MacOSX26.5.sdk) — proc_pid_rusage(V2) resident + phys_footprint every 20 ms, timed by kqueue EVFILT_TIMER NOTE_CRITICAL.
- startup-diag.mjs — spawn -> first TCP connect -> first HTTP 200 breakdown.
- cdp-record-proxy.mjs, cdp-summarize.mjs — see section 4.

### Commands
```
cd ~/pyro-baseline/scripts
B=~/pyro-baseline/browser/zig-out/bin/lightpanda
node run-baseline.mjs --bin $B --results ~/pyro-baseline/results/idle --startup-runs 10 --smoke-runs 5
node run-baseline.mjs --bin $B --results ~/pyro-baseline/results/warm --startup-runs 10 --smoke-runs 5 --warm-cpu
node cdp-summarize.mjs ../results/idle/smoke-proxy-1.cdp.jsonl ../results/cdp-inventory.json ../results/cdp-inventory.md
```
(results/superseded-run0 = first orchestrator run; superseded because its RSS samplers used blocking sleep(20ms) which this host
coalesces to 40-170 ms, giving only 3-6 samples per smoke, and the native sampler could write a partial line on SIGTERM. Fixed before idle/ and warm/.)

### Measurement-environment findings (measured)
- Timer coalescing: blocking sleeps (nanosleep/select/Atomics.wait) of 20 ms wake after 40-170 ms on this host; node event-loop
  timers are accurate (setTimeout 20 -> 22.1 ms); kqueue NOTE_CRITICAL timers are exact (20.0 ms). Samplers were designed around this.
- Startup is bimodal by CPU idle state (Apple Silicon DVFS, hypothesis consistent with experiments; no sudo for powermetrics):
  idle machine ~107 ms, with one core kept busy ~58 ms, under `taskpolicy -b` ~280 ms. All of it is before lightpanda's first
  log line. Details: results/startup-dvfs-notes.json. Hence two startup series: idle/ and warm/ (`--warm-cpu` = one node busy-spin child).

### Results (measured; runs started 2026-10-17T08:17:12Z idle, 08:17:23Z warm). n=10 startup, n=5 smoke per mode.
p95 is nearest-rank; with n=10 it equals the max, with n=5 it equals the max.

Smoke pass/fail: 9/9 steps pass in all 12 runs (5 idle direct, 5 warm direct, 1 idle via proxy, 1 warm via proxy), plus a manual
trial and an http:// discovery trial through the proxy (connectOverCDP('http://127.0.0.1:<proxy>') -> GET /json/version/ -> rewritten ws URL): 9/9.
connectOverCDP reports browser.version() = 124.0.6367.29; browser.contexts() = 1 default context at connect.
Per-step median ms (idle, 5 runs): connectOverCDP 25.9, newContext 1.2, newPage 2.6, goto 4.8, fill 14.7, click+waitForResponse 18.5,
evaluate 1.5, route.fulfill(+click+wait) 17.8, close 3.9.

| metric | idle | warm-cpu |
|---|---|---|
| startup spawn -> /json/version 200, median / p95(=max) / min (ms) | 108.04 / 109.56 / 94.30 | 59.86 / 62.32 / 58.21 |
| smoke script process wall (node start + require + all steps), median / max (ms) | 225.5 / 226.1 | 229.7 / 233.2 |
| smoke in-script wall (connect -> close), median / max (ms) | 91.1 / 92.9 | 92.3 / 94.5 |
| peak RSS, native proc_pid_rusage @20.0 ms (17 samples/run), median / max (KiB) | 37,024 / 37,088 | 37,152 / 37,504 |
| peak RSS, ps -o rss= @~21.7 ms effective (14-15 samples/run), median / max (KiB) | 37,312 / 37,504 | 37,088 / 37,504 |
| /usr/bin/time -l max RSS (whole process life), median / max (bytes) | 38,518,784 / 38,535,168 | 38,486,016 / 38,551,552 |
| peak phys_footprint native, median (KiB) / time -l peak footprint (bytes) | 14,193 / ~14.7e6 | 14,273 / ~14.7e6 |
| idle RSS right after /json/version ready (KiB) | ~18,150 | ~18,150 |
| via proxy: smoke process wall / in-script wall (ms) | 237.2 / 102.6 | 241.5 / 102.1 |

Raw: results/{idle,warm}/baseline.json and per-run *.smoke.json, *.rss-native.txt, *.rss-samples.txt, *.time-l.txt, *.lightpanda.stderr.log.

## 4. CDP recording proxy
- ~/pyro-baseline/scripts/cdp-record-proxy.mjs — uses `ws` 8.21.0 loaded read-only by absolute path
  ~/Documents/GitHub/tos-monorepo/node_modules/.bun/ws@8.21.0/node_modules/ws (overridable with CDP_PROXY_WS_PATH).
  CLI: `node cdp-record-proxy.mjs --upstream http://127.0.0.1:<pyro port> --out <file.jsonl> [--listen <port>] [--host 127.0.0.1] [--params]`
  Forwards all HTTP (rewrites upstream host:port -> proxy host:port in /json* bodies) and WebSocket upgrades on any path.
  JSONL records: c2s command (id, method, session bool), s2c event (method, session bool), s2c response/error (id, matched method,
  latency ms, error code+message), ws-open / ws-close (with unanswered command methods) / http.
- ~/pyro-baseline/scripts/cdp-summarize.mjs `<in.jsonl> <out.json> [out.md]` -> sorted count tables.
- Verified: error path logged correctly with a raw ws client (`Bogus.method` -> -32601 "'Bogus.method' wasn't found").
- Inventory of the smoke: results/cdp-inventory.md / .json (from results/idle/smoke-proxy-1.cdp.jsonl; warm run table identical).
  91 commands (34 distinct), 0 error responses, 33 events (19 distinct), 1 WS connection, 1 HTTP GET /json/version.
