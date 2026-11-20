# Animation coefficients and registered endpoints

Parent `a4bf8f6d`; pinned Chrome stable, WPT and JSC as in `combined-a414.md`. In three matched six-job 22-URL runs, both the fit-content min-height additive extrapolation at −0.3 and the initial registered `--color` sibling-index assertion passed 0/3 before and 3/3 after. The former requires rounding the combined coefficients once; the latter preserves the endpoint's computed color space after checking interpolation compatibility. Later sibling invalidation assertions are distinct. A constructable stylesheet import-load assertion was parent-only in one focused repeat, but passed in both full runs.

| Matched cohort | Engine | Strict pages | Passing names | Observed names |
|---|---|---:|---:|---:|
| Main (8,082 URLs) | Parent | 5,462 | 176,952 | 194,540 |
| | Candidate | 5,463 | 176,962 | 194,540 |
| | Chrome stable | 6,950 | 182,963 | 194,912 |
| Supplemental (41 URLs) | Parent and candidate | 27 | 577 | 874 |
| | Chrome stable | 35 | 619 | 874 |

Main exclusive sets, candidate/parent: 1/0 strict pages and 10/0 passing names; candidate/Chrome: 254/1,741 pages and 5,649/11,650 names. The 227-URL cross-feature control subset gained the target page and two target names without loss. No missing URLs; main LP timed out 11 times and hung once, Chrome 10 times and once. Supplement has no timeouts or hangs; 39 exclusions not run. Sibling `endterms` artifacts retain the exact sets. Regenerated Stylo patch passed forward and reverse verification.
