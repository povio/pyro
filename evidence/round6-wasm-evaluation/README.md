# Wasm backend evaluation

The two experimental branches use Wasmi 2.0.2 for Wasm and the pinned JavaScriptCore for JavaScript. Neither is part of the browser conformance branch. Both use source base `e4ca26f4883eb47e507fa8bb2aa87aa6ae9a7d27` and CI JSC `0e0dd7038a7a-6f4649e395e0`. The ordinary WPT corpus is pinned at `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`.

| Exact artifact | Git commit |
| --- | --- |
| Portable interpreter | `b2484cc8557d682cbb416446eb8cde346935f50f` |
| Optimized automatic dispatch | `86f4b697544ba8e1989eb5d9fdeb36c4135e1fa9` |

On 382 matched ordinary Wasm WPT URLs, the two Wasmi variants produced identical outcomes. They passed 18,801 named cases and 14 complete pages, compared with 39,603 and 80 on original LP and 164,142 and 350 on local Chrome stable. Wasmi recorded 11,931 named gains and 32,733 named losses against LP. It reported zero browser restarts in this cohort. Generated tests sometimes change their names or never register after an error. The archive preserves occurrences and missing results.

Four realistic input/output-verified workloads exercised image processing, document indexing, audio filtering and JS callbacks. Automatic dispatch completed those workloads 3.35–4.81 times as fast as portable dispatch. Its warm durations were 6.6–14.4 times the earlier Chrome measurements. The latter comparison spans separate measurement windows. Pinned JSC did not return a checksum on its first application health check, so application timing against JSC is unavailable. The bundle contains all 159 eligible fresh-process measurements and both healthy JSC controls, with the original failure recorded as metadata.

The automatic-dispatch release-fast build was 2,932,064 bytes larger than the matched LP control, or 1,856,768 bytes larger after stripping. The additional code was statically linked. The release-fast comparison did not include hosted PGO training or package publication. The bridge lacks core requirements for a production replacement, including live JavaScript memory/table/global interop and full reference/GC support. The Wasmi dependency raises the minimum Rust version from 1.85 to at least 1.86.

Use the accompanying SHA-256 file to verify the archive, then extract it and verify its SHA256SUMS file. The distribution contains licensed fixtures, a restricted benchmark harness, the source patches, raw per-process timings, build metadata, and the full-outer per-name WPT comparisons. Its JSC harness runs only verified healthy controls. The shipped backend should remain on JSC pending a materially different compatibility and workload result.
