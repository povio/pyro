# Independent intrinsic and percentage references in calc-size()

`eef18a05` separates the intrinsic `size` coefficient from percentages of the containing block. Row and column flex-basis now pass strictly. Width and height interpolation each gained 20 named assertions; static width and height each gained two. Parsing, Typed OM, and out-of-flow layout each gained one. No previously passing page or assertion was lost, and no observed name changed.

| Cohort | Parent strict / named | Candidate strict / named | Chrome strict / named |
|---|---:|---:|---:|
| Main (8,082) | 5,792 / 182,015 | 5,794 / 182,066 | 6,950 / 182,963 |
| Focus (418, subset of main) | 328 / 8,015 | 330 / 8,066 | 409 / 8,981 |
| Supplement (41) | 27 / 577 | 27 / 577 | 35 / 619 |

Main Chrome-only: 1,443 pages / 6,803 names; LP-only: 287 / 5,906. LP recorded 11 timeouts and one hang, Chrome ten and one. Supplement: eight Chrome-only pages / 42 names. The 39 withheld URLs are unscored. Exact pass-set differences, observations and compact outcomes accompany this note.

Studio Chrome stable `158.0.8127.37`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, LP `0.0.17`, JSC `0e0dd7038a7a-6f4649e395e0`. Six workers with wptserve/testdriver; worker globals only for the supplement. Native computed-style suite: 18 passed, one ignored. Four vendor patches verified in both directions against `a7f8d09e2`.

Successful incremental build: 27 s; focused runner: 1.6 s; immutable candidate main: 345.0 s; supplement: 0.3 s. These measured stages total 373.9 s (491 gained assertions and 19 gained pages per validation-hour); this excludes development, native tests and setup. Whole checkpoint wall time from previous evidence commit to this comparison was about 19 minutes (~161 named gains/hour, ~6 strict pages/hour).
