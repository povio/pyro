# Optimization inventory (automation / TOS E2E)

This file separates what has been **measured** from what is only a **hypothesis**. The measurements are local and
small-sample. They are not TOS results. Do not turn them into speedup claims: the rewrite does not load documents
yet, and it has no DOM or WebAPIs, so it does far less work than upstream.

Environment for every measurement below:
- Host: Apple M1, 8 GB, macOS 26.7.1.
- Toolchain: rustc 1.99.1, `SDKROOT=MacOSX26.5.sdk`.
- Builds: release builds of `lightningpanda` at 3d9d8d3 (plus the measurement scripts) and upstream lightpanda
  2ad38773f (`-Doptimize=fast`).
- Raw outputs: `bench/results/rewrite-3d9d8d3/` and `bench/results/upstream-2ad38773f/`.

## Measured

| What | Value | Source / method |
|---|---|---|
| Upstream spawn → `/json/version` 200 | median 108 ms idle CPU, 60 ms with one core busy (n=10 each); 57 ms when alternated with another binary (n=10) | `run-baseline.mjs`, `first-page.mjs`. CPU frequency scaling is uncontrolled. |
| Upstream micro-smoke peak RSS | ~37 MiB (20 ms sampling), 38.5 MB `time -l` max | `run-baseline.mjs`, local static site |
| Rewrite spawn → `/json/version` 200 | first launch of a fresh binary copy 212.5 ms; then median ~7 ms (n=10) | `startup-readiness.json`. **Readiness only: no JavaScriptCore VM exists yet at this point.** |
| Rewrite `EngineHost::new` (JSC VM + browser state) | 6.3–7.1 ms (3 runs) | `cdp_microbench.txt` (in-process) |
| Spawn → connectOverCDP → newPage → first `evaluate(1+1)` on about:blank, alternating binaries, n=10, medians | upstream 57 / 60.6 / 63 / 64.7 ms; rewrite 6.8 / 10.4 / 12.9 / 14.4 ms (ready/connected/page/evaluated) | `first-page-vs-upstream.json`. **Not comparable as browser performance:** the rewrite realm has no DOM or WebAPIs. Upstream builds its full V8 snapshot-backed environment. |
| In-process CDP costs (release) | context+target+enables median 0.42–0.51 ms; `Runtime.evaluate` 6–7 µs; `callFunctionOn` small fn 7–8 µs vs ~38 KB fn 172 µs; handle create+release 7 µs; about:blank navigation (realm rebuild) 138–140 µs | `cdp_microbench.txt`, 3 runs |

## Per-connection memory bounds (measured)

Every per-connection queue or table has a limit; exceeding it fails explicitly or closes the connection (1008). It is
never dropped silently. Figures are macOS `phys_footprint` deltas of one process per scenario. They were measured
with the real JavaScriptCore engine, a release build, and the state still retained. Source: `crates/lp-cdp/examples/memory_caps.rs`;
raw output (3 rounds): `bench/results/memory-caps-cdf2d33/memory_caps.txt`. Same host as above, `main` cdf2d33 plus
the handle-budget change.

| Limit | Default | Measured at the limit | On overflow |
|---|---|---|---|
| Outbound encoded frames (`DEFAULT_OUTBOUND_BUDGET`) | 64 MiB | bounded by bytes | close 1008 |
| Engine events queued within one task (`MAX_QUEUED_EVENTS`) | 100,000 | +37.0 MiB primitive args; +56.2 MiB `{i}` object args; +44.3 MiB 1 KiB strings. These include the encoded frames, measured with the outbound budget disabled; normally that budget closes the connection first. | close 1008 |
| Remote-object handles, shared by all sessions of a connection (`HandleBudget::DEFAULT_LIMIT`) | 100,000 | +28.3–28.8 MiB (small objects) | `Runtime.*` error "Too many remote objects" |
| Pending `awaitPromise` replies (`MAX_PENDING_REPLIES`) | 10,000 | +1.4–1.5 MiB (9,999) | error reply |
| Retained response bodies (`BodyLimits`, process-wide) | 64 MiB total, 8 MiB per body; at most 4,096 records per page | bounded by bytes/records | body not retained; `getResponseBody` errors |
| Paused Fetch requests (`MAX_PAUSED`) | 4,096 | not measured | request failed |
| Contexts / targets / sessions | 256 / 256 / 512 | not measured | error |

Before this change, the handle limit was 1,048,576 *per session* (512 sessions), so the aggregate was unbounded in
practice. Retained objects themselves are not limited by size: a client that keeps handles to large objects keeps
their memory. That is inherent to the protocol. It is the same in Chrome, which has no handle limit.

## Hypotheses (unmeasured unless stated)

1. **Process reuse / several contexts per process.**
   - TOS starts one Lightpanda process per live context because upstream allows a single browser context per
     process (`target.zig`: "Cannot have more than one browser context at a time").
   - The rewrite supports several contexts per connection.
   - A TOS adapter could therefore keep one process per Playwright worker.
   - Expected saving: per-test process spawn plus engine creation (upstream ≈ 57–108 ms readiness).
   - Requires a TOS fixture change and parity validation. Not started.
2. **Compiled-function cache for `Runtime.callFunctionOn`.**
   - lp-js re-parses `functionDeclaration` on every call. Parse cost scales with source size (8 µs → 172 µs for 38 KB,
     measured).
   - Worthwhile only if Playwright's real per-call sources are large. Record their sizes from a TOS run with
     `cdp-record-proxy.mjs --params` before building a per-realm cache keyed by source hash.
3. **Pre-created engine.**
   - The JSC VM is created when a connection arrives (6–7 ms, measured).
   - The server could create it ahead of the first connection.
   - Expected effect: about 6 ms per process, which matters only in combination with (1) not being adopted.
4. **Lazy WebAPI installation.**
   - There is no startup snapshot in JSC, so each realm will build its classes at creation.
   - Today an about:blank realm rebuild costs about 140 µs. That will grow with the DOM/WebAPI surface.
   - Lazy prototype installation is a W1 item (docs/PARITY.md). Remeasure once lp-webapi lands.
5. **Protocol round trips during Playwright page setup.**
   - Playwright sends about 15 commands per new page.
   - The in-process cost is about 0.5 ms, so socket/serialization overhead is the remaining variable. Measure it
     with the recording proxy timings before optimizing.
6. **Cache/login state reuse.**
   - Out of scope until the rewrite baseline exists: the user directive puts caching and login features after
     the baseline.
   - Candidates: HTTP cache shared across contexts of one process; storage-state injection without UI login.
   - Both need isolation review.
