# Selected underline and strike-through on elements with authored decorations

Frozen source `96938716f0a394a3202e39347bc6e69128488127`, tree `c85b71f8b7c2edbcc03a373cbe4de9fe4f44d5c7`, executable SHA-256 `f536faf5d16982d34ceae8f3b2f3ef6d1c8ceb9a490c5de23021e4910f477aca`. Parent `3c26eb435` differs from tested `df1c0bb23` only in evidence. The parent executable and named WPT records come from the original noncollapsed-script run. Parent/head direct comparisons use signed Chrome 158.0.8127.37.

Independent QA matched 33,316 named assertions across 44 pages. All 36 frozen targets and four additional signed passes recovered. All three pinned losses are counted. Membership remained constant. Each loss selects the middle character of a semantic element with an overline. Chrome and candidate both split the original wrapper around that character. Legacy mode puts a strike or underline inside a span carrying the overline. CSS mode puts underline and overline on one span. Exact markup and states for all three arms are recorded under [pinned](pinned). The parent passed all three pinned markup assertions.

In 96 semantic controls, the first command matched Chrome in every recorded field. Repeat matched glyph decorations, states, selected text, direction, endpoints and tracked node connectivity in all 96. Repeat markup matched in 80. All eight initial identity and 32 repeat glyph regressions from the first candidate recovered. Across 96 ordinary controls, repeat glyphs matched in 96, endpoints in 80 and markup in 46. The two previously lost ordinary endpoint comparisons recovered. Across 48 additional authored-property cases, all glyphs retained their original text color and overline color, thickness and dotted style on both toggles and both undos. Chrome reset decoration properties in this cohort.

## Atomic undo

Undoing the repeat restored each candidate’s recorded first-command markup, glyph decoration lines, both query states, selected text and direction in 192/192 cases. Endpoint paths and offsets returned in 88/96 semantic and 82/96 ordinary cases. Original tracked-node connectivity vectors returned in 0/192. The per-case comparisons are stored in revision3.

Compared with signed Chrome, semantic intermediate-undo markup and glyph decorations match in 48/96, queried underline and strike in 76/96 each, and endpoints in 88/96. Four glyph, eight underline-state and twelve strike-state matches from the parent diverge. Chrome clears inline decoration during its intermediate undo in affected cases. The coordinator accepted restoring the actual first-command formatting. Final undo matches Chrome on endpoints and decoration lines in all 192, with markup matches in 48/96 semantic and 88/96 ordinary cases.

The native collapsed-decoration typing guard passed. All 10,092 command multitest and 13,060 static guard rows kept their parent status using the archived parent arms and fresh candidate runs.
