# Studio batches 17 and 18: full-run review

These are the original completed reports, not reruns. Each batch has 16 disjoint ordered main plans (8,082 URLs) and one disjoint supplement (41). All 17 execution plans match batches 14 and 16 in exact order; batch 9's positive shard inputs match their membership, not necessarily runner order. None of the 39 withheld URLs appears. WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, pinned Chrome 158.0.8127.37.

Main totals are 5,902 strict pages and 194,673 observed names for each; 182,830 passing names for 17 and 182,854 for 18. Supplement is identical for both: 27 strict, 577 passing, 874 observed, all 41 complete OK. Combined: 5,929 strict, 195,547 observed, and 183,407 / 183,431 passing respectively.

Main directions, **candidate only / reference only**. OK, error and harness timeout denote completed harness statuses 0, 1 and 2. Counts compare sets, not net totals.

| Batch / reference | Strict | Named pass | Observed | OK | Error | Harness timeout | Runner timeout | Hang |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 17 / 14 | 5/0 | 34/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| 17 / 16 | 11/0 | 92/0 | 87/0 | 9/0 | 0/1 | 0/0 | 0/8 | 0/0 |
| 17 / frozen | 68/0 | 332/0 | 1/0 | 2/0 | 0/0 | 1/2 | 0/1 | 0/0 |
| 17 / Chrome | 292/1340 | 5937/6070 | 260/499 | 15/128 | 75/3 | 55/15 | 4/3 | 1/1 |
| 18 / 17 | 0/0 | 24/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| 18 / 14 | 5/0 | 58/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| 18 / 16 | 11/0 | 116/0 | 87/0 | 9/0 | 0/1 | 0/0 | 0/8 | 0/0 |
| 18 / frozen | 68/0 | 356/0 | 1/0 | 2/0 | 0/0 | 1/2 | 0/1 | 0/0 |
| 18 / Chrome | 292/1340 | 5937/6046 | 260/499 | 15/128 | 75/3 | 55/15 | 4/3 | 1/1 |

Supplement: every direction is 0/0 versus the Pyro references; versus Chrome only strict (0/8) and named pass (0/42) differ. The compressed differences list each URL or `(URL, name)` in both directions.

Both batches pass the four width interpolation assertions lost in batch 15 (restorations, **not** gains over 14). Both retain the five focused Fail → Pass assertions versus 14: three reversal/retargeting and two cancellation. Two additional `transition-behavior.html` passes were outside that focused set. All 24 changes in 18 versus 17 are Chrome-confirmed flex scrollWidth Fail → Pass on `scrollWidthHeight-negative-margin-002.html` (12 visible, 12 clip); no page crosses the strict threshold. Exact focused names and original batch 16 statuses are in `focused-map-{17,18}.json`.

The eight original runner timeouts and one original harness error in batch 16 remain recorded, not replaced by successful repeats. All nine now match batch 14 exactly, including two pre-existing assertion failures: the 87 restored observations contain 85 passes, **not** 87 new passes. The batch 16 owner's matched ABBA evidence (`0aba0c48`) also recovered all nine, without establishing cause. No new loss versus 14 or 16, or in 18 versus 17, warranted another repeat. No security or native-fault replay was performed.

| Batch | Source | Read-only candidate SHA-256 | Build wall | Validation elapsed | Browser / runner CPU | Whole-host busy samples |
|---|---|---|---:|---:|---:|---:|
| 17 | `063c60bb14b4e9c744a75d29c1748522b57e49bb` | `9b72eeb6072cc1133c4c211f5e8975d5b10d68631c5f02e2a8859d911211dcf2` | 56.71 s | 202.741 s | 152.80 / 76.49 s | 12.37–48.10%, n=7 |
| 18 | `83bb4bdbbfe7d5b3fe53f68facbe5986f1ca2192` | `932d425d43a07caf969e394ddfa681cb8f99058786d8653e9243c1b153cc1340` | 10.56 s | 202.227 s | 149.06 / 73.39 s | 12.09–30.52%, n=7 |

All reports name the corresponding 0555 executable; rehashed snapshots match the saved receipts. The shared build output independently matched 18 at review; it no longer represented 17. Source receipts and ancestry, not a reproducible rebuild, bind the snapshots to commits. Stable patch IDs matched the focused reversal (`eefe04af`), cancellation (`8202ed42`), and flex (`8f5ac0a5`) sources. Both builds used three Cargo jobs; reports used two workers per shard. Each full run records one restart, zero watchdog kills and zero measured server CPU. Elapsed is first log birth to last report modification, excluding builds; CPU is summed across all 17 reports. Host samples include unrelated work.

Reconstruct with `python3 -B archive_17_18.py "$LP_CACHE/studio-r7"` then `python3 -B map_17_18.py` from this directory with the referenced evidence commits available. Each batch includes raw report digests, ordered plans, compact outcomes, logs, per-shard metrics and a SHA-256 manifest.
