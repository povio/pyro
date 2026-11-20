# Validation of combined topic fadcbb56e

Across the 2,859 matched non-CSS primary URLs, LP fadcbb56e gained **4,142 named passes** and lost **3** against its exact batch base aa0268b47. Fully passing pages moved from 1,342 to 1,493. The combined topic includes spatial audio, editable paragraph joins, frame URL synchronization for subresource requests, and Fetch MIME packaging.

| Guard | URLs | Parent → combined named passes | Gained | Lost | Parent → combined fully passing |
|---|---:|---:|---:|---:|---:|
| Primary non-CSS | 2,859 | 180,383 → 184,522 | 4,142 | 3 | 1,342 → 1,493 |
| CSS | 1,050 | 33,321 → 33,321 | 0 | 0 | 819 → 819 |
| CSS workers / UTF-16 | 41 | 577 → 577 | 0 | 0 | 27 → 27 |
| Historical-loss pages | 159 | 60,289 → 60,261 | 81 | 109 | 38 → 42 |

The non-CSS primary guard comprises three disjoint selections of 2,407, 38 and 414 URLs. The historical sample and primary guard overlap on 71 URLs. Separate samples of an overlapping URL remain separate in the export. All six comparisons above use receipts from the exact aa0268b47 binary.

## Named changes in the primary guard

| WPT area | Names gained | Names lost |
|---|---:|---:|
| editing | 1,584 | 0 |
| mimesniff | 1,546 | 0 |
| webaudio | 940 | 3 |
| referrer-policy | 72 | 0 |

All three lost primary pass names came from Web Audio and were absent from the new inventory: each old name includes a missing-constructor error string. The included occurrence file preserves every changed name, including names that appear or disappear. A single matched sample records the observed change. It does not, on its own, establish the underlying cause.

## Historical Chrome comparisons

| Guard | LP named / full pages | Local stable named / full pages | Public stable named / full pages | Public canary named / full pages | LP-only / local-only names | LP-only / public stable-only names |
|---|---:|---:|---:|---:|---:|---:|
| Primary non-CSS | 184,522 / 1,493 | 191,862 / 2,305 | 191,809 / 2,319 | 193,114 / 2,392 | 5,005 / 12,345 | 5,047 / 12,334 |
| CSS | 33,321 / 819 | unavailable | 34,492 / 933 | 34,759 / 948 | unavailable | 795 / 1,966 |
| CSS workers / UTF-16 | 577 / 27 | unavailable | 619 / 35 | 619 / 35 | unavailable | 0 / 42 |
| Historical-loss pages | 60,261 / 42 | 60,713 / 78 | 60,886 / 80 | 60,896 / 83 | 1,265 / 1,717 | 1,139 / 1,764 |

**Chrome stable 158.0.8127.37 is the cross-machine acceptance target. No matched 158 receipt is available in this readback**, so this report does not evaluate that acceptance criterion. URLs, source blobs, environment variants, original subtest names and occurrence numbers are retained for a separate 158 join.

The historical local stable column is the retained exact-pin macOS Chrome 157.0.8103.80 WPT-profile reference. The non-CSS local ledger contains no CSS results. Linux public stable 157.0.8103.38 (run 5478394868006912) used 1eecaa0c30, ten commits before the pin; source identity matches 4038 of 4038 selected URLs. Equal source files do not ensure identical support resources. Public canary 159.0.8143.0 (run 4907012216061952) used the exact WPT revision.

## Historical-loss disposition

The canonical 292 keys come from e4ca26f48 versus 4c13e694a on 159 URLs. Of these, 157 are referrer-policy names across 64 URLs. Each row below counts named occurrences, with a missing name distinct from an unobserved page. The dedicated historical 35df and full-census 902 observations are retained for provenance; the matched parent for this topic is aa0268.

| Fixed keys | Observation | Pass | Fail | Other | Missing name | Missing URL |
|---|---|---:|---:|---:|---:|---:|
| All 292 | 902fe811f | 131 | 149 | 0 | 12 | 0 |
| All 292 | 35dfb7474 | 154 | 127 | 0 | 11 | 0 |
| All 292 | aa0268b47 | 148 | 133 | 0 | 11 | 0 |
| All 292 | fadcbb56e | 156 | 124 | 0 | 12 | 0 |
| All 292 | Local stable | 191 | 91 | 0 | 10 | 0 |
| All 292 | Public stable | 192 | 90 | 0 | 10 | 0 |
| All 292 | Public canary | 193 | 89 | 0 | 10 | 0 |
| Referrer 157 | 902fe811f | 105 | 52 | 0 | 0 | 0 |
| Referrer 157 | 35dfb7474 | 127 | 30 | 0 | 0 | 0 |
| Referrer 157 | aa0268b47 | 123 | 34 | 0 | 0 | 0 |
| Referrer 157 | fadcbb56e | 130 | 27 | 0 | 0 | 0 |
| Referrer 157 | Local stable | 157 | 0 | 0 | 0 | 0 |
| Referrer 157 | Public stable | 157 | 0 | 0 | 0 | 0 |
| Referrer 157 | Public canary | 157 | 0 | 0 | 0 | 0 |

