# Studio batches 5, 7, 8 and 9: independent evidence

All counts are distinct. Each row covers exactly the 8,082 main URLs. Gains are strict pages / named passes; neither set lost a member against the previous listed head or frozen. Results are observations, not attribution to individual commits. Batch 7 is compared with 5, not 6.

| Batch (source) | Pages / named passes | Gain vs previous | Gain vs frozen | Candidate-only vs Chrome | Chrome-only vs candidate |
|---|---:|---:|---:|---:|---:|
| 5 (`3d9c4ae4`) | 5,857 / 182,599 | 5 / 5 vs 4 | 23 / 101 | 291 / 5,930 | 1,384 / 6,294 |
| 7 (`e5b9ecf3`) | 5,862 / 182,617 | 5 / 18 vs 5 | 28 / 119 | 292 / 5,930 | 1,380 / 6,276 |
| 8 (`d47b735e`) | 5,865 / 182,622 | 3 / 5 vs 7 | 31 / 124 | 292 / 5,930 | 1,377 / 6,271 |
| 9 (`5503e80e`) | 5,877 / 182,643 | 12 / 21 vs 8 | 43 / 145 | 292 / 5,930 | 1,365 / 6,250 |

Against batch 5, batch 7 swapped one observed assertion for another as navigation-002 and navigation-003 swapped runner/harness timeouts; crossdoc-001 changed from OK to harness timeout. Batch 8 reversed all three changes. Both archived matched repeats on each of heads 5 and 7 produced identical keyed outcomes: crossdoc-001 OK and both navigation tests harness-timeout. Batch 9 navigation-002 moved from runner to harness timeout, observing one additional nonpassing `Navigate between URLs` assertion. The 21 newly passing assertions are distinct from this change. Observed names: 194,672 for 5/7/8 and 194,673 for 9.

All four 41-URL supplements match batch 4's complete keyed outcomes: 27 pages / 577 passes / 874 observed names, 41 OK harnesses. Chrome alone passes 8 more pages and 42 more names. The 39 withheld URLs are disjoint from all inspected plans and results; no test was executed for this review.

## Elapsed time and host load

| Batch | Combined elapsed (s) | Host samples | Whole-host CPU busy %, min / median / mean / max |
|---|---:|---:|---:|
| 5 | 978.9 | 31 | 1.32 / 9.13 / 9.68 / 25.66 |
| 7 | 985.5 | 31 | 1.32 / 6.58 / 10.12 / 60.77 |
| 8 | 990.2 | 33 | 1.57 / 17.32 / 22.88 / 60.77 |
| 9 | 208.7 | 7 | 19.02 / 43.80 / 37.45 / 55.06 |

Elapsed spans the earliest main/supplement runner log creation to the last completed report modification. Batch 9's 16 main plans contained 444–514 URLs each; runner times ranged from 58.426 to 75.363 s (median 65.023 s). Those times overlap and must not be summed as elapsed. CPU busy is 100 minus the recorded idle percentage, sampled about every 30 s; the figures describe the whole machine, not exclusive runner utilization. Some batch windows overlap. Each `timing.json` retains per-report timestamps and wall milliseconds, plus in-window host samples as `[epoch, log name, [1-, 5-, 15-minute load averages], busy percent]`.

## Receipts and reproduction

`python3 -B archive_cumulative.py "$LP_CACHE/studio-r7"` reads existing files only from the cache. It uses `collect` from the byte-checked frozen verifier, checks binary hashes and source commit IDs from the original receipts, and reconstructs the exact sets against the archived frozen, Chrome, batch 1, and previous listed outcomes. Each batch directory contains original source and binary receipts, raw report digests, runner logs, and checksummed compact outcomes and directional differences. Batch 9 also retains the exact 16 supplied plan files (gzip); every plan matches its individual report and their disjoint union equals the frozen main allowlist. Batch 7 includes the original four navigation repeat reports and their shared plan. Verify an archive from inside its batch directory with `shasum -a 256 -c SHA256SUMS`.
