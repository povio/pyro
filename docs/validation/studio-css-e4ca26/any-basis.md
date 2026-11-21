# Independent calc-size calculations

Main cohort at `5edeefe4`: 8,082 URLs, 5,375 strict pages and 172,302 passing names. Relative to `5c9d80c8`: identical strict pages, +181/-0 passing names (124 width, 56 height, one fixed table layout), +1/-0 observed names. Against Chrome stable 158.0.8127.37: 242/1,817 exclusive pages, 5,391/16,052 exclusive passing names and 252/854 exclusive observed names (LP/Chrome). LP recorded 101 harness errors, 105 harness timeouts, 11 runner timeouts and one hang; stable recorded 23, 62, 10 and one respectively. See the stretch-main archives for the exact sets.

The largest single Chrome-only cluster was calc-size height interpolation (752), then system-font animations (588) and calc-size width interpolation (380). `calc-size(any, …)` failed at parse time. Its calculation takes ordinary percentages rather than the intrinsic `size` placeholder. The new representation preserves this distinction in specified/computed CSS and resolves the dimension through normal length/percentage layout. Interpolation with an intrinsic basis can use an independent length; mixing a containing-block percentage with a coefficient of an intrinsic size still needs two distinct percentage bases.

Matched 1,013 sizing, positioning and alignment URLs (same WPT, JSC, Chrome and harness as `calc-size-readback.md`):

| Metric | `5edeefe4` | Any basis | Stable |
|---|---:|---:|---:|
| Strict pages | 715 | 715 | 931 |
| Passing names | 32,640 | 32,967 | 36,044 |
| Height interpolation | 404 | 564 | 1,156 |
| Width interpolation | 352 | 504 | 732 |

327 new passes, no lost passes, no change to page or observed-name sets. Other gains: five Typed OM, four parsing, four static height and two static width. Both LP builds had three harness errors and no runner timeouts; stable completed all URLs with OK harness status. The 41 supplemental URLs (LP 27 pages/436 names; stable 35/619) and 39 exclusions remain outside both main and focused totals.

## Full run of `1973f6a1`

All 8,082 URLs accounted for. LP: 5,374 strict pages, 172,629 passing names, 194,311 observed names. Stable: 6,950 / 182,963 / 194,912. LP/stable exclusive sets: 242/1,818 strict pages, 5,391/15,725 passing names, 252/853 observed names. Harness and runner status counts match the preceding full run. Against `5edeefe4`: strict +0/-1, named +328/-1, observed +1/-0. The additional pass outside the focus was a newly observed fixed-table-layout name.

The sole loss was the offscreen transition ready-time equality (5.9 vs 5.8 ms). Three matched repeats per binary, each with the same 12 neighboring URLs at six jobs, yielded two passes and one 0.1 ms mismatch on **each** binary. Keep the loss in the full-run totals; the repeats establish that the mismatch also occurs on the parent, not that the underlying timing defect is fixed. Raw repeats and exact main sets are archived alongside this note. Remaining largest stable-only pages by named count: height interpolation 592, system fonts 588, gap color repeaters 276, gap width repeaters 252, width interpolation 228.