Relative to the parent, the fixed 292 keys gained 25 and lost 17 passes. Within the fixed 157 referrer keys the counts were 23 gained and 16 lost. Across the entire 159 pages, 35 Fetch pass names were present only in the parent sample on window-location.https.sub.html, which timed out on both binaries. The two partial name sets are preserved. The totals for the 159 entire pages also include other assertions. The 64 historical referrer URLs are disjoint from the separately validated 354-URL parser-time referrer guard.

## Focused runtime controls

The 354-URL parser-time referrer cohort is fully contained in the primary selection. It changed from 282/354 to 354/354 fully passing pages, with 72 named gains and 0 losses. These URLs have zero overlap with the 64 URLs behind the historical 157 referrer occurrences.

On the combined binary, the same-origin header probe observed the expected referrer in 24/24 top-document cases and 24/24 child-frame cases. Each matrix covers fetch, XHR, images, scripts, stylesheets and iframes at URL lengths 64, 4095, 4096 and 4097. The cumulative audio probe returned the same channel samples and curve-setter outcome as its previously reviewed LP feature binary. Nine of ten stereo channel pairs matched local Chrome within 0.00000014; the source coincident with the listener under a zero-width cone remains different. HRTF was outside this equal-power validation. [Reviewed audio control and Chrome observations](https://github.com/povio/pyro/tree/a9c7cbe49452871ddabe765ba22275b53fecf135/evidence/round6-spatial-audio)

focused-controls.json and the three compressed probe receipts retain the case counts and input hashes. The targeted header checks do not establish the cause of each historical referrer outcome.

## Coverage and provenance

This batch contains 4,109 cohort observations across 4,038 distinct official WPT URLs. Of the 31,827 non-CSS manifest URLs, 2,947 were observed here, 27,566 previously eligible URLs were outside this batch, and 1,314 retain their earlier withholding reason. The latest full-corpus evidence is tied to the separate frozen 902fe811f binary. These guard results cannot replace that full census. [Frozen census](https://github.com/povio/pyro/tree/fdf757052183b8df60dae474f478d20de16aa436/evidence/round6-noncss-902fe811f) · [Parent merge evidence](https://github.com/povio/pyro/tree/20fec3cd80197c06fa44a7a44a5ef98a707ef183/evidence/round6-noncss-merged-aa0268)

All four non-CSS receipt pairs used four runner jobs; both CSS guard pairs used two. WPT was a421f2fea5eb7f91e0db399e44b0fb0779ee1603 on macOS arm64 with JSC 0e0dd7038a7a-6f4649e395e0. provenance.json contains all executable and receipt hashes, job counts, restart/watchdog telemetry, HTTP/2 and manifest modes. Exact expected outcomes in the source test suite were untouched.

Run `python3 verify.py` in the distribution to check the checksums, strict-page totals, full occurrence alignment, canonical WPT port mappings, both historical cohorts and omitted inventory.

- pages.jsonl.gz rows contain cohort, URL, WPT source, git blob, equal-source flag for public stable, and one record per column. Each record contains outer status, harness status, strict pass, named count, pass count, and the producing server configuration SHA for local runs. Column order is in provenance.json. variant-inventory.jsonl.gz adds the observed execution environment, scheme, source and source blob for every URL in each cohort.
- names.jsonl.gz stores the full union of named keys (URL, canonical name and one-based occurrence), with each original name and status. A null entry means absence. prior-changes.jsonl.gz contains all status changes versus aa0268 for that guard. cohorts.json stores matched and missing counts for every comparator.
- historical-292.json and historical-referrer-157.json preserve original classifications and each individual occurrence. noncss-coverage.jsonl.gz holds all official non-CSS URL variants and their measurement disposition. local-stable-provenance.jsonl.gz and local-stable-receipts.json identify the original same-pin local Chrome receipt and server config.
