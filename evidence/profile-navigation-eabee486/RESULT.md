# Exact-main diagnostic profile navigation

Browser eabee486709f79167520a46887cd6f303b26711d, signed binary66e7448e07666176e9650ba714cba04ecc570c65f6e6139ad5bd759f5b49ad88. Explicit allow-jit and JSC_usePollingTraps=true. All12 navigations completed the seven original profile-settings assertions. This external hard-navigation diagnostic is not an unchanged suite run.

|Shared MiB|Process|Nav|Usable ms|DCL ms|Load ms|Browser CPU to usable ms|RSS usable MiB|Cache-hit requests|Last asset ms|
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
|0|1|1|595.6|269.1|269.1|845.1|210.0|0|1384.6|
|0|1|2|350.7|175.3|175.3|584.7|298.0|285|1276.0|
|0|1|3|367.0|193.1|193.1|646.1|383.8|285|1218.4|
|0|2|1|557.7|236.0|236.0|803.8|213.3|0|1364.6|
|0|2|2|360.2|184.5|184.5|588.2|298.3|285|1190.9|
|0|2|3|359.2|188.0|188.0|626.9|384.9|285|1165.7|
|64|1|1|558.1|238.0|238.0|796.2|212.9|0|1356.6|
|64|1|2|359.3|185.4|185.4|582.7|297.8|285|1316.3|
|64|1|3|364.8|192.4|192.5|644.1|384.5|285|1290.7|
|64|2|1|553.1|235.5|235.5|801.2|216.8|0|1348.9|
|64|2|2|359.8|184.3|184.3|582.4|298.1|285|1225.2|
|64|2|3|465.9|192.1|192.1|733.0|450.8|285|1276.2|

Private context cache remains64MiB in both lanes. Shared0 is not all-cache-off. Process1/2 are fresh; nav2/3 reuse the same context/page and retain caches. No route/autosave mutation. Original TOS f9f fixtures, assertions, auth and manifest unchanged; nativePG template reused from same UTC date/fixture ledger, per-run DB cleanup0 remaining, three ledger templates retained. Setup/auth/DB costs are outside navigation windows and in setup-timings/coverage.log.

Usable time is the completion of sequential Playwright assertions and includes polling/host scheduling. DCL/load are host-observed events; the optional document readiness marker was null for all12 and is unavailable. Network horizon is scheduled1000ms after usability; actual horizon is retained per navigation, not a blocking last-asset deadline. Last asset is the last observed Script/Stylesheet/Image/Font/Media event within that horizon; later activity is outside scope.

CDP dataLength and data-event encodedDataLength both count decoded chunks on this implementation; only dataLength is summed as decoded bytes. loadingFinished encodedDataLength is separately reported as encoded transport body bytes, not total HTTP wire bytes/headers. fromDiskCache means in-memory cache here. Browser native proc_pid_rusage CPU includes all browser threads; it can exceed wall time. RSS is sampled retained memory, not live JS heap, and no forced GC was performed. Sampler launch/reading and observer/assertion instrumentation have overhead; timestamps expose sample windows.

The preceding /private/tmp/lp-profile-nav-eabee486 run lacked allow-jit and is separate diagnostic evidence. No pooled signed/non-JIT speed claim; sequential runs confound drift. No full TOS, general load-performance or memory-leak conclusion.

Baseline host timestamps are monotonic performance.now values; timeOrigin was not persisted. Native sample epoch and its host invocation interval are retained, so absolute navigation epochs are bounded rather than exact. The subsequent attribution collector explicitly records epoch markers.
