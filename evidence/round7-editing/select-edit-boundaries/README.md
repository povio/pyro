# Editing at select and option boundaries

The joined census at c3372ad21 contained 93 signed-only occurrences across the three delete, forwardDelete and insertText variants of editing-around-select-element.tentative.html. The exact same names and occurrences reproduced on integrated parent f579ffbf8. Its archived executable came from 12639aaec, with 14 evidence-only paths between the commits. All comparisons used WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Signed Chrome 158.0.8127.37 named results came from guard-main-003, guard-main-013 and guard-main-017 in the signed census.

The edit range distinguishes option content from selection of the entire control. A selection originating inside option text does not edit the option. Selecting an entire editable control from its parent can remove it atomically. The first candidate (4c27bad06) corrected DOM and endpoint agreement, but keeping the interior selection exposed 18 direction regressions. Chrome reports no rendered direction when both endpoints are inside option content, including separate selects. The second candidate (a90a59d26) corrected that observable independently of edit commands. It also preserved backward selections originating inside a control. Direct select-element endpoints and external ranges continue to have direction.

The supplemental forward selection from offset zero on the select into its option exercises another behavior. Chrome's backward deletion joins the control's inline line to the preceding editable paragraph. Its final direction is none because the edit collapses the caret at that seam. The reverse selection starts inside the option and remains backward. The third candidate (eae89185c) implements the editable paragraph join, reparenting the existing controls and consuming a terminating BR when present.

## Mixed endpoint comparison

The frozen matrix crosses three commands, four preceding contexts (editable paragraph, inline text, locked paragraph and no predecessor) and four following contexts (another select, text, a BR and text, or locked inline content). The first probe reused a document across cases and sampled undo eligibility. A separate pass used a fresh page for each fixture, recorded beforeinput and input, and executed undo. The original and fresh receipts are labeled separately. In the fresh Chrome arms the mixed selection and an explicit caret in the host before the control agreed on all 48 post-edit DOMs, endpoint pairs, directions, control connectivity, selected indices, events and undo eligibility.

The fourth candidate (26f2e984c) resolves the forward selection anchored at offset zero of a select with focus in its options to the outer parent/child boundary. It types at that edge outside the options, forward deletes the atomic box, and backspaces into preceding editable text or across an editable paragraph boundary. The reverse selection originates inside the option and remains unmodified. A locked predecessor or absent predecessor supplies no editable character for Backspace.

## Results

| Named WPT result | Parent | Final | Signed Chrome 158 |
|---|---:|---:|---:|
| delete | 3/37 | 33/37 | 31/37 |
| forwardDelete | 1/37 | 35/37 | 34/37 |
| insertText | 1/37 | 33/37 | 33/37 |

All 96 gains were matched Fail to Pass transitions. Of those, 93 pass Chrome and three fail Chrome. All five parent passes held and no names appeared or disappeared. The named ledger was identical across the four compiled candidates. All 13,060 static and 300 child-line guard statuses matched the parent, including 29 and 60 Pyro-only passes respectively.

The first 54 controls match signed post-command markup, both endpoints, direction, connectivity of the select/optgroup/option/text objects, undo eligibility and undo return in all cases. The detached original select still reports its previous selectedIndex in 12 cases where Chrome reports -1. The supplemental 36 now match post-edit DOM, endpoints, direction and selectedIndex in all cases. Their original shared-document undo-eligibility field depends on earlier edits and is retained as recorded.

| Fresh contextual cases | Exact markup | Both endpoints | Direction | selectedIndex | Original select connectivity | Undo return |
|---|---:|---:|---:|---:|---:|---:|
| Backspace | 8/16 | 8/16 | 8/16 | 16/16 | 4/16 | 8/16 |
| Forward Delete | 16/16 | 15/16 | 16/16 | 12/16 | 16/16 | 16/16 |
| Insert text | 16/16 | 16/16 | 16/16 | 16/16 | 16/16 | 16/16 |

On the original parent, all 48 contextual DOMs diverged, while direction and undo return matched in all 48 because Pyro edited the option and collapsed the selection. The fourth candidate matches 40 DOMs, 40 directions and 40 undo returns. In the remaining eight Backspace cases Chrome clears the entire host, including the locked paragraph when present. Pyro preserves every node and selection and creates no undo entry. These eight are counted as lost parent matches for direction and undo eligibility/return. In the one Forward Delete endpoint difference, deleting a select between a paragraph and locked inline content leaves Pyro at the host child boundary and Chrome at the end of the preceding paragraph.

After executing undo the original markup was present in all 48 candidate fixtures (40 edits undone and eight no-ops). For the 40 actual edits Pyro emitted beforeinput followed by input; Chrome emitted input alone. The eight no-ops emitted beforeinput without input. Exact edit event agreement was 0/48 on parent and final. In the four editable-paragraph joins Pyro keeps the original moved select connected while Chrome replaces it. Per-field receipts retain pre/post selection, the detached original control's index and connectivity, and separate history observations.

The compiler inputs were clean tracked source. The last three build provenance snapshots list the untracked evidence directory being assembled alongside the build. The superseded Rust 2021 syntax-only precursor 3cb60814c was never built.

Independent QA reran the target WPT and the frozen head probes. All 111 named results and all recorded rows from the 54, 36, 48 mixed and 48 explicit outer-caret head probes matched the owner receipts. The native paragraph/select boundary regression passed. Static and child-line guards were measured by the implementation owner.
