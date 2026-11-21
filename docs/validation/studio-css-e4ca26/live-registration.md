# Live custom-property registration

Parent `df1f6646`; WPT, Chrome stable, JSC and six-job inventories as in `combined-a414.md`. A successful script registration must invalidate the style snapshot, not just append a descriptor. Native tests cover live computed style and non-inheritance. With the registered syntax actually applied, the transform property parser incorrectly accepted `none` as a function, causing a union to interpolate instead of switching discretely. Reject empty transforms and enforce one function for an individual component, retaining space-separated lists. Both transform-union midpoint assertions now match stable. Native layout-text tests: 12/12. Stylo vendor patch passed forward/reverse verification.

| Cohort | Engine | Strict pages | Passing names | Observed names |
|---|---|---:|---:|---:|
| Main (8,082 URLs) | Parent | 5,463 | 176,962 | 194,540 |
| | Candidate | 5,472 | 177,172 | 194,540 |
| | Chrome stable | 6,950 | 182,963 | 194,912 |
| Supplemental (41 URLs) | Parent and candidate | 27 | 577 | 874 |
| | Chrome stable | 35 | 619 | 874 |

Main gained 9 pages and 210 names with no losses, including 57 names on registered-property computation and 27 on registered initial values. Candidate/Chrome exclusive: 255/1,733 pages and 5,655/11,446 names. All URLs accounted for; main LP 11 timeouts and one hang, Chrome 10 timeouts and one hang. The supplement has no timeouts or hangs; 39 additional exclusions remain unrun. Sibling `regtyped` artifacts retain the exact sets.

On 227 cross-feature control URLs, the full run gained 8 pages and 208 names without loss. In three additional matched repeats, page gains/losses were 9/0, 8/1, 8/0; named gains/losses 210/0, 208/1, 208/0. The sole repeat loss was the timing-sensitive offscreen transition start; it passed on both in the full run. The registration-only intermediate had one repeatable transform-union loss; the parser correction recovered it before publication.
