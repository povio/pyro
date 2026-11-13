# Observer-free confirmation and native attribution

The optional readiness observer in the initial matrix repeatedly scanned inputs/labels and never disconnected. Preserve that evidence as instrumented observation; use the following separate observer-free confirmation for the bounded profile baseline. No production bottleneck is established by the old matrix.

All three confirmation navigations passed the same seven original assertions, using exact main eabee486 and signed binary66e7448e07666176e9650ba714cba04ecc570c65f6e6139ad5bd759f5b49ad88. Shared/private cache64/64MiB, one fresh process, three hard navigations in one context/page. Explicit polling traps and allow-jit. No original source/auth/assertion/manifest changes.

|Nav|Usable wall ms|Browser CPU to usable ms|CPU after usable through horizon ms|RSS usable MiB|
|---:|---:|---:|---:|---:|
|1|598.23|838.65|34.77|210.1|
|2|361.84|599.59|110.69|301.3|
|3|367.13|632.12|119.96|386.1|

These are a small diagnostic, not a general speed/memory conclusion. Browser CPU includes all threads, so can exceed wall time. Assertion polling is included; backend/DB/auth setup is outside these windows. No forced GC; rising RSS alone does not establish a leak. Actual post-usability horizon1001–1006ms is recorded rather than assumed exactly1000ms.

The subsequent **diagnostic-only** ec6ae038fb71f0aecf6e601b8dc6d88b8af5d18c binary a0345b884025c8c8903f4223697235f64f932063cab69f90d822951332e1fdf6 also passed3/3 with the observer-free collector. Never merge its instrumentation or treat its timings as the baseline. W1's separate synthetic calibration measured material counter overhead (+27.6%CPU for that particular workload), not a universal correction factor. Navigation1 additionally includes one bounded native sample. Signed separately with allow-jit; polling explicittrue.

|Nav|Instrumented usable wall ms|All-thread CPU to usable ms|pyro-js included CPU ms|pyro-runtime included CPU ms|
|---:|---:|---:|---:|---:|
|1|969.03|885.10|521.04|502.03|
|2|604.80|854.82|481.85|470.57|
|3|618.95|911.30|479.26|466.99|

The pyro-js and pyro-runtime interval trees overlap: **do not add them**. Roots crossing each requested epoch window are excluded, never prorated; complete boundary root metadata is in each navN-usable/after-usable/horizon.json. Native categories can include unwrapped FFI/JS. Within pyro-js included roots, js.call accounts for333/295/289ms, native.guard64/63/64ms, HTMLLinkElement45/44/44ms. This identifies candidate areas for source investigation, not an optimization or attribution to one specific application function. Runtime included CPU after usability is114/14/16ms, while much of its elapsed post-usability window is waiting.

Both runs used the same unchanged f9f batch16 fixture/nativePG/auth helpers and same-day template ledger; each cleanup verified0 run databases remaining with three ledger templates retained. No duplicate full suite or TOS compatibility claim.

Exact collector hashes, host monotonic/epoch markers, browser PID-only native sampler records, and sanitized Network type/count/byte events are in each subdirectory. Full untruncated native trace CSVs are stored losslessly as gzip with raw SHA256/length manifest; decompress before using W1's diag/current-loading/summarize.py. Full native sample stays locally retained; the published subset includes static native frames only and cannot be interpreted as CPU percentages.
