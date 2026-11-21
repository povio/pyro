# Studio batch 13: conic gradient stops

Studio `0f4857a4`, not coordinator `d1b4e50e2`, gained six named passes and one strict page over batch 12 (`8695db84`). Only `css/css-images/parsing/conic-gradient-calc-angle-percentage-valid.html` changed. No pass or strict page was lost, no assertion appeared or disappeared, and no harness or runner status changed. All 41 supplement outcomes are identical. This review reused the existing 17 reports; it ran no WPT or withheld tests.

Directional counts are **batch 13 only / reference only**. Strict and status columns count URLs; passes and observed count distinct `(URL, assertion name)` pairs.

| Cohort / reference | Strict | Passes | Observed | Complete OK | Complete error | Harness timeout | Runner timeout | Hang |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Main / batch 12 | 1/0 | 6/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Main / frozen | 59/0 | 294/0 | 1/0 | 2/0 | 0/0 | 1/2 | 0/1 | 0/0 |
| Main / pinned Chrome | 292/1349 | 5931/6102 | 260/499 | 15/128 | 75/3 | 55/15 | 4/3 | 1/1 |
| Supplement / batch 12 or frozen | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Supplement / pinned Chrome | 0/8 | 0/42 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |

Main: 5,893 strict pages, 182,792 passes and 194,673 observed across 8,082 URLs. Supplement: 27 strict, 577 passes and 874 observed, all 41 complete OK. `batch13/difference-*.json.gz` preserves the exact directional sets. The six focused changes independently derived from the parent, candidate and repeat receipts at `ce951bbf` equal the entire main pass gain set. All six were Fail → Pass and pass in pinned Chrome 158.0.8127.37; names and statuses are in `batch13/focused-map.json`.

The stable patch ID of the three changed Stylo Rust files matches focused source `4e9ad445`; the full patch ID differs because Studio regenerated `UPSTREAM.patch`. `scripts/vendor/regen-patch --check --base 8695db84 stylo` verified forward reconstruction of the Studio tree and reverse recovery of upstream (`batch13/vendor-check.txt`). The 137,188,560-byte read-only candidate and build output both hashed to `2473204ee982a3f3fbcd6098ad9c75b90371682a6001857678bbe6e860cf367e`; all 17 reports identify this candidate.

Cargo built in 1m 43s (wrapper: 104s). Validation elapsed: 143.506s, earliest log creation to final report modification, excluding build. Overlapping main shard wall times: 58.722–76.297s, median 65.242s; supplement: 0.522s. Four whole-host CPU busy samples in time order: 66.69%, 49.86%, 56.59%, 23.04% (mean 49.045%, median 53.225%). These are host snapshots, not process utilization. `batch13/timing.json` preserves timings and sample timestamps.

From this directory, `python3 -B archive_cumulative.py "$LP_CACHE/studio-r7" --batch13` reconstructs the raw comparisons at frozen WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; `python3 -B map_batch13.py` rebuilds the focused map. The frozen cohort checks cover all 16 disjoint main plans and the 41 supplement URLs, excluding all 39 withheld URLs. Verify the saved receipts offline with `(cd batch13 && shasum -a 256 -c SHA256SUMS)`.
