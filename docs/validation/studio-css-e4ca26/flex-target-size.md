# Final flex targets are not intrinsic calculation bases

`7fdc30d4` preserves the item’s main-axis target when laying out its content. For an intrinsic fit-content width measured for the flex basis, the calculation acts on the fitted width. An initial focused candidate gained five names but lost the composed row fit-content case (504px instead of 516px); the corrected candidate preserves it. The native computed-style suite passed 16 tests, with one ignored. The Blitz vendor patch round-tripped against `c1e981d89`.

| Cohort | Parent `flexbasiscalc` (strict pages / named passes) | Candidate `flextarget` | Chrome stable |
|---|---:|---:|---:|
| Main (8,082 URLs) | 5,792 / 182,003 | 5,792 / 182,008 | 6,950 / 182,963 |
| Focus (418, subset) | 328 / 8,003 | 328 / 8,008 | 409 / 8,981 |
| Supplement (41) | 27 / 577 | 27 / 577 | 35 / 619 |

Two row and three column assertions gained. No lost pages, named passes, or observed names in either scored cohort. Main Chrome-only: 1,445 pages / 6,861 names; LP-only: 287 / 5,906. LP had 11 runner timeouts and one hang; Chrome had ten and one. The 39 withheld URLs are unscored. Exact sets, outcomes and focus inventory accompany this report under `flextarget`.

WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; Studio Chrome stable `158.0.8127.37`; LP `0.0.17`, JSC `0e0dd7038a7a-6f4649e395e0`. Six workers, wptserve and testdriver; worker globals only for the supplement.
