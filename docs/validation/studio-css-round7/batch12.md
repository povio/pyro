# Studio batch 12: table boxes and flex column baselines

**This is Studio `8695db84513be44b2aedc700e3e042ee9354b2b4`, not a run of coordinator integration `0038d6580`.** Compared with Studio batch 11, eight main URLs changed: 29 named gains, four newly strict pages, no lost passes or pages, no observed-name or harness/runner status movement. All 41 supplement outcomes are identical. No repeat was indicated. This review reused completed receipts; it executed no WPT tests.

Directional counts are **batch 12 only / reference only**. Strict and status columns count URLs; named pass and observed count distinct `(URL including query, assertion name)` pairs.

| Cohort / reference | Strict | Named pass | Observed | Complete OK | Complete error | Harness timeout | Runner timeout | Hang |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Main / batch 11 | 4/0 | 29/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Main / frozen | 58/0 | 288/0 | 1/0 | 2/0 | 0/0 | 1/2 | 0/1 | 0/0 |
| Main / Chrome | 292/1350 | 5931/6108 | 260/499 | 15/128 | 75/3 | 55/15 | 4/3 | 1/1 |
| Supplement / batch 11 or frozen | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Supplement / Chrome | 0/8 | 0/42 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |

Main totals: 5,892 strict pages, 182,786 named passes, 194,673 observed; 7,873 complete OK, 95 complete error, 102 harness timeouts, 11 runner timeouts and one hang. Supplement: 27 strict, 577 named passes, 874 observed, all 41 complete OK. Chrome-only results are parity gaps, not losses against batch 11.

## Focused receipts versus full result

`batch12/focused-map.json` records batch 11, batch 12 and pinned Chrome status for every focused gain. Reconstructing the table parent/final and repeat receipts at `1c879c43` yields 22 distinct keys across five URLs; the flex parent/final receipts at `897d8cc0` yield seven across three URLs. Their intersection is empty; their union equals the full matched pass gain set, with no missing or additional keys. All 29 were Fail → Pass. Pinned Chrome passes 28; it fails the explicit consecutive columns / border-spacing assertion in `html5-table-formatting-3.html`. This is a WPT gain, not a Chrome parity gain. The newly strict pages are `bounding-box-computation-1.html`, `bounding-box-computation-2.html`, `visibility-collapse-row-group-001.html` and `flex-align-baseline-004.html`.

## Provenance and timing

The clean Studio source descends from batch 11 by `a502e6db` and `8695db84`; their stable patch IDs match the focused `12b3480c` and `a4eb5356`, respectively. The repository's `scripts/vendor/regen-patch --check --base 1ee620d4 blitz-dom taffy` independently verified the auto-merged blitz-dom patch and Taffy patch forward and backward, using upstream reconstructed from batch 11 (`batch12/vendor-check.txt`). The read-only candidate and the build output both hashed to `a81addbbb01b697c4754f3bb9d9db1074bb20862bbd38e41387bc9ebfa02032f`; all 17 reports name this candidate.

`python3 -B archive_cumulative.py "$LP_CACHE/studio-r7" --batch12` verifies the source and executable receipts, reconstructs raw batch 12, batch 11, frozen and Chrome outcomes with the frozen verifier, and checks the references against their archives. The 16 main plans match the batch 9 plan bytes and report membership, are disjoint and cover 8,082 URLs at WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. The 41 supplement and 39 withheld URLs are disjoint from main and each other. Withheld URLs were not executed. Chrome is the pinned stable 158.0.8127.37 reference. `python3 -B map_batch12.py` rebuilds the focused map; `shasum -a 256 -c SHA256SUMS` inside `batch12/` verifies the evidence offline.

Cargo build: 57.84 s (wrapper: 58 s). Validation: 139.378 s from earliest log creation to final report modification, excluding build. Overlapping main shard wall times: 58.405–75.879 s, median 65.031 s; supplement 0.466 s. Five whole-host CPU busy samples in order: 52.57%, 36.53%, 29.52%, 28.21%, 18.33% (median 29.52%, mean 33.032%). `batch12/timing.json` preserves individual shard timings, sample timestamps and load averages. These are host snapshots, not process utilization or a speed benchmark.
