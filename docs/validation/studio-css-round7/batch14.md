# Studio batch 14: mixed flex-column baselines

Studio `8627d988` gains four strict pages and four named passes over its parent, Studio batch 13 `0f4857a4`. Each gain is `#target > div 3`, Fail → Pass, on a different page: `align-items-baseline-column-horz.html`, `align-items-baseline-column-vert.html`, `alignment/flex-align-baseline-006.html`, and `alignment/flex-align-baseline-007.html` under `css/css-flexbox/`. These are exactly the four changes independently derived from the focused parent/candidate receipts at `f3ba721f`; both focused and pinned Chrome pass all four. There are no other named status changes, missing or new names, harness or runner status changes, or lost strict pages versus batch 13. No repeat is indicated.

Counts are **batch 14 only / reference only**; passes and observed names are `(URL, name)` pairs, all other columns count URLs.

| Cohort / reference | Strict | Passes | Observed | Complete OK | Complete error | Harness timeout | Runner timeout | Hang |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Main / batch 13 | 4/0 | 4/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Main / frozen | 63/0 | 298/0 | 1/0 | 2/0 | 0/0 | 1/2 | 0/1 | 0/0 |
| Main / pinned Chrome | 292/1345 | 5931/6098 | 260/499 | 15/128 | 75/3 | 55/15 | 4/3 | 1/1 |
| Supplement / batch 13 or frozen | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Supplement / pinned Chrome | 0/8 | 0/42 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |

Main: 5,897 strict, 182,796 named passes, 194,673 observed. Supplement: 27 strict, 577 named passes, 874 observed; all 41 complete OK. The frozen verifier checked existing raw reports at WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. All 16 batch 14 main reports match their batch 13 shard membership and archived batch 9 plans; the disjoint union is exactly 8,082 URLs. Pinned Chrome's shard boundaries differ, so compare by URL across its complete union, not by shard number. The supplement covers exactly the separate 41 URLs; none of the 39 withheld URLs appears. No tests were rerun and no expectations changed.

All 17 candidate reports identify the same read-only (0555) 137,189,216-byte executable. At verification the saved candidate and separate build-output inode both hashed to `6cd1051702a04a283ccf8cfd80ca305ae9b4f7bebd738f90cd9d5a64858edd87`, matching the build receipt. The complete vendor/taffy tree matches focused source `3b25717c` (Git tree `18cc679989053a364efeec3ec40568a0ae480c61`). Cargo reported 53.74 s. Excluding the build, validation elapsed 139.174 s from first log creation to final report modification. Overlapping main shard walls: 58.458–75.736 s, median 64.445 s; supplement: 0.460 s. Five whole-host busy samples in time order: 38.75%, 23.90%, 30.00%, 29.26%, 7.90% (mean 25.962%, median 29.26%); these are not process CPU percentages.

From this directory run `python3 -B archive_cumulative.py "$LP_CACHE/studio-r7" --batch14` then `python3 -B map_batch14.py` to reconstruct the comparisons. Exact differences, source and binary receipts, raw report digests, shard plans, timing samples and the focused map are under `batch14/`. Verify the saved files with `(cd batch14 && shasum -a 256 -c SHA256SUMS)`.
