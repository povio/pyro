# Bold and italic at a collapsed caret

Tested source a8e6a9f1410b596386315c42e7d03a4237b1f8a4 adds bold and italic to the pending typing state. Toggling at a rich caret updates queryCommandState immediately. Typing applies the requested style to the inserted text. A selection move clears the override. Both exact native guards passed.

The parent 4fbfe2112 differs from 8500577ef only in evidence. Its reused executable had SHA-256 0a2103841c39019621a03700a23aa750df4a259bae89a1debd2369c2d88ac35c. Candidate SHA-256 was 52759c8aba6af1ab8d9a899f3a769ceafe8caa098f5b9ee4d943da25d9ae9138. Both used the same pinned JSC. profiles.json records the exact binary paths, selectors, WPT revision, page statuses, runner profile and counts. Parent and signed Chrome 158 receipts came from the preceding link-typing run. Independent QA repeated the candidate.

## Named WPT results

All three engines reported 10,092 multitest and 33,316 formatting occurrences in common. The ledgers contain 43,444 distinct URL/name/occurrence keys because Chrome reported 36 additional formatting occurrences absent on both Pyro binaries. Those 36 do not enter either the matched denominator of 43,408 or the gain count.

Independent QA confirmed 38,032 → 38,179 named passes (+147/−0), with 2/54 pages fully passing on both. Multitest gained 131 (118 signed Chrome passes). Formatting gained 16 (all signed Chrome passes). Of the 13 gained assertions Chrome fails, 12 cover queryCommandState after forwardDelete, indent or justifyLeft, and one checks exact wrapper ordering when italic and bold are combined. The named ledgers identify every occurrence.

## Direct controls

Independent QA exercised 40 combinations of bold/italic, keyboard/script insertion, caret movement, cancellation and a second toggle in rich, inherited, plaintext and locked content. Against signed Chrome, pending state improved from 28/40 to 40/40. Edited markup rose from 6/10 to 10/10 in rich text and from 6/10 to 10/10 for inherited formatting. Plaintext retained 10/10. Locked content retained 4/10. With an interior caret, Pyro can insert ahead of the locked span while Chrome leaves the markup untouched. Edited event sequences retained 26/40 agreement. After undo or no edit, markup matched Chrome on all 40 parent and candidate cases. qa-controls.json.gz preserves DOM, endpoints, state, event sequences and history availability for each arm.
