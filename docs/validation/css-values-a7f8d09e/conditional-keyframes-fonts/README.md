# CSS Values follow-up

The focused run at `f224e5521` compared 2,888 identical subtest names across 164 pinned WPT pages. The selection covers the changed CSS Values families, custom-property cycles and fallbacks, style queries, and computed font, transition and animation properties.

| Source | Named passes | Fully passing pages |
| --- | ---: | ---: |
| Assigned base `a7f8d09e2` | 2,323 | 99 |
| Previous topic `056e6e663` | 2,350 | 102 |
| Revised topic `f224e5521` | 2,631 | 121 |

Since the previous topic, 282 subtests gained a pass and one lost a pass. Of the 282 gains, 272 exactly match Chrome-only names in the original inventory. Across both rounds the topic recovered 299 distinct names from that inventory. Every page retained its harness completion status and every previously passing page still passed. Combined-corpus validation belongs to Studio.

| Recovered assertions by affected behavior | Gains |
| --- | ---: |
| Inline if(), style queries, cycles, random values inside selected branches, and attribute-derived conditional results | 253 |
| Cached keyframes that depend on sibling position | 19 |
| Parent font metrics in font-affecting declarations | 6 |
| Typed attribute units, fallback and substitution | 4 |

The keyframe cache records which sibling index or count a computed frame and its underlying style actually used. It reuses Stylo's traversal cache to refresh only affected results. Native coverage includes an implicit endpoint after sibling removal. Inline conditionals share a short-circuit predicate walk between dependency discovery and substitution. Attribute-derived input remains marked through comparisons, failed conditions, fallbacks and interpolated registered values. Font-affecting declarations and animated values compute local font units against parent metrics.

## Pinned expectation

The sole lost pass is `css/css-values/if-cycle.html`, subtest `CSS Values and Units Test: CSS if() function cycles 4`. The pinned test expects an empty custom-property value. The candidate produces the specified fallback token `value`. The current CSS Values 5 algorithm makes a cyclic property query false, so the else branch is selected. The native test covers the direct self-query as well as Boolean composition and selected-value cycles. [CSS Values 5 inline conditionals](https://drafts.csswg.org/css-values-5/#if-notation)

## Evidence and cost

The compressed original, parent and candidate reports contain all URLs, names, statuses and WPT configuration. The differential lists each new pass, lost pass and page result, plus executable hashes for the paired runs. WPT was pinned at `a421f2fea5eb7f91e0db399e44b0fb0779ee1603` and JSC at cache key `0e0dd7038a7a-6f4649e395e0`. The existing warm target was isolated to this topic. Cargo used two jobs and the WPT runner used one browser worker and free ports.

The first sibling-cache run gained 19 with zero losses. The first combined implementation exposed eight conditional provenance regressions. The final 164-page comparison retained all eight previous passes. Parent and candidate WPT execution took 7.094 and 4.688 seconds respectively, excluding server startup. There were no browser restarts or watchdog kills. Six incremental executable build commands took approximately 41, 110, 132, 121, 56 and 50 seconds wall time, including any Cargo lock waits. Implementation, independent review, guard iterations and packaging overlapped. Elapsed time from focused inventory setup through validation was about 19 minutes.

On the final source, all 13 sampled-animation renderer tests, nine browser CSS declaration tests, and two native CSS math/parser tests passed. Both modified vendor patches were verified by forward and reverse application.
