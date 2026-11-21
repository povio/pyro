# Resolved origins during animation

The renderer already resolves `transform-origin` and `perspective-origin` for layout boxes. CSSOM now passes the sampled style to that resolver, without changing the renderer's stored style. The 14 main-inventory URLs with either origin in their path passed 4 strict / 298 named on the parent, 12 / 512 on the candidate, and 13 / 516 on Chrome stable. All three observed the same 517 names. The four remaining Chrome-only names are SVG fill-box and viewport reference boxes.

Full 8,082-URL main cohort, pinned revisions and environment as in `system-fonts-and-transition-properties.md`:

| Build | Strict pages | Passing names | Observed names |
|---|---:|---:|---:|
| Parent (`83037cd4`, same code as `91c2bf96`) | 5,395 | 174,362 | 194,312 |
| Candidate | 5,405 | 174,581 | 194,312 |
| Chrome stable 158.0.8127.37 | 6,950 | 182,963 | 194,912 |

Candidate/parent exclusive: 10/0 pages, 220/1 passing names and 2/2 observed names. The lost `perspective-origin` name on explicit inheritance at zoom contains serialized values: parent and candidate both pass, with `50%` now resolving to `0px` on the zero-height boxes. The other renamed assertion (`transform-origin` on the same page) fails in both. Candidate/Chrome exclusive: 242/1,787 pages and 5,494/13,876 passing names. Harness status distributions are unchanged from the parent. The 41 supplemental URLs and 39 explicit exclusions are not part of these counts and were not rerun.
