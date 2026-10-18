# Current-main readiness: repeatable reused-context improvement, fresh result below gate

Exact main **93b1b3a** reached the selected profile content sooner than retained production **eabee486** on reused contexts in this bounded same-host matrix: **415.1 → 364.3ms (12.2%)**. Both balanced blocks and all four adjacent process pairs favor current main, passing the rule frozen in [PLAN.md](PLAN.md). Fresh-process readiness was **553.6 → 539.5ms (2.6%)**, below the predeclared5% gate, with essentially unchanged CPU. We do not claim a demonstrated meaningful fresh-process speedup.

This is a combined production comparison, not attribution to one patch, a full TOS suite, or a Rust-versus-upstream benchmark. It completes the authorized performance experiment scope. No additional profiling, speculative optimization or performance run follows this report.

## Results and variance

Eight distinct browser processes ran in **ABBA BAAB** order; each performed three hard navigations on one page/context. All24 navigations completed the same seven original assertions, with no failures, retries or replacement samples. One external diagnostic matrix test passed. Independent units are four processes per build: fresh=nav1, reused=median(nav2,nav3) for each process. Values below are medians of those process values, with observed ranges; these are descriptive distributions, not confidence intervals.

|Metric|eabee486 baseline|93b1b3a current|Change|
|---|---:|---:|---:|
|Fresh usable, ms|553.645 [552.806–595.826]|539.467 [538.324–541.628]|2.56% lower; below gate|
|Reused usable, ms|415.061 [409.805–462.289]|364.334 [343.164–403.901]|12.22% lower; gate passed|
|Fresh browser CPU to usable, ms|789.498 [778.234–847.982]|790.026 [767.536–795.900]|0.07% higher|
|Reused browser CPU to usable, ms|659.095 [648.986–695.987]|611.787 [587.525–639.559]|7.18% lower|
|Fresh load event, ms|238.275 [236.027–269.533]|231.364 [228.898–235.960]|2.90% lower|
|Reused load event, ms|189.140 [187.487–192.316]|185.122 [183.352–186.551]|2.12% lower|

Reused readiness block reductions are8.76% and17.58%; adjacent pair reductions are15.98%,1.44%,8.47%,25.77%. The direction repeats, but the magnitude varies substantially. Fresh blocks favor current by6.20% and2.33%, yet pooled median reduction remains below5%. Reused CPU also favors current in all four pairs; fresh CPU has mixed direction and no median improvement. These observations support a bounded reused-context gain, not a universal speedup or an exclusive JavaScript/native CPU explanation.

Individual reused navigations span357.944–472.422ms for baseline and338.193–463.158ms for current. Sequential assertion polling, host scheduling, JSC compilation/GC and the collector's own overhead remain included; there is no reliable attribution of the slower modes. We did not remove outliers or rerun to obtain a cleaner distribution. [Generated tables](TABLES.generated.md) contain every navigation, ordinal distributions and pair comparisons; [analysis.json](analysis.json) retains full precision.

## Equal observed request work; setup and lazy assets are separate

Every navigation in both builds observed293 requests/responses/completions,4,903,041 decoded bytes and zero failed or unassociated events within its measurement horizon. Fresh nav1 had zero cache responses and4,862,472 encoded response-body bytes; reused nav2/3 had285 in-memory cache responses and15,738 encoded response-body bytes. These are CDP response-body counters, not total HTTP wire bytes/headers. Decoded dataLength is summed once; data-event encodedDataLength is not independently added.

The existing horizon ended1001.13–1002.97ms after assertion usability. Last observed asset completion was1182.09–1359.36ms after navigation start across the matrix; it is neither the usable-content time nor proof that every later request had finished. Load/DCL are host-observed events. Per-navigation counts/bytes through usability and through the horizon are both retained.

Setup/auth/browser-facade preparation was outside the navigation windows: launch setup times656.9,509.7,72.8,74.6,75.2,75.6,76.6,74.6ms. The complete harness took41.60s including the24 one-second observation horizons and setup/cleanup. DB/build time is not charged to page readiness. NativePG reused the recorded same-fixture template; the run-owned DB was removed, leaving **zero run databases and three retained ledger templates**. Original auth remained backend test-token/localStorage, not UI login; the documented direct-lane version-gate setup remained unchanged.

## Exact build and collector provenance

