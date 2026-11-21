# Percentage interpolation and CSS math: integrated checkpoint

The immutable render-enabled Studio binary from `48d7e92e` (SHA-256 `b511ddccd825a72695f52fdb3c272f9b0290b8953fa1da32f89f0d40eca8b512`) combines `01b28425` with the CSS-values work from `056e6e663`. Against the last full `twobases` baseline it gained four strict pages and 43 named assertions, with no losses or changes in observed names. All 27 names in the other worker’s `evidence.json` are in the full gain set. Fourteen further height interpolation names also passed independently on the pre-integration candidate. The two remaining gains are negative time values on the Typed OM animation-duration and transition-duration pages.

| Cohort | Parent strict / named | Combined strict / named | Chrome strict / named |
|---|---:|---:|---:|
| Main (8,082) | 5,794 / 182,066 | 5,798 / 182,109 | 6,950 / 182,963 |
| Supplement (41) | 27 / 577 | 27 / 577 | 35 / 619 |

Main: Chrome-only 1,439 pages / 6,760 names; LP-only 287 / 5,906. LP timed out on 11 URLs and hung on one; Chrome timed out on ten and hung on one. Supplement: eight Chrome-only pages / 42 names. The 39 withheld URLs remain unscored. Exact directional differences for main, supplement and the 444-URL focus subset accompany this note.

Chrome stable `158.0.8127.37`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, LP `0.0.17`, JSC `0e0dd7038a7a-6f4649e395e0`. Six workers, wptserve and testdriver, worker globals only on the supplement. The combined build took 35 s; focused WPT 1.6 s; full main 345.0 s; supplement 0.3 s. The native computed-style suite passed 19 tests with one ignored after the test-only import correction `e6e2aa53`. From the last full evidence commit to comparison, roughly 15 minutes elapsed on Studio (~172 named gains and 16 strict page gains per elapsed hour; excludes the other worker’s parallel development time).
