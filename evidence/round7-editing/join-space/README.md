# Whitespace where sibling paragraphs join

Tested b73d56f8e055acddbed0fb6fb3872d9888b16637 against the shared exact cc4f21a5ed6c742057bd541ce7f7ef3791b37aa8 executable. Its SHA-256 was e1e2e303c8a02c93083d6e3591e07c05dedc28cd964a414562b48c863929ed47. The candidate SHA-256 was a3f1512358fb43a3e5ed378c4d1df324cf983e88d513eb297d3eca2f1c4e1e28. Both builds used the same pinned JSC.

When deletion merges two paragraphs, a run of nonbreaking spaces touching the join can span their original text nodes. The engine rebalances that run from left to right, keeping a nonbreaking space at a line edge when necessary. Restricting this repair to runs entirely made of NBSPs preserves their known character width. Ordinary and mixed whitespace runs can have collapsed at the original paragraph edges and still have interoperability differences.

## WPT results

Independent QA confirmed 12,785 → 12,797 passes across 13,060 matched named occurrences on 54 pages. The six sibling cases on each of delete-to-join-blocks and forwarddelete-to-join-blocks changed from fail to pass. All twelve pass signed Chrome 158. The twelve nested-block cases on each page still fail in Pyro. No other named result changed, and fully passing pages remained at 43/54. In a separate owner testdriver run, all 426 assertions across nine preformatted block-join variants passed on both builds and the archived signed reference. Parent and candidate runs were fresh, and the two WPT reference ledgers reuse archived signed measurements.

## Direct controls

Independent QA exercised 42 cases across both deletion directions, normal/nowrap/pre whitespace, empty blocks, and styled or locked neighbors. Exact post-edit markup agreement with Chrome increased from 12 to 16/42 and text agreement increased from 14 to 18/42. Caret agreement remained 22/42. The candidate introduced no new markup mismatch, and all 42 agreed on undo eligibility. Undo recovered the initial markup in 42/42 and caret in 39/42.

The separate owner matrix included pre-line and both one-sided and two-sided NBSP runs. Post-edit markup agreement rose from 12 to 36/48 and caret agreement remained 42/48. Range bounding widths agreed with Chrome in all 48 on both builds. Undo recovered the initial markup and caret in all 48. The raw records distinguish serialized markup, node retention, caret and width.
