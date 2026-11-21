# Apply the calculation to an intrinsic flex basis

`e1d7ea59` applies the calculation once to the measured flex base size, respecting box-sizing before flooring the inner base size. Numeric bases continue to use length-percentage resolution. Readback uses unrounded physical rather than logical dimensions in vertical writing modes. Native computed-style controls: 15 passed, one ignored. All three vendor patches round-trip against the parent.

| Matched cohort | Parent (`flexused`) pages / names | Candidate | Studio Chrome stable |
|---|---:|---:|---:|
| Main (8,082) | 5,792 / 181,987 | 5,792 / 182,003 | 6,950 / 182,963 |
| Focus (418, subset) | 328 / 7,987 | 328 / 8,003 | 409 / 8,981 |
| Supplement (41) | 27 / 577 | 27 / 577 | 35 / 619 |

Eight new named passes in each of the row and column flex-basis tests. Neither becomes fully passing; unsupported `content` bases and calculations mixing percentage reference sizes remain separate gaps. No strict or named losses, status changes or changed observed names. In the main cohort 1,445 strict pages and 6,866 names are Chrome-only; 287 pages and 5,906 names are LP-only. LP had 11 runner timeouts and one hang; Chrome ten and one. The 39 withheld URLs remain unscored. Exact comparisons and outcomes are archived under `flexbasiscalc`.

WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603; Chrome stable 158.0.8127.37; LP 0.0.17, JSC 0e0dd7038a7a-6f4649e395e0. Six workers, wptserve and testdriver; worker globals only on the supplement.
