# Replaced elements under inline-size containment

WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Studio Chrome stable `158.0.8127.37`, LP `0.0.17`, JSC `0e0dd7038a7a-6f4649e395e0`. Six jobs. Counts are strict pages / unique passing (URL, subtest name) pairs. The archived `difference-*` files retain both directional sets and the observed-name differences.

| Build | 8,082 main URLs | Main gains/losses vs preceding build (pages; names) | 37 containment/ratio controls | 41 supplemental |
|---|---:|---|---:|---:|
| Parent `3009773a` | 5,497 / 177,775 | — | 11 / 784 | 27 / 577 |
| Logical inline override `ef60c020` | 5,498 / 177,834 | +1/−0; +60/−1 | 12 / 844 | 27 / 577 |
| Drop natural ratio `4af12369` | 5,499 / 177,843 | +1/−0; +9/−0 | 13 / 852 | 27 / 577 |
| Chrome stable | 6,950 / 182,963 | — | 37 / 1,029 | 35 / 619 |

Both containment pages now pass: `contain-inline-size-replaced.html` (30/30) and `contain-intrinsic-size-028.html` (48/48). Eight native renderer controls passed, including horizontal and vertical canvas geometry. The ninth named gain in the second main comparison was recovery of the first comparison’s transition start-time loss. Four interleaved 35-URL runs per build at six jobs passed that assertion 4/4, 4/4 and 3/4 respectively; the full-run outcomes remain unadjusted. `cisguard-repeats.json.gz` retains all repeats.

Main Chrome-only: 1,707 pages / 10,815 names; LP-only: 256 / 5,695. LP observed 194,540 names, Chrome 194,912. LP had 11 timeouts and one hang; Chrome had ten timeouts and one hang. Supplemental observed 874 names on each binary; Chrome-only 8 pages / 42 names, LP-only zero. The remaining 39 of 8,162 manifest URLs are withheld, not passes.
