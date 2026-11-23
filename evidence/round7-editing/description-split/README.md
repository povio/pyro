# Splitting a child paragraph in a description list

Integrated parent c10471bc7 differs from measured source eb9b5aa98 only in 13 evidence files. The parent executable and the eight-shard WPT results were reused from the ordinary list split batch. Candidate source is 531432aa9bd1aadc1d6f460574473722d15a234d. The build receipts retain source trees, binary digests and pinned JSC.

Enter inside a nonempty P or DIV directly under a term or description splits the containing item when its parent is an HTML DL. Both halves retain the original item type. At the start of the paragraph the new empty item precedes the original; at the middle and end only the split paragraph moves to the following item. Other children of the original item stay with it. This extends the measured OL/UL behavior through the same editing path.

Across the eight insertParagraph shards at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603, the candidate passed 7,005/7,195 compared with 6,989 on the parent and 7,021 on signed Chrome 158.0.8127.37. All 16 gains passed Chrome and matched occurrences in the frozen c3372ad21 census. No parent pass was lost and no name changed. All 83 Pyro-only passes held; 99 signed-only failures remain on other cases. The WPT ending cases can expect an alternating term/description type where signed Chrome continues the current type. The direct ending cases record Chrome's actual behavior.

The 48 direct fixtures combine DT/DD, P/DIV, start/middle/end, plain and formatted content, sibling paragraphs and locked siblings. Exact markup and both selection endpoints after splitting and after typing improved from 0/48 to 48/48. Connectivity of the original item, paragraph, text and neighboring item matched in all cases. The 48 ordinary OL/UL traces from the preceding batch were rerun on the candidate and every recorded field matched the archived parent. All 13,060 static, 300 child-line and 72 void-paragraph guard statuses held.

Undo grouping differs on both Pyro builds. One undo returns to the split in Pyro; Chrome groups the split and subsequent typing. After two undo calls, initial markup and both selection endpoints matched in 48/48. Original node connectivity after undo differs from Chrome in 48/48 on parent and candidate.

Independent QA confirmed exactly the 16 named signed gains and 48 direct rows. The implementation owner reran the separate OL/UL guard and the native paragraph/select subtree test, which passed.
