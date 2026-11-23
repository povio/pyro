# Gap-rule lists and interpolation

Source `2898e522`, parent engine `436b0a12`. The pinned reference is Studio Chrome stable 158.0.8127.37, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`. Both browser runs use the same URLs. Strict requires successful completion and all named assertions passing.

| Cohort | Parent strict / named | Candidate strict / named | Chrome strict / named | Gains / losses (strict; named) |
|---|---:|---:|---:|---:|
| Main, 8,082 URLs | 5,556 / 178,661 | 5,583 / 179,866 | 6,950 / 182,963 | 27 / 0; 1,205 / 0 |
| CSS Gaps + multicol controls, 169 of main | 107 / 4,421 | 134 / 5,624 | 163 / 5,764 | 27 / 0; 1,203 / 0 |
| Supplement, 41 URLs | 27 / 577 | 27 / 577 | 35 / 619 | 0 / 0; 0 / 0 |

Main exclusive sets: Chrome 1,639 pages / 8,899 names; LP 272 / 5,802. LP observed 194,540 names, Chrome 194,912. Main timeouts: 11 LP, 10 Chrome; one hang each. The 39 withheld URLs are not counted. See `difference-gapzoom-*` for both directions and `outcomes-gapzoom-*` for candidate outcomes.

On repeater interpolation the width page gained 252 of 252 Chrome passes and the color page gained 252 of 276. The remaining 24 color misses include a 76-versus-77 channel rounding discrepancy. A timing-serialization experiment did not change any of the 169 controls and is not included. The intermediate `gaprules` run lost two names in explicit inherited width under zoom (both changed from 10px to 3.33333px). Declaring the list width zoom-dependent brought back both original names; the intermediate directional sets are retained separately.
