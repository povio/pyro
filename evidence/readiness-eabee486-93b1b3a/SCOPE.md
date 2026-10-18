# Scope and remaining limitations

This is the final bounded performance comparison for this assignment. Evidence only: no implementation, app, test assertion, auth, manifest, timing workaround, scheduler change or profiling instrumentation is included.

## What current main adds over the retained production baseline

- PR129 caches the parsed URL base used by reflected URL properties with bounded native storage and mutation/adoption/base invalidation. Its prior isolated Vite-shaped loop showed a useful gain; that microbenchmark is not a page-load prediction.
- PR126 corrects native layout behavior used by select/form controls. PR127/128/130 add native/browser/CDP frame focus ownership and input routing. PR131 reveals default focus targets before focus events and honors preventScroll. These are behavior repairs, not independently established page-readiness optimizations.
- PR125 preserves automatic JIT signing through release stripping. Both binaries in this comparison have verified allow-jit, so this is not a JIT-versus-no-JIT experiment.
- PR123/124 add evidence only. Earlier shipped runtime microtask/iframe/FileReader/URL/other compatibility changes are already in eabee486; this comparison does not isolate them or compare the Rust browser with Zig/upstream.

The discarded native-constructor stack-buffer candidate and full-rate/sampled native callback instrumentation are absent from both production trees. No new optimization was shipped merely to produce a favorable result.

## Calendar remains unresolved as compatibility behavior

The unchanged calendar-filter test still failed on pinned rewrite4e89ab4 (production-equivalent then-main420d126). Its routed helper returns after synchronous history/popstate and a zero-delay timer; it does not wait for the intervening todos route's lazy modules or React commit. Retained matched observations showed upstream2ad completing that route, removing the old input, then returning to a new calendar input with saved search. Rewrite returned to calendar before those module bodies executed; the same connected input remained and became empty. This is a measured ordering difference and compatibility race, not proof of a runtime correctness violation. Controlled module gating reproduced the state race in both Chrome and rewrite. We did not add waits or change assertions/auth to hide it.

Published evidence: `evidence/calendar-module-boundary-e7bb844` at16b752df73bce785e0ebf4ee8f26c88776e67097, and `evidence/calendar-module-stages-062e84f` at7716198a3213ce0043833e814855bb3d4ce44fa2. W1 owner partition atb78075ff34658b4e900c14232436c90bcc8b7fd9 records109.134ms owner CPU over109.539ms push-to-component elapsed, with80.326ms outside measured module-call spans. Both targeted transports were under0.6ms. These are diagnostic windows, not this final benchmark or exclusive function costs.

That80.3ms was not resolved into reliable native/application attribution. Full-rate callback counters materially perturbed cost; sampled counters missed rare long calls and failed their accuracy gate. Neither rejected probe supports application attribution. Host timer oversleep was independently observed but its exact cause remains unknown; the captured busy-owner interval does not establish an idle/wakeup bottleneck or justify a scheduler tweak.

## Interpretation boundaries

Seven original assertions define observable usability and include sequential locator polling, host scheduling and protocol time. Load/DCL are host-observed event arrival, not first usable content. Network events count only the observed windows, with a retained one-second horizon after usability; later lazy assets need not block the user, and later events are out of scope. The collector has no DOM readiness observer, but its host listeners, synchronous saves and native sampler still have cost.

All-browser-thread CPU can exceed wall time, and sampled RSS is not live heap. Setup/build/auth/DB costs are excluded from navigation metrics. Fresh processes reset browser cache state, not the operating system or service caches. Reused measurements share a page/context and are clustered within processes. Four processes per binary support a bounded descriptive comparison, not a population-wide confidence claim. A current-main gain, if present, cannot be assigned to a single patch without another experiment; none is authorized after this report.