Both lanes ran sequentially on the same **Apple M1 MacBookAir10,1, macOS26.6 build25G72,8GiB**, with rustc1.99.1/LLVM23.1.1, SDK26.5/minOS14.0 and optimized release DEBUG0. Both immutable binaries had verified allow-jit and explicit polling traps=true. Shared cache64MiB/private context cache64MiB were fixed. Fresh processes reset browser cache state; OS/server/database caches were not reset. Nav2/3 reuse the context/page and browser caches. No concurrent owned build/test was active during the matrix; other host activity is uncontrolled.

|Lane|Source / tree|Binary SHA256|
|---|---|---|
|A|eabee486709f79167520a46887cd6f303b26711d / a30b943b31b44e211e06b5184023877a6afac7f0|66e7448e07666176e9650ba714cba04ecc570c65f6e6139ad5bd759f5b49ad88|
|B|93b1b3a7c360cd892084a7e4d538024ead798fdc / 1daa4a31a0e4ea69e5b29fd0448486f3f5172063|02009fad9f28aa349ff88bb9b994ac160729b6b8b1ac55798a73de1e0969b633|

A is the retained production binary, manually re-signed after historical post-link stripping; B is a fresh locked build with the merged stripnone/automatic-signing policy. Both have JIT enabled, so this does not measure a JIT-enable effect. Production source states were clean. The f9fde4aa0d fixture retained its preexisting generated pdfgen file; original selected assertions/auth/manifest and all1229 static assets were unchanged. Full [binary provenance](../final-readiness-eabee486-93b1b3a/binary-provenance.json), [host provenance](../final-readiness-eabee486-93b1b3a/host-provenance.json), [source/cleanup verification](../final-readiness-eabee486-93b1b3a/VERIFICATION.json) and protected-file checks are retained.

The observer-free collector is `dac431328d13ff35ba630ded618c6540e19c6e79dc33d43e55070529f55e0f4e`, derived from validated `15df2156` only by binary/order labels and output path. Independent byte comparison confirms that navigation, seven assertions, sampling and horizon code are unchanged. It installs no app observer/rescan or page instrumentation, but host Network listeners, saves and native sampler launches still have overhead. The inherited collector saves its cumulative output synchronously after recording navigation start, so serialization/disk cost is included and may vary with launch order; the balanced order does not isolate that overhead. Observed start-to-commit arrival spans6.45–20.36ms, which includes navigation and is not a save-cost measurement. CPU is all-browser-thread user+system time across the recorded sample intervals; it can exceed wall time. RSS is sampled process memory, not live heap. Reused median RSS is374.3→348.9MiB while fresh is210.7→216.4MiB; no general memory-win or leak conclusion follows without heap/GC control.

## Shipped scope and unresolved limitations

The comparison includes the shipped URL-base cache, geometry corrections, native/browser/CDP frame focus routing, focus scrolling and signing policy. It does not isolate their contributions. The [scope inventory](SCOPE.md) identifies these changes and preserves earlier attribution limits. Rejected constructor-buffer and callback-instrumentation trials are absent from both binaries.

The calendar-filter compatibility failure remains unresolved and was not rerun here. Matched earlier observations showed rewrite returning before todos module readiness/React commit, retaining the old input; upstream committed the intervening route first. Controlled module gating reproduces that state race on both engines, so it is not established as a runtime correctness defect. Earlier diagnostic trace found active owner CPU and sub0.6ms target transport, with80.3ms still unclassified. Rejected intrusive/sampled probes do not resolve that attribution, and host timer oversleep has no established environmental cause. No scheduler fix, assertion wait or auth change is justified by this report.

## Reproduce analysis without running the workload

From repository root:

```sh
python3 evidence/readiness-eabee486-93b1b3a/verify.py
python3 evidence/readiness-eabee486-93b1b3a/analyze.py evidence/final-readiness-eabee486-93b1b3a/observations.json
python3 evidence/readiness-eabee486-93b1b3a/tables.py evidence/readiness-eabee486-93b1b3a/analysis.json
```

The generators emit JSON/tables only and cannot overwrite this narrative. Raw matrix source: evidence-only30608fa; observations SHA256 `c89f32fae9841ac46d34d8366adc86a874fb1974091bb07b80fdc8f7b5db3627`. All27 raw artifact hashes were independently verified. This report requests focused evidence/maintainability review; worker execution and descriptive measurements do not constitute independent correctness or full compatibility approval.
