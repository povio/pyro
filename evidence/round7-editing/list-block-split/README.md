# Splitting a nonempty paragraph inside a list item

Integrated parent 91d0d5228 differs from measured source 91dc1b1d4 only in 19 evidence files. This topic reuses the immutable executable and all existing parent and signed WPT arms. Candidate eb9b5aa9894869e3d8831fc3f1668be8f693bb1a was compiled once. The receipts identify the source trees, binaries and pinned JSC.

When Enter splits a direct P or DIV inside a list item, the new paragraph belongs to a sibling LI. At the start, an empty item is inserted before the original item, retaining the original paragraph, text and caret together. In the middle or at the end the split paragraph goes into a following item while the original item's other children stay in place. Empty formatted paragraphs receive their BR inside the inline formatting. The existing item and paragraph keep their IDs when a new shell is cloned.

Across eight insertParagraph shards at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603, passes increased from 6,962 to 6,989 of 7,195. Signed Chrome 158.0.8127.37 passes 7,021. Of 27 matched gains, 23 pass signed Chrome and correspond to the same 23 occurrences in the frozen c3372ad21 census. Four gains pass WPT and fail in Chrome: middle and end insertion in an OL with IDs on both LI and P, each with the two default separators. These outcomes retain separate attribution in the ledger. All 79 existing Pyro-only passes held, with no lost or missing names. There are 115 remaining signed-only occurrences on other insertParagraph cases.

In 48 direct cases, exact markup, anchor and focus agreement improved from 0/48 to 48/48 both after the split and after typing X. The fixtures cross OL/UL, P/DIV, three caret offsets, plain and italic text, sibling paragraphs and a locked sibling. Connectivity of the original LI, paragraph, text and neighboring LI matched in all 48 before and after the edit. After two undo calls, starting markup and selection matched Chrome in all 48. Pyro takes separate undo steps for typing and splitting; Chrome groups them. Original-node connectivity after undo differs in all 48 on both Pyro builds.

All 13,060 static editing, 300 child-line and 72 void-paragraph guard outcomes retained their parent status. The candidate guard runs were measured here. The native paragraph/select subtree regression passed.

Independent QA remeasured the eight WPT shards and all 48 direct cases on the frozen candidate. The per-occurrence counts and all direct observations matched the owner receipts.
