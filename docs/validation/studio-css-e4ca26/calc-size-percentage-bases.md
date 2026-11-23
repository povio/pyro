# Percentage bases in calc-size()

Parent `c66c9e42`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`, local Chrome stable 158.0.8127.37. Six-job main and supplemental runs, same inventories as `computed-longhands-and-mask.md`.

| Main (8,082 URLs) | Strict pages | Passing names | Observed names |
|---|---:|---:|---:|
| Parent | 5,460 | 176,595 | 194,538 |
| Candidate | 5,460 | 176,773 | 194,540 |
| Chrome | 6,950 | 182,963 | 194,912 |

Candidate/parent exclusive: 0/0 pages, 179/1 passes, 2/0 observed. Candidate/Chrome: 253/1,743 pages, 5,647/11,837 passes, 309/681 observed. Harness statuses 0/1/2: 7,867/98/105, 11 timeouts, one hang, zero missing. The sole lost assertion was transition start time at generation; it passed all three six-job repeats in both builds and remains in the full difference. The two previously lost mask assertions remain tracked.

On 281 sizing URLs: 164/8,922 → 164/9,099, Chrome 257/10,452 (strict pages/passing names). All 177 gains retained on the standalone focused run; no lost names. Height interpolation 750 → 914, width 576 → 588, one static height assertion gained; two further gains on fixed table layout in main. The separate 41 supplemental URLs stayed at 27/572 versus 35/619 stable (874 observed); 39 security/crash exclusions remain unrun. Render integration: 1,418 passed, three ignored, including different horizontal/vertical percentage bases in a nested expression. Both vendor patches round-trip.

Percentage-containing numeric bases now prepare against a distinct 100% basis, not the wildcard `any` or an intrinsic basis. Authored percentages alongside `size` in the calculation, indefinite-basis handling in all layout modes, and mixed wildcard calculations with percentages are not generally implemented.
