# Removing inherited italic from editable semantic elements

Tested source e79fded8c052055bd185ef876a2088c4e6cda2de against exact integrated parent c5aec33c68ef90999b3ead65754fb9497bf905e4. The candidate executable SHA-256 was b9432245c7d7b7cfe93789996a5f5d4a6e5dfcd9a6dd2cecfd437ad129fd628a and the parent's was 1db59574b2559aae3527c1d02307461dcdc7979f3f1df2e06cc04167e11b2094. The adjacent build receipts record both trees, the pinned JSC archive, profile and compiler. Commit c705e6d5 adds only the backward-selection native regression test.

The italic command now clears inherited font style without removing an address, citation, definition or variable element. With a partial selection it overrides the selected run. For a fully selected single-text inline semantic element the override can live on the original element. Clearing an authored font-style declaration preserves unrelated declarations. Bold and italic share the existing selected-run splitting and block-formatting logic, with separate rules for which semantic element reproduces the authored style.

## WPT

Independent QA confirmed **39 gains and zero losses** across 43,408 matched assertions. All 39 gains pass signed Chrome 158.0.8127.37: 24 italic-state queries, 13 innerHTML assertions and two indeterminate-state queries. The 44 formatting pages rose from 29,582 to 29,621/33,316 named passes. All 10,092 outcomes on the ten multitest pages were unchanged (8,992 passing). The fully passing counts remained 2/44 and 0/10. All 929 existing parent-only passes against Chrome were retained, including 767 formatting and 162 multitest assertions. The formatting union includes an additional 36 names emitted by Chrome alone.

The ledgers record URL, name, occurrence, parent, candidate and reference outcome. Both Pyro arms were measured against the same single-worker testdriver profile and pinned WPT revision. The signed WPT reference was reused; the independent direct comparison used the signed launcher. The 64 direct inherited-bold guard cases retained every parent observation on the candidate.

## Direct behavior

Across 40 independent ADDRESS/CITE/DFN/VAR cases, exact post-toggle markup improved from 8/40 to 40/40. Computed text styles and italic state also improved from 8/40 to 40/40, and each endpoint improved from 20/40 to 40/40. Selected text, selection direction, original semantic element, original text and neighboring elements agreed with Chrome in all 40. In all eight caret cases subsequent typing produced the signed reference markup.

Undo restored starting markup, selection endpoints and direction in 40/40. Pyro's history mechanism reconstructs original element and text nodes, while Chrome retains them. This difference also occurred on the parent.

A broader owner matrix exercised author CSS classes, mixed inline declarations, oblique angles, nested bold, plaintext and locked content. Of 96 cases, exact post-edit markup agreement improved from 24 to 78 and per-character font-style/color/weight agreement improved from 40 to 94. No formerly matching markup, computed-style, endpoint or node-retention observation became a mismatch. Residuals include re-enabling italic inside a normal-style child of CITE and differences in markup serialization. The four exact native checks cover the new italic behavior, prior inherited-bold behavior, formatting structure and undo.
