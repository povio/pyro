# Deleting a selection across shadow roots

Production source 6e242d0199972528c3c0c4ed86e3026259af0737 was tested against integrated fe759c18a. The parent used executable 305120731 because the nine intervening files were evidence only. The build receipts record the parent, first and corrected binary hashes, source trees and pinned JSC. Reference measurements used signed Chrome 158.0.8127.37.

Selection.deleteFromDocument() operates on its associated live Range. When the composed selection spans two roots, the associated Range can already be collapsed. In that case the DOM and selection stay intact. For a noncollapsed single-tree Range, deletion retargets the same Range object at the resulting caret. The corrected implementation also tracks that caret for composed snapshots when its shadow host is subsequently removed.

## Named results

Across 20 selection WPT pages, the target shadow deletion page improved from 27/29 to 33/33, matching Chrome. Two previously failing assertions now pass. Four additional assertions now execute because the surrounding tests finish without throwing; all four pass on both candidate and Chrome. No previously passing name was lost. Fully passing pages rose from 18 to 19. The ordinary deleteFromDocument page retained 60/60 and HTMLDetails retained 30/30. All 13,060 statuses in the separate 54-page guard were identical to the parent. The 20-page cohort was measured in all three arms. The 54-page comparison reused archived parent and reference receipts. Both native filters ran exactly one test.

## Range identity and host removal

Across 32 owner cases with open/closed roots and both addRange and setBaseAndExtent, the held live Range remained the same object immediately after deletion in 32/32 on the final candidate and Chrome, up from 8/32 on the parent. The held Range's endpoints matched Chrome in 32/32, up from 26/32. All 32 candidate light/shadow DOM results matched Chrome.

The first candidate lost six composed-caret snapshots following host removal. All six now collapse at offset 1 of the host's former parent, matching the parent executable and Chrome. These are same-shadow-root deletion through addRange and setBaseAndExtent, and deletion of an already collapsed shadow Range through addRange, each in an open and closed root. The other six owner removal cases retained their first-candidate outcomes. Including the four pre-existing light-tree endpoint differences, final composed endpoints match Chrome in 8/12 removal cases.

The held Range object remains inspectable after removal. In eight owner cases Pyro exposes no active legacy range after detachment, so callers cannot retrieve the held object again via getRangeAt. Chrome still exposes it. This legacy selection visibility difference also occurs on the parent. The owner-removal archive records retrieval identity separately from the held object's endpoints and the composed snapshot.

Independent QA reproduced the corrected 54+20 WPT cohorts and verified that all 16 direct post-deletion traces retained their first-candidate outcomes. It confirmed the same live Range in 16/16 immediately after deletion. After host removal the saved Range endpoints and both DOM trees matched Chrome in 12/12, active legacy anchor/focus matched in 4/12, and Selection.type differed in 12/12. The existing cross-root selection string difference remains visible in four of the original 16 controls.
