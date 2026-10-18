# Final bounded page-readiness comparison

Frozen before execution. Evidence only; no product changes. This concludes the performance experiment scope after reporting; no additional speculative optimization or intrusive profiling follows.

A: retained production eabee486709f79167520a46887cd6f303b26711d, tree a30b943b31b44e211e06b5184023877a6afac7f0, signed binary66e7448e07666176e9650ba714cba04ecc570c65f6e6139ad5bd759f5b49ad88. B: fresh exact production main93b1b3a7c360cd892084a7e4d538024ead798fdc, fetched2026-10-18. Current main includes bounded URL-base cache, geometry/focus changes and signing policy; this measures their combined current behavior, not isolated causal gains.

## Fixed budget and setup

W2 owns one same-host execution under its heavy lock, sequentially, no concurrent owned builds/tests. Eight fresh processes in ABBA BAAB order, three hard navigations per process:24 navigations maximum. One existing300second collector test limit, no retries, replacement samples, or extra warmup/app runs. Each launch creates a fresh facade/context/page; navigations2/3 reuse that context/page. Shared cache64MiB and private context cache64MiB throughout; private cache not disabled. Fresh browser caches are not cold OS/server/database caches. Keep per-run isolated nativePG DB/auth fixture behavior and cleanup; no production credentials/shared destructive cleanup.

Both immutable binaries: release DEBUG0, identical rustc1.99.1/LLVM23.1.1 and macOS SDK26.5 on W2, allow-jit verified, polling traps explicitlytrue, no profiling env. Baseline was manually re-signed because its historical strip policy discarded the linker entitlement; currentmain uses merged stripnone/automatic signing. Record this distinction and binary/tree/hash/LC_BUILD_VERSION/build settings. Do not relabel historical or diagnostic binaries as main.

## Collection contract

Use retained observer-free collector15df21569518e8f091c45a9f146f406b4369d4112b9aa553cc03ee6db2a8f564 and sampler e17e429b843600737c72aa07a97c41eb537a132c648ec46659380c45500e2d5b. Mechanical edits limited to output paths, frozen binary/order/launch/block labels. No added page script, observer rescans, app patches, request interception, or profiler. Keep auth/helper/manifest/static bundle and all seven assertions from unchanged f9fde4aa0d profile-settings case intact: URL, title, heading, First Name, Last Name, disabled Email, Invoice signature. Original spec is not edited; external diagnostic wrapper only. This is not an unchanged suite/fullTOS result.

Measure host navigation start to completion of those sequential assertions (usable), host commit/DCL/load, existing all-browser-thread CPU and RSS samples before goto/at usable/after observation horizon, sanitized Network counts/decoded bytes/terminal encoded-body bytes/cache flags. Keep existing+1second post-usable horizon, report actual horizon; last lazy asset is not the usable deadline. StartWorkerSession/auth/DB/build are outside navigation windows and reported separately. No raw URLs, headers, tokens, bodies, or auth state in publication.

Collector has no optional DOM readiness observer; assertions still poll and Network listeners/sample launches/save calls have overhead. CPU spans sampler windows and all threads, not exclusive JS or navigation CPU. CDP dataLength is decoded; terminal encodedDataLength is encoded response body, not total HTTP bytes. Cache flags represent in-memory cache. Count unknown/request-unassociated events and failures; no completion claim for requests beyond horizon. No forcedGC or memory-leak inference.

## Predeclared interpretation

Report every navigation and distributions by ordinal1/2/3. Primary independent units are processes: fresh=nav1; reused=median(nav2,nav3), four units per binary each. Do not treat24 navigations as independent observations. Report median/range and all four adjacent A/B process pairs (0/1,2/3,4/5,6/7), plus separate ABBA and BAAB block results.

Call usable time a **repeatable reduction within this workload/matrix** only when pooled process-median reduction>=5%, both block medians favorB, and at least3/4 adjacent process pairs favorB, evaluated separately for fresh/reused. Otherwise state no demonstrated repeatable improvement or inconclusive difference. Report CPU/load/request/byte changes separately; no formal confidence/p-value or causal attribution to one shipped patch. Any failed assertion/missing primary measurement prevents a successful comparison claim; retain partial rows, do not rerun.

Calendar remains a pinned compatibility failure: return navigation precedes todos module readiness on the measured rewrite lane, retaining old input and losing restored search; matched upstream committed the intervening route first. Controlled module gating reproduces the state race on both engines. Not a proven runtime correctness defect. Prior native trace found busy owner CPU rather than slow transport, but80.3ms remained unclassified; rejected full-rate/sampled callback probes do not fill that gap. Constructor-buffer experiment was rejected and is not in either binary. No attribution of this comparison to calendar or unknown80.3ms.
