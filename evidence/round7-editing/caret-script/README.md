# Subscript and superscript typing at a caret

Final source 8539ce9d656e823d7b8fcb20b16f22ff58f3afbd was tested against exact integrated parent 2de0ad05cc6bc18fda0f1c56acbeaf207889f9a1. The first candidate was 25f325bde6129b310e6aa6f60871f1a6577c1b2e. Parent and final executable SHA-256 values are 15647520e93ac68df0029bdf20c39a75c097773bcc09e28b86fac395fd0fb5b2 and 6f30952b2f45493f58478b504583276040da3430f2bdeb539400a1fc0cf0612d. Build receipts record each Git tree, profile, compiler and pinned JSC archive.

At a rich caret, the commands set the typing style without editing the DOM or adding a history entry. Switching scripts changes which style the next character receives. Typing splits semantic or inline styles where needed and keeps unrelated formatting. With a stylesheet-defined script, removing the typing override leaves the author's rule in control. An outer script remains observable through a nested inline element until text is actually inserted.

## Named WPT outcomes

| Cohort | Pages | Shared names | Parent passes | Final passes | Gains passing Chrome | Other pinned gains | Losses |
|---|---:|---:|---:|---:|---:|---:|---:|
| Formatting | 44 | 33,316 | 29,464 | 29,480 | 16 | 0 | 0 |
| Multitest | 10 | 10,092 | 8,791 | 8,897 | 94 | 12 | 0 |

Independent QA repeated the frozen final candidate and verified all 43,408 shared outcomes matched the first candidate. All 904 parent passes that failed signed Chrome 158.0.8127.37 were retained (766 formatting, 138 multitest). The other twelve gains concern the two scripts' queryCommandState results after forwardDelete, indent or justifyLeft. All earlier passing page counts held. The formatting union includes 36 additional Chrome-only names absent in both Pyro arms. Each ledger records URL/name/occurrence with parent, first, final and Chrome status. Exact parent/candidate WPT were measured; WPT Chrome receipts came from the earlier matching signed profiles. QA verified the final receipts and used archived first-candidate owner results for its incremental comparison after a raw output filename collision.

## Direct behavior

- Independent QA ran 84 primary cases across keyboard and scripted typing, authored semantic and inline CSS, bare text, plaintext and locked content. The two pending state flags matched Chrome in 84/84, up from 24/84 on the parent. Typed markup agreement increased from 24 to 64/84. Bare and semantic sub/sup markup matched 36/36, inline CSS 16/24, plaintext 12/12 and locked content 0/12; the locked difference existed on the parent. Typing event sequences agreed in 72/84.
- The correction retained every primary direct observation and restored the four owner class-only markup/state losses and four related class-plus-italic repeat-toggle cases. In 24 independent class-only cases, typed markup agreement rose from 8/24 on the first candidate to 24/24. For example, in a class-styled subscript span the off command can report false while pending. The next character remains in that span, and the rule makes the resulting state subscript again. Undo restored initial markup in all 132 independent cases.
- In 24 additional nested-script cases, immediate state flags improved from 0/24 to 24/24; post-typing flags also matched 24/24. For a subscript around italic text, Chrome can report both the outer subscript and a pending superscript. When the new character is typed, it receives the superscript and stays italic. Pyro splits the italic element around the insertion, while Chrome can move a single italic element outside the separate script fragments. Exact markup in these nested cases matched 0/24 on both candidates. The before/after records include the neighboring text and undo.

## Rendering comparison

Sixty owner cases used a 20 CSS px monospace host. The typed X's computed vertical-align and font-size matched Chrome in 60/60, up from 36/60 and 24/60 on the parent. Its font style, weight and color agreed in all 60. Measured glyph height agreed in 60/60, up from 24/60. These style measurements do not imply pixel agreement.

The Y position of X relative to the preceding character matched Chrome exactly in 6/60 on the candidate and 12/60 on the parent. The six newly differing cases all placed an explicit baseline span inside SUB and toggled back to ordinary typing. Pyro preserves the unselected characters' 16.6667px size and types X at 20px. Chrome also types X at 20px but removes the surrounding SUB from the neighboring characters, making them 20px. The resulting relative offset was −1 CSS px on the candidate versus 0 in Chrome. The parent left X itself at 16.6667px. The full geometry records retain both the intended new-character styling and this neighboring-content tradeoff.

A clean-document replay separated markup effects from rendering differences. The parent and corrected binaries gave identical per-character geometry for all 120 fixed inputs (60 candidate-produced fragments and 60 Chrome-produced fragments). With the same Chrome-produced fragments, exact relative Y agreement between runtimes was 12/60. Switching the input fragment from Pyro's output to Chrome's output changed the relative offset in 30/60 cases within each engine. The replay and direct edit receipts are separate.

Three exact native guards ran one passing test each, including the new caret toggle, typed markup and undo test. The owner and independent QA used one runtime worker per allocated lane, and the private serial compiler slot was released before runtime checks.
