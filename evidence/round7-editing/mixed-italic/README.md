# Italic across plain text and semantic inline content

The baseline is integrated 0d161b84e. Its only differences from measured executable source 9208b1211 are the blockquote-list evidence files. The immutable baseline was rehashed before reuse, and its original WPT and build receipts are retained. Corrected source is 64235eb22e7a7628ab11e40a05b4432b33e59d17.

When a selection starts in plain text and includes an entire italic semantic element, the element can share the new italic wrapper with adjacent selected text. CITE, DFN and VAR retain their semantic identity. Redundant I/EM presentation is combined into the new wrapper. ADDRESS keeps its block boundary. A partly selected semantic element with an existing italic style needs no additional italic wrapper. A normal inline style on a fully selected semantic element can be removed while preserving unrelated declarations.

The first candidate 4bfc9d4ff used the prior all-selected command state when choosing the toggle. It gained the 24 targeted signed WPT passes but introduced ten endpoint regressions in the supplemental semantic-leading probes. Chrome toggles from the first selected content in document order on these mixed semantic selections. The correction uses that first run even when the anchor is at the other end. It distinguishes fully included semantic content from a partial range requiring a normal override.

## WPT results and accepted state discrepancies

The 44 formatting pages at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603 have 33,316 Pyro-matched occurrences. The parent passed 29,909; the corrected candidate passed 29,935. Independent QA confirmed **30 gains and four losses**. All 24 targeted gains pass signed Chrome 158.0.8127.37 and match historical URL/name/occurrence triples in the RC.5 census at 516dc3159. The other six gains are pinned assertions failing Chrome. No matched name appeared or disappeared. The complete ledger includes another 36 signed-only observations absent on both Pyro arms.

All four losses are pinned-only italic-state assertions, before and after each of these two selections:

| Initial range | Pinned expectation and exact parent | Signed Chrome and corrected Pyro |
|---|---|---|
| `foo<i>ba[r</i>b]az` | false → true | true → false |
| `foo bar <i>baz [qoz</i> quz] sic` | false → true | true → false |

The exact fixture probes also confirmed those values in the reverse selection direction. The coordinator accepted the signed behavior with all four losses counted. The WPT signed reference and parent arms were reused; both candidate revisions were measured. All 10,092 multitest and 13,060 static guard outcomes held. The native semantic italic removal regression passed.

## Direct controls

Across 96 core and 48 semantic-leading controls the final candidate matches every signed glyph's font-style and color after both the first and repeated toggle. All ten first-trial endpoint regressions recovered individually. No measured direct snapshot field lost a parent or first-candidate match.

In the core cohort (plain first), exact markup matches 94/96 after the first toggle and 90/96 after the second. Anchor/focus agree in 93/96 and 94/96 respectively. In the semantic-leading cohort, first-toggle markup, state and endpoints agree in 48/48. Repeated markup agrees in 26/48 and repeated endpoints in 36/48, despite matching glyph styles throughout. First-toggle original-node connectivity agrees in 96/96 core and 40/48 semantic-leading cases.

Undo return agrees in all 144. After two undos restored markup agrees in 120/144 and original connectivity differs in all 144, as on the parent. Intermediate undo, selected text, endpoints, direction, styling and node connectivity are recorded individually. Independent QA reran the eight-direction/shape combinations per semantic type on the corrected binary.
