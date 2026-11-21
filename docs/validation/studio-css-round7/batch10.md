# Batch 10: cumulative flex baseline export

The 8,082-URL main run gained **6 strict pages and 107 distinct named passes over batch 9, with zero losses**. All 107 passes are also passes in pinned Chrome. Observed-name and harness/runner status memberships are unchanged from batch 9. No loss requires a matched repeat. No suite or withheld URL was run for this review.

| Main reference | Pages gained / lost | Named passes gained / lost | Names observed only in batch 10 / only in reference |
|---|---:|---:|---:|
| Batch 9 | 6 / 0 | 107 / 0 | 0 / 0 |
| Frozen | 49 / 0 | 252 / 0 | 1 / 0 |
| Pinned Chrome | 292 / 1,359 | 5,930 / 6,143 | 260 / 499 |

Batch 10 passes 5,883 pages and 182,750 names; 194,673 names were observed. Status counts: 7,873 complete/OK, 95 complete/error, 102 complete/harness timeout, 11 runner timeouts and one hang. All 41 supplement outcomes equal batch 9's keyed outcomes (27 pages, 577 passes, 874 observed, all harnesses OK). Supplement page/pass and observed sets also equal frozen; Chrome passes eight additional pages and 42 additional names. `batch10/difference-*.json.gz` contains exact directional memberships, including every status, not just net changes. `outcomes-*.json.gz` retains keyed statuses and assertions for each cohort.

## Independent checks

- Batch 10 source `c10efae54a7ce4c25be1d8bab090fcc34cab6e7b` directly parents back to batch 9 `5503e80ece0f148851478a65fdd36de7d10be500`. Their candidate digests verify against their respective receipts: `c1bdfd0cf717c115508d834f1ac5a7a808adb3f6f1324fb732c4d7e5b6024c1a` and `bc65351d7fd6651754ce5b48ed0a11102f8389f846c538aa460de12205363644`. The build log selects patched JSC `0e0dd7038a7a-6f4649e395e0`; the wptiter build finished in 51.49 s.
- Integrated and focused (`51a14fdc`) changes have the same stable patch ID, `9489514fcfa2602f9164177ed2856ed46c80b1c2`, not the same binary. The focused parent and candidate hashes matched the separate receipts in evidence commit `c98838f2d6024320afb22458108768d996a227f3`. Independently reconstructed all six focused reports against the two archived manifests and keyed outcomes. The 59 flex/control URLs gained 4 pages and 61 passes; the five disjoint consumer URLs gained 2 pages and 40 passes. Neither lost pages, names or passes. The full run found six more passing assertions across five `css/css-inline/baseline-source` URLs. Native focused receipt: 28 passed, zero failed.
- Verified all sixteen reused batch 9 plan files byte-for-byte against the prior archive, each plan against its batch 10 report, and their disjoint union against frozen main. The 41-URL supplement and 39 withheld URLs are disjoint from main and each other. Checked WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603` against the checkout and reports. Reconstructed batch 9, frozen, and pinned Chrome main and supplement outcomes from original raw reports and compared them to the checked-in baselines. The installed stable version matches the historical pin `158.0.8127.37`; this is not a fresh Chrome run.

## Timing and verification

Combined validation elapsed **143.183 s**, from earliest runner-log creation to final report modification, excluding the build. Shard wall times ranged from 58.475 to 75.709 s (median 65.161 s); they overlap. Four whole-host CPU busy samples fell inside the validation window: 63.80%, 30.00%, 35.45%, 21.18%, in time order. Min / median / mean / max: **21.18 / 32.725 / 37.6075 / 63.80%**. This is sampled machine load, not process utilization. `batch10/timing.json` retains timestamps, load averages and report durations.

Rebuild from existing receipts with `python3 -B archive_cumulative.py "$LP_CACHE/studio-r7" --batch10`. From `batch10/`, run `shasum -a 256 -c SHA256SUMS` to verify the archive without the host cache.
