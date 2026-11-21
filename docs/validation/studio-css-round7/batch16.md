# Studio batch 16: original full runs and matched repeats

The original batch 16 main run gained one strict page and lost seven against batch 14. All 85 formerly passing assertions were **absent**, not recorded failures. On four nine-page repeats in parent/candidate/candidate/parent order, all 36 pages completed with harness OK. Every repeat recovered the exact batch 14 full-run named outcomes (85 Pass, two pre-existing Fail); none reproduced the nine status changes. This does not establish why the original run missed completion, or rule out a load-dependent problem. The repeats do not replace any full-run row.

## Original full runs

Counts are candidate-only/reference-only. Strict means complete OK with a nonempty all-passing assertion list. Names are keyed by URL and assertion name.

| Cohort / reference | Strict | Named passes | Observed names | Complete OK | Complete error | Harness timeout | Runner timeout | Hang |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Main / batch 14 | 1/7 | 27/85 | 0/87 | 0/9 | 1/0 | 0/0 | 8/0 | 0/0 |
| Main / frozen | 64/7 | 325/85 | 1/87 | 2/9 | 1/0 | 1/2 | 8/1 | 0/0 |
| Main / pinned Chrome | 292/1351 | 5937/6162 | 260/586 | 15/137 | 76/3 | 55/15 | 12/3 | 1/1 |
| Supplement / batch 14 or frozen | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |
| Supplement / pinned Chrome | 0/8 | 0/42 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 | 0/0 |

Batch 16 totals: 5,891 strict pages, 182,738 named passes and 194,586 observed names on main; 27, 577 and 874 on supplement. All 27 transitions among names observed on both binaries were Fail → Pass: 13 in `color-computed-none.html`, 12 in `scrollWidthHeight-negative-margin-002.html`, two in `scroll-width-height.tentative.html` (the new strict page). Those gains were not rerun. All 17 ordered candidate plans matched batch 14. The frozen verifier reconstructed the original reports for every reference by URL: 16 disjoint main shards covering 8,082 URLs, plus 41 separate supplemental URLs. None of the 39 withheld URLs was executed.

## Nine changed statuses only

All nine were complete OK in batch 14. Eight batch 16 runner timeouts had zero subtests. The complete error had only the synthetic `no test suite completion` Fail, message `The test never reaches the completion callback.` This is not a named WPT assertion. Exact paths, repeat diagnostics, assertion messages, host samples and report files are in `batch16/matched-repeats/`.

| Page basename | Original batch 16 | Lost passes / names | ABBA page times (ms) |
|---|---|---:|---|
| animation-play-state-005.tentative.html | timeout 15004 ms | 1/1 | 141, 137, 134, 138 |
| box-shadow-valid.html | timeout 15004 ms | 38/38 | 19, 18, 18, 17 |
| pseudo-elements-004.html | timeout 15022 ms | 12/12 | 11, 10, 9, 11 |
| baseline-alignment-affects-intrinsic-size-005.html | harness error 1, 27 ms | 24/24 | 33, 29, 25, 30 |
| grid-item-margin-auto-columns-rows-vertical-lr-002.html | timeout 15025 ms | 2/2 | 12, 10, 11, 11 |
| logicalprops-with-variables-revert.html | timeout 15010 ms | 6/6 | 11, 11, 9, 11 |
| private-shared-stylesheet.tentative.html | timeout 15007 ms | 0/1 | 16, 15, 12, 14 |
| scrollsnapchange-scroll-snap-type-change.html | timeout 15010 ms | 0/1 | 16, 14, 12, 13 |
| variable-reference-variable.html | timeout 15011 ms | 2/2 | 12, 10, 9, 10 |

ABBA used the frozen CSS testharness runner, pinned WPT, wptserve, testdriver, two jobs, fresh contexts and the same nine-URL positive allowlist. No crashtests, withheld pages, native-fault or security cases were run. Runner walls: 234, 223, 188, 217 ms, excluding server startup; zero restarts and watchdog kills. No new Chrome or full run.

## Identity and timing

Batch 14 `8627d988ed60b15992282350b5c17d97cb39d9e7` is the measured ancestor; the literal Git parent of `55bca2409fdf6ac3695558827066ea4f8f3f639e` is `9b8a5d9e19087b5fffb7f846e7fb6996d75f00df`, not batch 15. Candidate tree `40ffa12e36be5f87cd342bb31a7aceff95e8c406`. Both saved executables were independently rehashed: batch 14 `6cd1051702a04a283ccf8cfd80ca305ae9b4f7bebd738f90cd9d5a64858edd87`; batch 16 `d39942da6b64e4bf821ca3baedb45fd223278c300ea294b3394137e2ac0e4bee` (0555, 137,182,208 bytes). All candidate reports name the saved candidate. Frozen source `305840f45afbc4f55bfa24f303c1e858d30e4c1d`, Chrome stable 158.0.8127.37, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; reference provenance is in `frozen/provenance.md`.

Build receipt: 88 s. Original full validation: 147.873 s from first log creation to final report modification, overlapping main shard walls 58.823–79.370 s, supplement 0.684 s. Five whole-host busy samples: 67.20%, 57.88%, 44.53%, 27.13%, 61.53% (mean 51.654%); one-minute loads 22.03, 17.04, 13.81, 10.72, 14.60. Repeat pre-launch host busy snapshots: 25.82%, 67.43%, 27.74%, 26.32%. These are not browser CPU or proof of causality.

From this directory, `python3 -B archive_cumulative.py "$LP_CACHE/studio-r7" --batch16` reconstructs the full comparisons and verifies the saved repeats without launching browsers. `(cd batch16 && shasum -a 256 -c SHA256SUMS)` checks the archived receipts.
