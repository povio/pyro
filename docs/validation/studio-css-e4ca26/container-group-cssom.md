# Container rules in CSSOM

Parent `a17876be`; same WPT, Chrome stable, JSC and six-job settings as `first-line-cssom.md`. Previously an opaque rule string bypassed condition parsing and lacked a child rule list. Parse condition lists with Stylo, serialize the conditions, and store child rules in the existing CSSOM grouping model. The native parser regression passed.

On 638 CSSOM, nesting, cascade and conditional control URLs: 399 → 403 strict and 8,611 → 8,656 passing names. Chrome passed all 45 gained names. `at-container-parsing.html`: parent 85/117, candidate and stable 117/117. Additional gains cover nested serialization, malformed preludes and IDL.

| Cohort | Parent strict / named | Candidate strict / named | Chrome strict / named | Observed names LP / Chrome |
|---|---:|---:|---:|---:|
| Main (8,082) | 5,483 / 177,300 | 5,487 / 177,343 | 6,950 / 182,963 | 194,540 / 194,912 |
| Supplemental (41) | 27 / 577 | 27 / 577 | 35 / 619 | 874 / 874 |

Main: +4/-0 strict and +46/-3 named; LP-only / stable-only: 255 / 1,718 strict and 5,655 / 11,275 names. Eleven LP timeouts and one hang vs ten and one on stable. All three observed losses were on `CSSStyleSheet-constructable.html`. The resource timing check measured 19.3 ms against a 19.4 ms lower bound; its assertion precedes link removal in that callback. The two subsequent import checks observed red instead of black, consistent with the link remaining attached. The 638-page control and three more paired six-job, 32-URL runs passed all three names on both binaries. Retain the losses rather than discounting them. The supplement (4 UTF-16 and 37 workers) and 39 exclusions are unchanged. Sibling `cgroup` artifacts contain the exact differences, candidate outcomes and paired repeats.
