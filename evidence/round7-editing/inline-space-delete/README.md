# Space deletion at an inline formatting boundary

The joined census on c3372ad21 identified 47 signed-only occurrences on the paired chrome-compat delete and forwarddelete pages. All 47 matched the exact 56eca18f3 baseline by URL, name and occurrence. The previous editing executable lacked a Web Audio source change, so the parent was built once in a private serial target. Its source, tree, JSC and executable digest are recorded alongside both candidate build receipts.

When a whitespace character is deleted from a visible run spanning inline formatting nodes, the editor rebalances the portion at the downstream deletion position. If the removed character was last in its node, the following text supplies that position even though the caret can remain at the preceding text end. Whitespace-preserving text retains its own style. Contiguous sibling text nodes still use their existing deletion path.

| WPT page | Exact parent | First candidate | Corrected candidate | Signed Chrome 158 |
|---|---:|---:|---:|---:|
| Backward deletion | 570/584 | 569/584 | 582/584 | 584/584 |
| Forward deletion | 569/602 | 598/602 | 601/602 | 602/602 |

The first candidate 75b3a0ce8 gained 39 signed passes and lost 11 parent passes, ten backward and one forward. The corrected source eaf1cd20d05a1e2d753327fa9f92e750bce95784 recovered all eleven individually, kept all 39 gains and gained five more. There were no new or missing names. The revision-history ledger includes parent, first candidate, corrected candidate and signed outcome for each occurrence. The signed results were reused from the pinned WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Both target pages describe observed browser compatibility behavior.

Three signed-only cases remain. One is backward deletion across adjacent bare text nodes. The other two delete the only space inside a preformatted span; Pyro retains the empty span and Chrome removes it. The 52 other static guard pages retained all 11,874 outcomes, the eight child-line pages retained all 300, and the recent collapsible-deletion and image-insertion cohorts retained all 118. Exact parent and first head ran separately; the corrected head reused their immutable comparisons.

In the 24 controls beginning with a fully visible run across formatting elements, markup agreement improved from 14/24 to 24/24, and caret node/offset agreement from 18/24 to 24/24. In the 24 controls with adjoining ASCII spaces or pre-wrap, markup improved from 6/24 to 11/24 and caret agreement from 16/24 to 19/24. All 48 retained the original text nodes during editing as observed in Chrome. Undo returned the same result and restored the same markup in all 48. Pyro reconstructs the text nodes on undo, so the post-undo connectivity and caret references to the original objects differ. No parent-matching measured post-edit field was lost.

Independent QA confirmed 1,183/1,186 on the corrected candidate and verified every first-trial gain and loss individually. No direct field lost a parent or first-head match. The native regression for deletion next to locked blocks passed.
