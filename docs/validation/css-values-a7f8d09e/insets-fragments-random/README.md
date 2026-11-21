# CSS Values follow-up from 9a84bcad8

Source `3e799f754` recovered 30 of the 922 outstanding Chrome-only names. All 30 passed in two successive candidate runs. The expanded focused selection contains 201 pinned WPT pages and 3,278 unique named assertions. Every run observed the same names. Page completion statuses stayed stable.

| Build | Named passes | Fully passing pages |
| --- | ---: | ---: |
| Original assigned base, `a7f8d09e2` | 2,656 | 117 |
| Previous integrated topic, `9a84bcad8` | 2,964 | 139 |
| Candidate, `3e799f754` | 2,994 | 142 |

| Shared cause | New passes |
| --- | ---: |
| Inset transitions used unclamped endpoints, and large inset serialization differed from CSSOM | 22 |
| Computed CSS URLs lost their fragment-only spelling and could resolve against a different resource base | 5 |
| Early random() evaluations in separate conditional predicates reused an index | 2 |
| Typed OM rounded a numeric value to its CSSOM text precision | 1 |

Transitions now compare and sample the same absolute layout endpoints, including interruption and reversal. CSS keyframes retain their separate interpolation behavior. Local CSS references retain their fragment through computation and refer to the document even when its base URL is different. The renderer guard checks the clip and mask pixels and records the requests made for a relative image and local SVG references. Conditional random keys follow the order of evaluated substitutions. Native checks cover nested fallback order, registered style-query operands, subpixel Typed OM values, zoom and exact integer values above 2²⁴.

The candidate gained 30 names and lost zero against the previous integrated topic. Across the original comparison it gained 339 names and lost one, for a net gain of 338 on this selection. Of the 339 gains, 329 match the original 1,221 Chrome-only names. The previously reported loss on `if-cycle.html` subtest 4 remains in those totals. The pinned expectation is an empty property. The current CSS Values 5 draft makes the cyclic style query false and selects the else branch. [CSS Values 5](https://drafts.csswg.org/css-values-5/#if-notation)

892 original names remain. Of these, 728 belong to Studio’s intrinsic-size work. Another 63 on `random-in-custom-function.tentative.html` depend on CSS @function registration, argument binding and invocation. The 19 angle-serialization names embed engine-specific matrix text. Both engines passed those assertions on the original investigation.

## Verification and cost

The compressed reports contain every selected URL, status, test name and assertion outcome. The differential records named gains and losses, executable hashes, WPT revision and JSC cache key. The selected tests ran with one browser worker and fresh ports. Parent and final candidate WPT execution took 9.131 and 5.101 seconds respectively, excluding startup. Neither run restarted its browser or hit a watchdog. The isolated cached target used at most two Cargo jobs. Eight incremental executable build attempts consumed 305 seconds of wall time in total, including compile failures. The final executable build took 36 seconds. All three vendor patches passed forward and reverse verification.

All 14 browser CSS declaration tests and 19 renderer request-identity tests passed on the final source. Elapsed time from inventory subtraction through validation was approximately 17 minutes. Studio separately confirmed the previous 299 recovered inventory names in its full run. The additional 30 await combined validation.
