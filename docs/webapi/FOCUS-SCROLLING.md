# Focus scrolling

HBL diagnostics on the production tree equivalent to main `198b2e2` placed the trigger around 32,325px below a 900px viewport. Chrome 147 placed the actual app trigger around 474px. The UI intentionally reverses options for top popovers; no JavaScript sort defect or selector repair is involved. Three app focus calls during opening requested no `preventScroll`, yet the rewrite's scroll offset remained zero.

A separate local HTTP Chrome control centers an offscreen focused button before `focus` and `focusin`; `preventScroll: true` leaves it offscreen. The rewrite previously never scrolled. The native repair converts the option before taking focus or DOM borrows, carries it immutably with the existing cancellation token, and reveals a valid new focus target before its events. Visible axes retain their offsets. Auto/scroll ancestors are processed inside-out, then the viewport. Existing synthetic geometry and scroll bounds remain authoritative. Scroll notifications use owner tasks and discard replaced or disposed documents.

Public focus-transition and owner-helper signatures remain compatible. Owner helpers receive the same token, including its immutable scroll option. They must continue checking cancellation and document liveness as described in [FOCUS-TRANSITIONS.md](FOCUS-TRANSITIONS.md).

This does not implement full CSS placement, transforms, fixed positioning, smooth scrolling, or the distinction between `overflow: hidden` and `overflow: clip` for ancestor programmatic scrolling. Child-frame geometry retains the existing browser/protocol limits. The actual HBL outcome is pending a signed integrated candidate and unchanged targeted rerun; the reduced default-action correction alone is not a compatibility claim.
