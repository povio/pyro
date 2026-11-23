# Resolve the content basis of calc-size() on flex items

`6856defe` preserves the flex-specific `content` keyword through parsing, nested calculation composition and layout. An explicit height or width is not the content basis. The native suite passed 17 computed-style tests (one ignored), including nested content calculations and rejection on width. Both vendor patches round-trip against `7fdc30d4`.

| Matched cohort | Parent `flextarget` (strict pages / named passes) | Candidate `flexcontent` | Studio Chrome stable |
|---|---:|---:|---:|
| Main (8,082) | 5,792 / 182,008 | 5,792 / 182,015 | 6,950 / 182,963 |
| Focus (418, subset of main) | 328 / 8,008 | 328 / 8,015 | 409 / 8,981 |
| Supplement (41) | 27 / 577 | 27 / 577 | 35 / 619 |

Three new row and three new column assertions plus one parsing assertion. No lost pages or names, no changes in observed names. The row and column pages still fail on percentages: the containing block and the intrinsic basis are distinct reference sizes. Main Chrome-only 1,445 pages / 6,854 names; LP-only 287 / 5,906. LP had 11 runner timeouts and one hang; Chrome ten and one. The 39 withheld URLs are unscored. Exact directional differences and compact outcomes are archived under `flexcontent`.

WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; Chrome stable `158.0.8127.37`; LP `0.0.17`, JSC `0e0dd7038a7a-6f4649e395e0`. Six workers, wptserve, testdriver; worker globals only on the supplement.
