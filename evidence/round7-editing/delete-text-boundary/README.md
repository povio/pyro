# Character deletion across adjacent text nodes

Tested parent 9e33d57db51611e341c94e163da60ebce5d540e7, first candidate 419af3bc04fdd6eedc9106d7f10914c78afc9200 and corrected source c2e7abda1c14afe85b01deaee0441dd0596aa11f. Separate build receipts record the three executable hashes, source trees, build flags and common JSC archive. The reference was signed Chrome 158.0.8127.37.

Character deletion now keeps the caret at the end of the preceding text node when two sibling text nodes share a caret position. The engine repairs exposed whitespace within the affected node and removes empty text nodes encountered at the boundary. The correction checks the whitespace before deletion so ordinary spaces that already collapsed do not become newly visible NBSPs.

## Named outcomes

Across the same 54 pages and 13,060 names, passing assertions increased from 12,721 to 12,780. Backward deletion gained 28, rising from 542/584 to 570/584. Forward deletion gained 31, rising from 538/602 to 569/602. All 59 gains pass signed Chrome, and all other statuses are identical. Fully passing pages remained at 40/54. The first and corrected ledgers are byte-identical. Parent WPT was run once, candidate WPT was run separately for each executable, and the archived static signed Chrome receipt was reused.

## Consecutive ordinary spaces

The 18-case owner width matrix used an unpadded, borderless inline-block editing host with a 20 CSS px monospace font. Both the host width and the range rectangle were recorded, along with literal text nodes, markup, focus, identities and undo. The first candidate made ten normal/nowrap cases wider than the parent by 12 to 36 CSS px. The correction restored exact parent markup and host width in all 18 cases.

For example, with two spaces at the end of the left node and three before “b” in the right node, Backspace at offset one of the right node gave these results. Widths are rounded to two decimal places.

| Runtime | Result | Host width |
|---|---|---:|
| Parent | a, four ordinary spaces, b | 36.02 px |
| First candidate | a, two ordinary spaces, NBSP, ordinary space, b | 60.02 px |
| Corrected | a, four ordinary spaces, b | 36.02 px |
| Chrome | ab | 24.03 px |

The remaining difference is also present in the parent. Chrome deletes a rendered collapsed whitespace unit, while Pyro can delete only one underlying character. In the six preformatted cases all four arms retained the same text, and the browser widths agreed within 0.02 CSS px.

## Direct deletion and undo

In 48 owner cases, complete post-edit focus node/offset agreement with Chrome rose from 23 to 42, surviving-node identities from 34 to 46, and exact node text arrays from 28 to 40. Post-edit serialized markup matched in 40/48 on both parent and final. Undo restored starting markup in all cases. Pyro's history restoration reconstructs the host, so the original adjacent text-node identities and caret representation can differ from Chrome after undo. Both exact native guards passed on the corrected source.

Independent QA reproduced all 13,060 corrected candidate statuses. Across its 18 direct scenarios, anchor node agreement rose from 9/18 to 18/18, offsets from 5/18 to 10/18, and node identity/content arrays from 8/18 to 10/18. Post-edit markup agreement remained 10/18. Undo restored starting markup in all 18. An independent 12-case width check confirmed three ordinary U+0020 characters and zero NBSPs after deletion. Normal and nowrap remained at 36.015625 CSS px in eight cases. Preformatted text decreased from 72.015625 to 60.015625 CSS px in four. Undo restored all twelve initial widths.
