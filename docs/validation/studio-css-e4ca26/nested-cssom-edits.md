# CSSOM edits in a nesting context

Implementation `b59bd97a`, parent `05cb366af` (`nestblock`). Studio Chrome stable 158.0.8127.37 and WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, LP 0.0.17 with JSC `0e0dd7038a7a-6f4649e395e0`. Six workers, wptserve, testdriver; worker globals on the supplement only.

Both `css/css-nesting/invalid-inner-rules.html` and `css/css-nesting/nested-declarations-matching.html` changed from one failing assertion to fully passing, matching Chrome. The native control also covers invalid selector assignments and a malformed multi-rule insertion. No expectations changed.

| Cohort | Parent pages / names | Candidate pages / names | Chrome pages / names |
|---|---:|---:|---:|
| Main (8,082) | 5,786 / 181,861 | 5,788 / 181,863 | 6,950 / 182,963 |
| Focus (158, subset of main) | 123 / 887 | 125 / 889 | 137 / 908 |
| Supplement (41) | 27 / 577 | 27 / 577 | 35 / 619 |

Main gains: two strict pages and two names; no losses in either cohort. Chrome-only 1,449 / 7,005; LP-only 287 / 5,905. Parent and candidate both had 11 timeouts and one hang; Chrome had ten timeouts and one hang. The 39 withheld URLs remain unscored. The `nestedit` outcome files and six directional comparisons preserve the exact sets.
