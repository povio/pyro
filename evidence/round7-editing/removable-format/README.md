# Removing inline formatting and restoring the visible selection

Tested source **68d89ef246b115b373962279ec61e4c26371cddd** against exact integrated parent **869d44e0a482e9947a42fd2936a7d0141cae2b4a**. The frozen parent executable SHA-256 is 54eb4bfe9b2e4c20fe06c273530908bcffee15da0b37496a48601f6aa975b2dd; the final executable SHA-256 is 0eb98eabed6789b125fa9ab7fd8408bd5c341de78ab67cb29eac31713f26f76d. The adjacent receipts record source trees, pinned JSC archive, compiler, profile and binary hashes for each tested revision.

removeFormat clears supported HTML inline formatting elements while retaining DEL annotations. Splitting a wrapper keeps its original surviving suffix and avoids duplicate IDs. Removing strike formatting can preserve unrelated attributes on the selected fragment. Foreign elements with the same local names keep their own namespace semantics. Formatting may wrap selected SVG text/tspan content in the rendered text tree. If the new HTML wrapper makes that text invisible, endpoint restoration follows visible editable text and retains the incoming selection direction. Toggling decoration on text with no rendered SVG position sets the pending state without a DOM edit or undo entry.

## Matched WPT

Independent QA confirmed **43 gains and zero losses** over 43,408 shared names. The 44 formatting pages rose from 29,621 to 29,664/33,316. removeformat.html gained 27 and strikethrough.html gained 16. All 10,092 statuses on the ten multitest pages were identical, with 8,992 passing. Of the 43 gains, 40 pass signed Chrome 158.0.8127.37. Chrome fails three pinned assertions: one removeFormat innerHTML assertion for a split span with an ID, and two mixed S/DEL queryCommandIndeterm assertions. All 929 existing Pyro-only passes remain, including both pinned MARK cases. The formatting union also contains 36 Chrome-only names excluded from the matched denominator.

Ledger columns are URL, name, occurrence, parent, first (0c6c998e8), second (dbb3012c9), third (09336b35e), fourth (b7f78198a), final and Chrome. Revision 442a47128 failed to compile before producing an artifact; dbb3012c9 supplies the measured second revision. Both Pyro arms used the same single-worker testdriver profile. The WPT reference was reused from the signed archive; head-only repeats used frozen binaries. Native guards passed for namespace/prefix identity, DEL and split-ID behavior, SVG caret/direction, ordinary formatting, and history.

## HTML identity and namespace controls

Exact post-edit markup improved from 63/112 on the parent to 108/112 on the final HTML matrix. All 28 prefix identity observations match Chrome. There are no duplicate IDs on the final candidate. The four remaining markup differences involve removing MARK, which Chrome retains. On an attributed prefix MARK, parent and Chrome each had one ID after the edit. Pyro's parent had transferred it to a clone. The final candidate keeps the actual original right-hand MARK but removes its ID when clearing the leading fragment. This is one parent-relative ID-count discrepancy, even though wrapper identity improved. The separate 64 foreign-namespace probes match Chrome for post-edit markup, node retention, ID counts, endpoints, strike state and undo eligibility. Selection.toString() matches 32/64: Pyro continues to expose text selected in invisible SVG nodes. All 176 HTML and foreign fixtures restored their starting markup on undo.

## Rendered SVG and direction attribution

Independent QA compared 16 rendered SVG text/tspan cases spanning partial, prefix, suffix and full selections in both directions. The separate owner 40-case matrix also covers foreignObject and ineligible SVG descendants. On the final candidate all 16 independent cases match signed Chrome on serialized DOM, original element/text and neighboring-node retention, ordered range endpoints, anchor/focus, direction, selected string, strike state and undo eligibility. Ten ordinary explicit-collapse/forward/backward/retarget observations passed on both candidate and reference.

| Stage | Rendered text after formatting | Direction and state |
|---|---|---|
| Exact parent 869d44e0a | Created the wrapper but selected its now-invisible text. | Preserved forward/backward, but reported an active strike and nonempty string. |
| dbb3012c9 | Suppressed the wrapper in four initial rendered text/tspan probes. | Avoided mutation and history on those cases. |
| 09336b35e | Restored wrapping. Kept a range inside the wrapper, as the parent did. | Reported strike true although Chrome reported false. Unrendered pending state was corrected separately. |
| b7f78198a | Matched Chrome's collapsed visible endpoints in all 16 cases. | Reported direction none in all 16. This lost 16 parent-matching direction observations. |
| Final 68d89ef24 | Retains the corrected visible caret. | Matches all 16 original directions (eight forward, eight backward) while anchor and focus remain equal. Ordinary explicit collapse still resets direction. |

For partial and suffix selections Chrome places the caret at the end of the preceding visible text. Prefix selection lands at the start of the following SVG text. Formatting the whole run lands at the next visible HTML text. Per-case original, intermediate, final and Chrome endpoint/direction observations are in svg-directions-per-case.json. The owner replayed only the missing original-parent arm of the independent harness. The final and Chrome entries are independent QA measurements.

**Post-undo endpoints remain a separate discrepancy.** Markup agrees in all 16 cases. Both parent and final put anchor and focus at offset zero in the following HTML text node (host child 2), with direction none. Chrome restores the original SVG selection, including its direction. Thus post-undo anchor/focus agreement is 0/16 on both parent and final. The post-edit direction recovery does not change that history behavior.
