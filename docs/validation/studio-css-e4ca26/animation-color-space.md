# Select animation space from resolved colors

Same engine versions, WPT revision and URL cohorts as `combined-a414.md`. Previously generic animated colors always used sRGB even when an endpoint was modern. Choose the space only after both endpoints resolve, including when `currentcolor` depends on the element; do not override explicit color-mix spaces. A native paint test checks the midpoint of black and a legacy/modern white foreground. Render integration: 1,419 passed, three ignored. Stylo vendor patch round-trip verified.

| Main, 8,082 URLs | Strict pages | Passing names | Observed names |
|---|---:|---:|---:|
| Combined base | 5,459 | 176,777 | 194,540 |
| Candidate | 5,462 | 176,925 | 194,540 |
| Chrome stable | 6,950 | 182,963 | 194,912 |

Candidate/base exclusive: 3/0 pages, 149/1 names. Box shadows and SVG fill each gain 72 names. Five more gains come from animations of relative colors, color-mix and OKLCH. Against Chrome: 254/1,742 pages and 5,649/11,687 names exclusive. Candidate: 11 timeouts, one hang, zero missing. Separate 41 supplemental URLs unchanged at 27/577, Chrome 35/619, both 874 observed. The 39 security/crash URLs are still excluded.

The one lost name is `Initially, the sibling-index() is 3 for --color`: at zero progress LP emits an equivalent Oklab color instead of the specified sRGB endpoint of a registered custom property. Reproduced in 3/3 six-job repeats on candidate, 0/3 on combined: **not timing noise**. The older offscreen transition loss is intermittent: passes in 0/3 prior CSS, 3/3 combined, 1/3 candidate repeats. The full differences are retained, not adjusted using the repeats.
