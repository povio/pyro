# Signed zero in CSSOM declaration updates

Exact source `eb0eeb5809cccf7d700e7e4b88475783953184b2` was independently compared to integrated base `fd40461ffa223d0449ab26d8c2e768171aebfe28`. On 17 ordinary pinned WPT pages, passes increased from 523 to 534 out of 554 named observations. Fully passing pages increased from 11 to 12. All 11 gains were retained through the final optimization, including eight sign-function assertions and three typed arithmetic assertions. The other 15 pages retained their named outcomes. No selector, harness status or observation changed, and there were no restarts or watchdog kills.

CSSOM publicly serializes negative zero as zero. The parser must still see the original sign inside unresolved calculations, where division or sign() can observe it. The change retains a private representation when needed while preserving the public attribute, selector behavior and rule cssText. It handles inline, stylesheet, nested, constructed, keyframe and page-margin declarations. Style sharing distinguishes unequal unresolved math, and CSSOM writes to independent longhands can reuse validated declaration ranges. The cache holds at most four slots for blocks of at most 8 KiB. No omitted security or native-fault selector was replayed. [Signed zero in CSS Values 4](https://drafts.csswg.org/css-values-4/#calc-ieee)

## Cumulative patch for the integration branch

The intermediate change was reverted on the shared branch. Apply the entire source delta from `fd40461ff` through `eb0eeb580`. Merging only the commits after the revert can omit the original changes. The supplied patch was applied to a temporary Git index loaded from the exact base, and its resulting tree was verified against the source commit.

| Artifact | SHA-256 |
| --- | --- |
| Decompressed cumulative Git patch | `6c65857b065cf89493e58c4b3b6a5be6d470cb16263be5716996bc53031efb46` |
| Compressed cumulative patch | `112c0fc0597551fdef67bd71e1ecf0c71ad7180992f0a8a349744cc459f93347` |
| Tested base executable | `485ce56577ebdae1bec31181511f2ef716db501fbe7314e2607eeea2d64f7470` |
| Tested candidate executable | `561ce54b82d3d59c87649ef09ebceab4713474e3d7fbf6fb1e54cfd166a7bbf0` |

Base tree `fc1e1db9910091ba59c86d752956a2cd6780dde3`. Reconstructed source tree `538312b8404e6ac1d83b73217358c92dc17597d6`. The cumulative patch excludes this evidence directory. [Patch](cumulative.patch.gz) · [Patch receipt](patch.json) · [Independent WPT differential](differential.json)

## Representative setter costs

Each sample ran 3,000 iterations with two assignments per iteration against one live DOM element. There were 20 timed samples per fixture and binary, after a warmup for every fixture and process. Binaries ran sequentially in ABBA/BAAB order with fresh isolated ports. Wall medians include the single CDP evaluation per sample. CPU figures are mean process-tree CPU time per sample. These workloads measure repeated mutation, without an interleaved layout read on each write.

| Fixture (6,000 writes) | Base wall ms | Candidate wall ms | Base CPU ms | Candidate CPU ms |
| --- | ---: | ---: | ---: | ---: |
| Ordinary literal declarations | 18.09 | 11.21 | 17.0 | 10.5 |
| Ordinary functional declarations | 23.07 | 11.11 | 22.5 | 11.5 |
| Eight rotating target properties and margin-left | 38.04 | 31.10 | 38.0 | 32.0 |
| Signed-zero declaration plus unrelated writes | 19.70 | 11.47 | 20.0 | 12.5 |

The earlier `168067d79` run had medians of 24.55 ms ordinary, 33.90 ms functional and 69.93 ms signed, versus 22.26, 29.04 and 29.31 ms on its paired base. It ran at 94.4% aggregate host busy time. The final run took place at 19:12:35.709–19:12:42.972 UTC, with 61.70% aggregate host occupancy (base arms 58.28%, candidate arms 65.77%). One census shard and CI were active. Numeric QA finished before this timing interval. Comparisons above use the base measured in the same run. These measurements do not establish the cost of arbitrary CSS workloads or memory use. [Timed rows](cost-nine.jsonl) · [Host samples and overlap](host.json) · [Benchmark driver](cssom-cost-repeat.cjs)

## Validation

Independently checked WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, JSC key `0e0dd7038a7a-6f4649e395e0`, and the explicit URL-list SHA-256 `bb9b767738dba65bfe891befce74e36c3145f924d8af7ede64b0e4140fff2b55`. QA ran one browser worker. The final executable built in 73 seconds with one Cargo job. Eight connected browser guards and one parser unit test passed, including connected keyframe edits, suppressed !important declarations, cloned inline styles, margin-rule CSSOM text, direct attribute replacement, shorthand expansion, external edits and byte-range cache invalidation. [Raw base](parent.json.gz) · [Raw candidate](candidate.json.gz) · [Incremental comparison](incremental.json)
