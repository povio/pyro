# StylePropertyMap on 43 selected WPT pages

Source `939d3d6085ed0716941bdd712d1cd63881454ef5` was tested against `8f0c4154a1fbead80070887bfbc48113646da0c4`. It preserves property-specific typed numeric values on writes, reifies computed and declared values, orders map entries and validates list appends. Unitless line-height retains its number in Typed OM and resolves to unzoomed CSS pixels in CSSOM.

Independent QA matched all 3,142 named assertions by occurrence. Passing assertions rose from 2,930 to 3,033 (+103/−0) and fully passing pages from 0 to 23 of 43. All 103 gains also pass the archived Studio Chrome 158.0.8127.37 profile. The selected pages cover known gaps. [Chrome reference and runner profile](../studio-css-e4ca26/README.md)

The final head ran on 2026-11-21 from 20:34:31 to 20:34:35 UTC. WPT revision was `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, JSC key `0e0dd7038a7a-6f4649e395e0`. Pyro used a single wptserve testharness worker, the pinned manifest inventory, the selection in spm-urls.json and no native retry. No names went missing and no browser restarted. Final WPT outcomes also matched the earlier head `7c3c83537` on every assertion.

All three native guards passed on the final source (0.09 seconds after a 43.53-second test build). Its single-job incremental executable build took 61 seconds. The earlier source took 177 seconds to build and passed two native guards. Its third guard exposed resolved line-height of 40px at 2× zoom where Chrome returns 20px. Separate provenance and results preserve the original failure.

The performance script uses eight fresh browser processes in ABBA/BAAB order. Each process runs ordinary CSSOM width writes and reads, typed opacity writes and reads, and declared StylePropertyMap iteration. Each fixture has one warm-up and five measured samples of 2,000 element operations. Sample wall times include the page evaluation round trip. CPU readings aggregate browser-process-tree CPU time across the five samples.

| Fixture | Base median ms | Candidate median ms | Change | Base / candidate mean CPU ms per sample |
| --- | ---: | ---: | ---: | ---: |
| ordinaryCssom | 7.34 | 7.00 | -4.7% | 8.0 / 7.5 |
| typedWriteRead | 15.15 | 12.66 | -16.4% | 21.5 / 21.5 |
| declaredIteration | 70.16 | 69.38 | -1.1% | 81.5 / 83.5 |

These are descriptive medians from 20 measured samples per arm per fixture. The typed-write wall difference was not accompanied by a change in mean CPU; iteration mean CPU rose by 2 ms. Host occupancy and other workloads affect the timings.

The eight timed processes ran from 20:35:13.192 to 20:35:20.056 UTC. One-second samples weighted by process duration measured 44.39% host CPU overall, 41.96% during baseline processes and 46.97% during candidate processes. CI was active. Census, StylePropertyMap QA and popover QA had zero overlap. [Per-process host measurements](host.json)
