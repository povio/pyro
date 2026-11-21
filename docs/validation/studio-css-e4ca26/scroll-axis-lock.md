# Scroll axis locking

Studio, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, stable Chrome `158.0.8127.37`, LP `0.0.17`, JSC `0e0dd7038a7a-6f4649e395e0`. The parent and candidate ran the same 239 URLs, six workers: the main cohort's overflow and overscroll-behavior directories plus XML initial-value enumeration. The stable reference uses the same URLs. Outcomes include missing names and harness statuses.

`3db4c45b` adds the non-inherited discrete CSS property to Stylo. With `none`, the UA must not lock the scroll to an axis; with `auto`, locking is optional. LP already consumes wheel delta on each axis independently. No input behavior or test expectations changed.

| Build | Strict pages | Named passes |
|---|---:|---:|
| Parent `a69223b2` | 116 | 1,361 |
| Candidate `3db4c45b` | 120 | 1,420 |
| Chrome stable | 216 | 1,696 |

Four new strict pages: valid parsing, computed value, inheritance and discrete interpolation. The 59 new passes comprise 6, 8, 2 and 42 names on those pages, plus one new XML enumeration assertion. Zero LP-only page or name losses. Candidate-exclusive versus Chrome: 3 pages / 52 names; Chrome-exclusive: 99 / 328. Touch, viewport and subframe input failures remain in the results; no interaction-test gain is claimed. This focused set does not replace the 8,082 main, 41 supplemental and 39 withheld accounting.
