# Highlighting selected inline text and size carriers

Frozen source `bbb8a325699cc86323c3018114e605702f25bfdf`, tree `34650f8ed19fa87cfcc32f9b3c9b3ab9d4651799`, executable SHA-256 `95cd07cfc5289107a532e7050951d1ee13f7f919d9bcfbd872938dcfa2e2e6d3`. The integrated AudioEncoder inputs required an exact parent build at `89ac63995d3f245ff11c9429cdb0553a7b37ff9f`. Parent and all candidate build receipts are included. The archived WPT comparator retains its signed Chrome 158.0.8127.37 provenance. The direct probes measured each arm.

The edit applies hiLiteColor or backColor to the selected part of an inline size or paint carrier. A partially selected size carrier keeps its extent. A paint-bearing span splits with the original node retained on the unselected suffix. A fully selected carrier can share a highlight wrapper with adjacent text. Repeating that highlight replaces the wrapper and retains its children and their stylesheet boundaries.

## Results on pinned assertions

Independent QA matched 33,326 assertions on 44 pages. All eight frozen signed targets passed. The change also lost 36 parent passes against the pinned expectations; the name membership stayed constant.

- **24 markup assertions.** The candidate and Chrome produced identical actual HTML and per-character background/size arrays in each exact fixture. The direct comparison preserves the parent, both early candidates, final candidate and signed outputs. [Actual markup and paint](pinned-markup.json.gz)
- **12 event assertions.** For orange, #00FF00FF and #0000FF88 under both commands, the pinned tests expect formatBackColor and a string data value. Chrome and the candidate emit an empty inputType and null data. [Expectation and diagnostics](pinned-diagnostics.json)

The four block-level cases that passed on both parent and Chrome retained their passes. Four nested transparent SPAN cases also retained the parent’s pinned markup; the previously measured difference on the unselected suffix remains. Transparent command controls and earlier revisions are separately recorded.

## Direct comparison and history

Across 192 core cases, markup, paint/size/color/italic glyph vectors, endpoints, direction, selected text, indeterminate state and tracked connectivity match signed after both first command and repeat. Query value matches in 96/192. Across 192 neighboring cases glyph vectors match in 192/192 after both commands. All 16 original suffix connectivity losses and all eight stylesheet-class repeat mixed-state losses recovered. Reapplying a whole selected highlight creates a new wrapper in Chrome and the candidate; the children retain their styling. The mutation observer sequences differ in the number and order of internal DOM operations. The class-repeat command and undo events match the signed sequence.

Eight previously matching intermediate-undo original connectivity vectors differ. These are a subset of the 16 final-undo losses on already-cyan spans, full or partial selection, both directions, modes and commands. The parent skipped normalization, leaving undo disabled; signed Chrome and the candidate performed undoable edits. Pyro reconstructs the nodes on undo. The history suite covers the same 16 final fixtures. [Per-fixture history](undo-identity.json)

The separate repeat suite narrows the selection to offset 0–1 of the tracked text object. No previously matching glyph paint or size is lost against the parent on repeat or after undo. For the eight original spanning selections, all arms select “a” and paint every glyph cyan after repeat. The candidate’s extra nested carrier changes both endpoint paths in those eight. For the eight original prefix selections, Chrome’s original text object now starts with “c” while the original text object in all Pyro arms starts with “a”. Chrome therefore also paints “c”; the parent and both revised candidates leave it transparent. All 16 post-repeat paint/size vectors are identical to the parent, eight match Chrome. After undo, final paint and size match Chrome in 16/16 versus 0/16 on the parent. Four prefix backColor query values lose an earlier parent match after undo. Each of those four returned the parent’s transparent value after undoing the first edit; Chrome and the final candidate retained the first paint but query it differently at their respective endpoints. The narrowed-repeat evidence enumerates the characters selected by each engine, all neighboring colors/sizes and the eight endpoint/four query losses separately. [Four-arm narrowed-repeat observations](narrowed-repeat.json.gz)

Some block and nested-transparent fixtures still differ on intermediate undo. Reverting those cases to their prior editing path retains parent outcomes and loses some matches from the first candidate. Per-stage counts and field-level comparisons are in the direct and independent directories.

The 96 fontName, 96 fontSize and 96 foreColor controls produced identical records on exact parent and final candidate. The native font-size selection test passed. Fresh comparisons retained the parent status of all 10,092 command multitest and 13,060 static neighboring assertions.
