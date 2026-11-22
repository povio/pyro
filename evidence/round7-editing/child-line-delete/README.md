# Preserving a child paragraph when deleting its preceding lines

Tested source 4478f9e89841d147cf2367988e9199ce90f0b975 against exact integrated parent 4ebaf69178ea80f6b59bb9cd8b9a03995ef13fb4. The parent executable SHA-256 was b9bdef7cbe921ade8b0bcd9f22edad19950b960b7d47365c91ad688581dd420d. The final executable SHA-256 was 65d6cd47283ff2f7fa9891599a9a8a636dce7cb1ef051b457b8ab3b1fab47a39. The build receipts retain source trees, compiler, flags and pinned JSC.

Deleting an entire anonymous line immediately before an editable child paragraph leaves the paragraph in place and sets the caret at the beginning of the surviving content. Typing over the same selection inserts into that paragraph. The deletion plan distinguishes rendered BRs and preserved linefeeds from hidden content, noneditable islands and breaks belonging to another block. The same plan supplies the beforeinput target range. For a suffix removed inside a surviving inline element, the target ends inside that element. Collapsed deletion repairs the exposed whitespace at the child caret.

## Frozen-census selection and fresh WPT evidence

The local even-shard census of c3372ad21 recorded 155 signed-Chrome-only occurrences across five variants of this test. They identify the opportunity. The exact current parent and candidate were measured separately across all eight pinned variants. All 155 historical URL/name/occurrence triples matched current observations and pass on the candidate. Of those 155, the current parent reported Fail for 125 and did not reach 30. The historical rows and the join are in historical-even-subset.json.gz.

| Primary eight variants | Parent | First 0effbb0e9 | Final 4478f9e89 | Signed Chrome 158 |
|---|---:|---:|---:|---:|
| Named passes | 4 | 296 | 300 | 240 |
| Named assertions observed | 204 | 300 | 300 | 300 |
| Fully passing pages | 0 | 4 | 8 | 4 |

Independent QA confirmed **200 previously observed Fail → Pass transitions and zero lost passes**. Every one of those 200 passes signed Chrome (100 keyboard, 100 script). The WPT registers target-range assertions conditionally after earlier checks succeed. The final candidate reached 96 additional named assertions, all passing; Chrome passes 40 of those and fails 56. All four parent passes were retained, including the preformatted forward Delete target-range case lost on the first trial. This yields 296 additional passes when matched and newly observed names are counted together. The signed total and the Pyro total describe distinct outcomes.

The separate 54-page guard had two matched gains and zero losses across 13,060 names, rising from 12,821 to 12,823 passes. Both gains pass signed Chrome and concern a caret after two BRs at the beginning of a paragraph. All six intermediate NBSP losses recovered: backward deletion across sibling and nested breaks, with NBSP-only and NBSP-plus-text suffixes, and forward deletion before the two nested suffixes. All 29 parent-only guard passes were retained. Fully passing guard pages remained at 45/54.

primary-named.jsonl.gz and guard-named.jsonl.gz use URL, name, one-based occurrence, parent, first candidate, corrected candidate and signed Chrome columns. Null means that arm did not report that occurrence. The changes files isolate the directional transitions without dropping intermediate losses. The primary Pyro arms used one wptserve/testdriver worker and WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Signed Chrome 158.0.8127.37 primary outcomes were read from archived signed shard data by URL; the guard used its archived static reference. Independent QA repeated both candidate profiles. The frozen census pin and results were not modified.

## Direct DOM, target ranges and history

Across 36 key controls, exact post-command DOM agreement with signed Chrome improved from 11/36 to 24/36, anchor agreement from 15 to 29, and focus agreement from 14 to 28. Exact event sequences including target ranges agreed in 9/36 on parent and final, versus 5/36 on the first trial. No parent-matching post-command field in those controls became a mismatch. The locked and hidden preceding Backspace cases individually match Chrome's post-command fields, event sequence, and undo snapshot. The direct archive includes cancellation, unrelated sibling, middle-of-line, BR, preserved newline, and inline-wrapper cases.

Undo restored starting markup in all 36 cases in every arm. Whole undo snapshots matched Chrome in 10/36 on the parent and final. In two forward Delete cases, the parent did nothing and kept its original nodes. The candidate and Chrome delete the empty line, preserve the child paragraph through the edit and make undo available. Pyro reconstructs nodes from its history snapshot on undo; Chrome retains their identities. Thus the parent had matched post-undo identity in those two cases via a no-op. This discrepancy is distinct from restored markup and post-command node retention. All three exact native guards passed, including the child identity, insertion-at-caret and undo regression guard.
