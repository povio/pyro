# Formatting visible whitespace between selected inline spans

Tested source `25353ada439cf8ee504167307f4eae019635afee`, tree `4337512f578ceb096c454122a830baf275248daf`, executable SHA-256 `28aef500f6abdab8d6e4b313457df54645959745bf6ad9935bfd23f3bc10f5e4`. Exact parent `b5175107eab1e5b3c4d7c9dee9deb71c1c3e5007` required a build for its Web Audio changes. The initial compile stopped before linking on ambiguous Rust type inference; the first runnable candidate and later correction each have separate provenance. Signed WPT rows retain their original Chrome 158.0.8127.37 provenance. Direct Chrome, exact parent and candidate probes are archived alongside their fixtures.

Independent QA matched 33,316 assertions across 44 WPT pages. All 19 frozen signed-only assertions recovered across 12 commands, with no losses and identical name membership. The command wraps the selected sibling spans and the visible separator together. Partial and locked selections use existing per-element formatting.

Of 288 direct cases, all match Chrome on first-command selected text, direction and original node connectivity. First markup matches in 244, both endpoints in 274, and glyph vectors in 280. Repeat markup matches in 250. Final undo markup matches in 284, endpoints and glyph vectors in 288. Original tracked-node connectivity after final undo differs in 288. No measured field lost a previous match compared with the parent or first executable. All 36 declaration controls match exact repeated-command markup and glyph vectors, including important, multiple and custom declarations.

## Repeated links and history

In all 32 history controls, the first and repeated commands and both undo operations returned true. Undo eligibility was false initially, true after each command and the first undo, then false after the final undo. Both same- and different-href repeats replace the selected anchor while retaining its child nodes. Each emits insertLink twice across the two commands, then historyUndo twice. Candidate and Chrome agree on markup, endpoints, selection direction, glyphs and input-event sequence after repeat and each undo. Chrome reconnects the first anchor after the intermediate undo. The candidate restores equivalent markup with a reconstructed anchor. Original child-node connectivity after the final undo also differs in all 32.

The first executable skipped the same-href repeat. The correction recovered eight forward anchor and eight backward focus comparisons across four whitespace/style shapes and two CSS modes. It also recovered both repeat markup regressions on styled CSS fontSize by serializing the retained declarations. The independent comparison records every fixture and field.

## Neighboring controls

The 80 collapsed-whitespace fixtures from the earlier format-collapsed-space group were run on the exact parent and both candidates. All recorded rows are identical. Historical signed controls retain their [original provenance](../format-collapsed-space/README.md). The native numeric-bold selection test passed. All 10,092 multitest and 13,060 static neighboring WPT results retained their parent status. Supplemental event probes also record remaining differences in value-command inputType (font name and foreground/background colors).
