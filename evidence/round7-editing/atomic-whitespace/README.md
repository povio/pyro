# Whitespace after deleting an inline replaced box

Tested parent bc0cba7f504cca02541fc1127cfcf1fc6e9a9ea4, first candidate 902fa81f0f40e356e1fd96d02e0fcc8a84ce7c8d, and corrected source e8d39ab42a24515752800e8cff15fd579bee43d5. Each binary used the same pinned JSC. The adjacent build receipts identify each source tree, compiler, profile and executable hash.

Deleting an editable inline replaced box can expose whitespace in the text node immediately following it, or the preceding text node at the end of the container. The correction keeps whitespace repair in that node. In the first candidate, nowrap selected a different whitespace sequence and inadvertently extended the repair into preceding text.

## Named WPT evidence

Across 54 matched pages (13,060 occurrences), both candidates gained twelve against the exact parent and lost zero. All twelve gains pass the signed Chrome 158 run. delete-img and forwarddelete-img each rose from 1/7 to 7/7. All other named outcomes held. Fully passing pages rose from 37 to 39. Named rows are URL, name, occurrence, parent, corrected candidate and signed Chrome status. profiles.json records the runner, page statuses and denominators. Parent and reference ran once and their receipts were reused for the correction.

## Direct nowrap recovery

All four inputs contained the text node `a&nbsp;&nbsp; `, followed by the named element, then a text node containing `b`. The parent, final and Chrome each produced `a&nbsp;&nbsp; b` in all four cases. The first candidate produced `a &nbsp; b`. All four marked the edit undoable and restored starting markup on undo. The DOM caret endpoints below use zero-based child indices and UTF-16 offsets. Host means the editing host itself.

| Element and key | After deletion, all Pyro builds | After deletion, Chrome | After undo, all Pyro builds | After undo, Chrome |
|---|---|---|---|---|
| img, Backspace | host at 1 | child 0 at 4 | child 2 at 0 | host at 2 |
| img, Delete | host at 1 | child 0 at 4 | child 0 at 4 | host at 1 |
| canvas, Backspace | host at 1 | child 0 at 4 | child 2 at 0 | child 2 at 0 |
| canvas, Delete | host at 1 | child 0 at 4 | child 0 at 4 | child 0 at 4 |

Across the 28 cases, only those four observations changed from the first candidate. Exact post-delete markup agreement with Chrome rose from 13/28 on the parent to 15/28 on the first candidate and 19/28 after correction. Exact post-delete caret agreement stayed at 2/28, original-element presence at 23/28, and undo availability at 27/28. Starting markup was recovered in 28/28. The archive includes the original and restored node identities. In the terminal canvas fixture Chrome preserved the canvas in both directions, while both Pyro revisions and the parent removed it.

Independent QA repeated the final 54-page selection and all 28 direct controls. Its named outcomes matched the first candidate throughout, and its direct rerun confirmed each of the four nowrap recoveries. Both exact native filters passed on the corrected source.
