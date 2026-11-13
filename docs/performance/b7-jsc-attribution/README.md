# B7 JSC sample attribution

The retained profiles show substantial time sampled in host functions and
unidentified executables, alongside active baseline, DFG and FTL JavaScript.
They do not isolate a product CPU bottleneck or justify an optimization. In
particular, `Host` includes JSC builtins as well as embedder bindings; a native
function name does not identify a specific DOM receiver or Rust call site.

## Evidence and reproduction

Input is the evidence-only commit
[1a898d03bf533b9892bb7938e8f07900f38721ef](https://github.com/povio/pyro/tree/1a898d03bf533b9892bb7938e8f07900f38721ef/evidence/b7-jsc-profile-lanes-2).
All 27 entries in its `SHA256SUMS` were verified before aggregation. The older
`a1d841a` evidence has empty B/C profiles and is not an attribution input.

Both B2 and C2 used the signed release diagnostic build
`10571fe2b6efde29b417b9f0214e1fa63e82e89a`, SHA-256
`d9bd2c8bcc1399323330946643dedc62cb8a46e4fc082989a26731451158dc96`,
with `allow-jit`, `PYRO_DIAG_GRACEFUL_SIGTERM=1`, task/module tracing and the JSC
sampler enabled at a nominal 1 ms interval. The diagnostic code is not for merge.
B2 disables the shared asset cache; C2 sets it to 64 MiB. Each completed the
unchanged 13-case B7 selection on TOS `f9fde4aa0d` with 13/13 passing and zero
remaining run databases, as recorded by W2. No workload was rerun for this report.

Run `python3 aggregate.py /path/to/evidence/b7-jsc-profile-lanes-2` to reproduce
[summary.json](summary.json). The script verifies checksums, rejects empty
profiles, and verifies each exclusive tier sum against its recorded total.

## Observations

Percentages below divide by the recorded sample count, not by wall time or
process CPU time. Samples are pooled within a lane, not averaged per process.

| Sample category | B2: 951 samples | C2: 1,987 samples |
| --- | ---: | ---: |
| Host | 302 (31.76%) | 842 (42.38%) |
| Unknown executable | 347 (36.49%) | 580 (29.19%) |
| LLInt | 35 (3.68%) | 36 (1.81%) |
| Baseline JIT | 104 (10.94%) | 172 (8.66%) |
| DFG | 115 (12.09%) | 213 (10.72%) |
| FTL | 47 (4.94%) | 143 (7.20%) |
| RegExp | 1 (0.11%) | 0 |
| Unknown frame | 0 | 1 (0.05%) |
| C/C++ and Wasm categories | 0 | 0 |
| JS builtin, overlapping annotation | 47 (4.94%) | 129 (6.49%) |

The prepared aggregation script originally divided by the sum of all rows,
including `js builtin`. That understates percentages: JSC records builtin
status in addition to the execution tier. This report corrects the denominator.
The exclusive categories sum to each profile's total. The upstream
[SamplingProfiler implementation](https://chromium.googlesource.com/external/github.com/WebKit/webkit/+/ec42d329299fa971b4c1e2c1637289175ffcce74/Source/JavaScriptCore/runtime/SamplingProfiler.cpp)
corroborates that counting convention; it is not proof of the exact proprietary
framework build's implementation.

Selected counts in the *published top-function tables* follow. Those tables
omit lower-ranked functions independently per profile: these sums are lower
bounds on the named function's observations, not a complete ranking or a
partition of all samples.

| Published function name | B2 | C2 |
| --- | ---: | ---: |
| get href | 97 | 158 |
| parseModule | 89 | 143 |
| get length | 15 | 221 |
| stringSplitFast | 45 | 87 |
| setAttribute | 11 | 43 |
| moduleDeclarationInstantiation | 12 | 20 |
| fetch | 16 | 9 |

These native names have no source identity in the retained summaries. For
example, `get length` cannot distinguish collection, window, array or other
receivers. Of C2's 221 published `get length` samples, 210 occur in one 1,041-sample
profile. That concentration calls for a reproducible, receiver-specific probe
before attributing it to a browser implementation defect. `parseModule` and
`stringSplitFast` are engine work, not evidence that Rust DOM bindings dominate.
The DFG/FTL observations establish that optimized JIT tiers executed during these
runs; they do not establish that every hot function reached those tiers.

## Limits and disposition

B2 retains three profiles for four processes (406, 359 and 186 samples); C2 has
four (351, 211, 1,041 and 384). VM-address filenames share a directory across
processes, so an overwritten B2 file is plausible but unproven. The missing
profile prevents claiming complete B2 process coverage. We cannot pair profiles
to individual processes or navigation phases from filenames alone.

The host oversleeps even outside the product; the nominal 1 ms sampling interval
is not a measured cadence. No millisecond conversion, confidence interval,
cache speedup, profiler-overhead estimate or cross-lane CPU comparison is derived
here. Lane A is an older diagnostic build and has no sampled attribution.
A zero C/C++ category is not proof that native execution, GC, networking or
scheduler waits cost nothing. Unknown executable samples remain unattributed;
text summaries do not provide native stacks or complete call graphs.

The task/module evidence and prior modulepreload investigation remain separate
from these samples. The samples do not overturn that investigation or isolate
the cause of timer oversleep. No system settings, debug entitlements or product
behavior changed. This completes bounded attribution; there is no supported
optimization to ship from these summaries alone. If CPU investigation resumes,
use per-process profile directories and receiver-specific bounded instrumentation
with matched workloads and native CPU evidence before changing implementation.
