# Removing inherited bold while preserving paragraph and inline content

Tested source 426b11f7edf37260136e5513c278dd213ccb4614 against exact parent 775ab38c69d63c9e2631afebc5a0cb6068db29df. The parent executable SHA-256 is 57f53fac8952919df3639d7ffd6f70cb66b1d16a25f2606c9ea68bf185cc7416. The final executable SHA-256 is 8a729be3649d5b91b2a16d1f90932bd71f87d6a6f17b70ffba4322305969f110. Build receipts record the parent, first candidate 48b382751, second candidate bfdb018b7 and final candidate, including each source tree, compiler, profile and pinned JSC.

The bold command can now remove a selected span's explicit weight while keeping its color and other inline declarations, or override inherited bold inside a heading. For an explicitly weighted paragraph containing direct text or a single chain of simple inline elements, the edit leaves the selected text in place and carries the original weight onto unselected text. The original paragraph and italic element stay connected. More complex authored structures can use a normal-weight inline override.

## Named results

Independent QA verified **35 gains and zero losses** over 43,408 shared assertions. The 44 formatting pages rose from 29,547 to 29,582/33,316. The ten multitest pages retained 8,992/10,092 passes. All 928 existing Pyro-only passes remained (766 formatting and 162 multitest). Fully passing pages were unchanged. The formatting union includes another 36 Chrome-only occurrences.

All 34 original gains pass signed Chrome 158.0.8127.37. The additional pinned pass on bold.html?2001-3000 checks the resulting HTML for CSS-mode bold removal across part of an explicitly bold P. Pyro removes the now-empty style attribute. The signed reference retains an empty style attribute on P and fails the assertion. The second and final candidates have identical named outcomes. Each ledger contains URL, name, occurrence, parent, first, second, final and Chrome status. The WPT reference comes from the archived matched profile; direct paragraph measurements used the signed launcher.

## DOM, selection, styles and undo

The first candidate fixed the selected text's weight but added a span inside explicitly weighted P elements. That moved the range endpoints away from the parent/Chrome paths in the partial forward, partial backward and full selections. The second candidate restored those paths, including the caret typing case, by transferring the weight to the text outside the edit. The final candidate applies the same operation inside an existing inline formatting chain.

- In independent paragraph controls, both endpoint paths and offsets now match Chrome in **64/64**, up from 48 on the parent, four on the first candidate and 52 on the second. This recovers all eight italic-child partial/reverse cases across weights 600, 700, 900 and bold. All 16 styled-child cases retain the original P, I and original Text object; outside neighbors also remain in place.
- Bold state agrees in 64/64. Exact serialized markup agrees in 30/64 and complete per-character weight/color arrays in 46/64. For example, removing bold from the middle two letters of red 900-weight italic text leaves the two neighboring letters red and weight 900; the selected letters remain red and become 400. Chrome can use a legacy B element that changes an unselected numeric weight. It can also produce an empty style attribute on the I. The exact DOM and computed styles are stored in paragraph.json.gz.
- Undo restored starting markup and both endpoints in all 64. Pyro's history snapshot reconstructs nodes on undo; preservation of the P/I/Text objects above describes their identities immediately after the edit.
- Two independent 32-case cohorts covering headings and paragraphs retained Chrome agreement on computed colors, weights and bold state in 32/32 each; exact markup agreement was 30/32 each on the second candidate. The owner additionally recorded locked, plaintext, host-styled and normal-inside-bold behavior in shapes.json.gz. Its 44-case final markup agreement was 39/44, up from 11/44 on the parent. Exact computed text styles rose from 16/44 to 40/44. The normal-inside-bold enabling cases remain outside this removal correction.

Three exact native guards passed, including the backward selection test for heading identity, unselected weight, selected color and undo. All per-build evidence retains its original provenance.
