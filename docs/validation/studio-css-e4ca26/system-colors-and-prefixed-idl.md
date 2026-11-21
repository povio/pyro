# System colors and vendor-prefixed declaration attributes

Studio, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, stable Chrome `158.0.8127.37`, LP `0.0.17`, JSC `0e0dd7038a7a-6f4649e395e0`. Both implementation commits were measured independently with the six-job main runner.

| 8,082 matched URLs | Strict pages | Passing names | Observed names |
|---|---:|---:|---:|
| Parent `54176175` | 5,627 | 180,611 | 194,655 |
| System colors `cd010d7e` | 5,628 | 180,638 | 194,655 |
| Prefixed IDL `2b5142fd` | 5,628 | 180,662 | 194,655 |
| Chrome stable | 6,950 | 182,963 | 194,912 |

Neither change lost a previously passing page, passing name, or observed name. The final strict sets have 1,596 Chrome-only and 274 LP-only pages; the named sets have 8,176 Chrome-only and 5,875 LP-only passes. LP recorded 11 runner timeouts and one hang, Chrome 10 and one. On the separate 41-URL supplement, all harnesses completed: LP 27 pages / 577 names, Chrome 35 / 619, both 874 observed. The remaining 39 withheld URLs receive no credit. Direct focused outcomes and comparisons, full and supplemental candidate outcomes, differences and harness statuses accompany this report.

- System colors: on the 43-URL control set, 30 pages / 1,159 names became 31 / 1,186; Chrome passed 33 / 1,193. System color consistency went from 35/62 to 62/62, matching Chrome. Renderer integration on each binary's source revision: 1,411 passed, three ignored, the same nine failures (one marker and eight mirror tests). Both vendor patches were round-trip checked.
- CSSOM: on 59 URLs, 48 pages / 1,835 names became 48 / 1,859; Chrome passed 53 / 1,858. Five declaration integration tests passed. The engine now exposes IDL spellings of supported vendor-prefixed properties; this does not add layout support for `-webkit-box-ordinal-group`.
