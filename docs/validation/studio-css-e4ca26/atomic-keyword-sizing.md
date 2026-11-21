# Keyword sizing of atomic inline and replaced boxes

Studio LP 0.0.17 with JSC `0e0dd7038a7a-6f4649e395e0`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable 158.0.8127.37. All comparisons use the same 8,082 main URLs. Strict requires a completed successful harness, named assertions and no nonpassing assertion. The 41 supplemental URLs and 39 withheld URLs are not part of these totals.

| Source | Strict pages | Named passes | Strict gained/lost from previous row | Named gained/lost |
|---|---:|---:|---:|---:|
| Integrated `52d8b7c5` | 5,552 | 178,540 | — | — |
| Inline bounds `6417d55e` | 5,552 | 178,617 | 1/1 | 78/1 |
| Replaced size transfer `65e687f9` | 5,554 | 178,645 | 2/0 | 28/0 |
| Intrinsic cross size `3f365318` | 5,555 | 178,646 | 1/0 | 1/0 |
| Chrome reference | 6,950 | 182,963 | — | — |

The inline-block keyword WPT improved from 44/74 to 74/74. The replaced-element keyword WPT improved from 74/84 to 84/84. Both match Chrome. The initial inline change lost the min-width:max-content assertion on `replaced-element-max-content-contribution-transferred-size.html`: the intrinsic probe discarded the specified height, so the ratio-only image used its default size. Carrying the definite cross size through measurement recovered the assertion and strict page. The 314-page control inventory includes that regression and the floating, replaced, positioned, flex, grid and intrinsic sizing cases.

`inlinecross` is compared directly with the integrated checkpoint, not just the preceding commit: +3/−0 strict pages and +106/−0 named passes. Stable-only: 1,667 pages and 10,119 names; LP-only: 272 pages and 5,802 names. Observed names: LP 194,540, Chrome 194,912. LP had 11 timeouts and one hang; Chrome had 10 timeouts and one hang. Supplemental outcomes unchanged: LP 27 strict / 577 named, stable 35 / 619, 874 observed each. Missing and withheld cases are not passes. Per-URL outcomes and directional set files accompany the comparison logs. No test or expectation edits.
