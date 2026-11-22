# Exiting an empty list item with insertParagraph

Across the eight insertParagraph shards, the joined census at c3372ad21 recorded 176 occurrences that passed signed Chrome and failed Pyro. The exact parent 3bef762b07f83ec0511cd2b3a5dbfcdbef17c6f0 reproduced 176 signed-only occurrences. A recurring cause was paragraph insertion on an empty list item, including after replacement of its selected text. The archived preceding editing binary lacked the integrated Web Audio change, so this batch built one exact parent and reused it throughout.

Enter on an empty item exits one list level. At the top level the default paragraph separator receives a BR and the caret. In a nested list the new paragraph is a list item at the outer level. Where items remain on both sides, the split preserves the original list and ID on the left and moves the following items to the right list. Nonempty items and a list that is itself the editing host retain their prior behavior. The correction distinguishes indentation text that collapses between list items from a surviving run of content.

| Pinned WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603 | Exact parent | First 77e97d909 | Corrected 751267b4f | Signed Chrome 158 |
|---|---:|---:|---:|---:|
| Named passes across eight shards | 6,924/7,195 | 6,949/7,195 | 6,949/7,195 | 7,021/7,195 |

All 25 matched gains pass Chrome, all 79 Pyro-only passes held, and no names were added or lost. The first and corrected candidates agree on every named status. All 25 gains matched historical URL, name and occurrence triples. The other 151 signed-only failures on these shards remain. The WPT parent and both candidate arms used the frozen builds identified in the adjacent receipts. The signed arm was reused from the void-paragraph command archive.

The first candidate matched all 48 ordinary fixtures on markup, both endpoints, unique ID count and original list/item/neighbor connectivity. Tests covered OL and UL, sole/first/middle/last items, full text replacement, a BR placeholder, ordinary nonempty text and both default paragraph separators. Source indentation between items exposed empty list shells in six of eight additional fixtures. The corrected candidate matches all eight on those same fields. The 32 supplemental controls retain 26/32 markup matches, 30/32 caret node matches and 32/32 caret offset matches. All six remaining markup differences involve a selection explicitly inside the BR node. The 72-name void paragraph guard retained every status, including the two existing Pyro-only cases. The 54 static editing pages retained 13,060 statuses and the eight child-line pages retained 300.

After undo, markup agrees with Chrome in 40/48 ordinary, 6/8 indented and 32/32 nested cases on both parent and final. Pyro restores markup by reconstructing nodes. Original-node connectivity after undo differs in all 88 fixtures, as on the parent. All measured post-edit fields that matched on the parent still match.

Independent QA confirmed all 25 signed gains and all 88 direct traces on the corrected binary. The native paragraph/select subtree regression passed.
