# Preserve cubic-bezier timing controls across serialization

Parent `ef69dd5a`, same WPT revision, Chrome stable and JSC as `combined-a414.md`. Rounding keyframe control points to six decimal places changed the curve when the canonical easing was reparsed. The native regression round-trips effect timing and compares sampled progress. Browser integration: 485 passed, four failed, four ignored; the unchanged parent passed 484 and failed the same four (computed style shape, two own-name counts, accessor slot capacity).

| Matched main cohort (8,082 URLs) | Strict pages | Passing names | Observed names |
|---|---:|---:|---:|
| Parent | 5,462 | 176,925 | 194,540 |
| Candidate | 5,462 | 176,952 | 194,540 |
| Chrome stable | 6,950 | 182,963 | 194,912 |

Candidate/parent exclusive: 1/1 pages, 28/1 names. Candidate/Chrome exclusive: 254/1,742 pages, 5,649/11,660 names. The 43-page focus gains 22 names with no loss. All main URLs accounted for; LP has 11 timeouts and one hang, Chrome 10 timeouts and one hang. Separate 41-URL supplement unchanged: LP 27 pages and 577 names, Chrome 35 and 619, both 874 observed. The 39 excluded URLs remain unrun. Sibling `difference-bezierprec-*` and `outcomes-bezierprec-*` files preserve exact comparisons and candidate outcomes.

Three matched six-job repeats including both changed pages: the offscreen transition passed 2/3 on parent, 3/3 on candidate (timing-sensitive). The lost min-height composition assertion at -0.3 passed 3/3 on parent and 0/3 on candidate: the expected `size + 70px` instead serializes as `(1 * size) + 70px`. This is a reproducible calc-size arithmetic loss, not timing noise. Neither result is subtracted from the full differences.
