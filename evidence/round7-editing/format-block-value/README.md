# Querying and applying formatBlock

Tested source **936cd08445615aec327ebf838b1109952d6599ec** against exact parent **0961f10129454a559a3fb9ba67d90b6c8b4268b2**. The parent binary SHA-256 was 5ecaf1257b165f82aad85b2170f1493fe7bfa588df04e0357ac8c4cbbb4de458. The final binary SHA-256 was 4378ca8de4435081549062941fec514d60704c7cbdad67877e0684cb767a3ad0. All private builds used the same pinned JSC. The exact source, tree, compiler, profile and hashes for every revision are adjacent.

The query resolves the range's element boundaries inside its editing host and finds the formatting block enclosing both. During an edit, adjacent selected lines can share a new block with a BR between them. The operation keeps structural table cells/list items in place and retains non-formatting block objects when wrapping them. The ordered selection after the edit agrees with the signed reference in the three backward mixed-line cases.

## Independent WPT results

Across 44 formatting pages, 29,418 → 29,464 of 33,316 shared assertions passed (+125/−79). Signed Chrome 158.0.8127.37 passes 109 of the gains and fails the other 16. It fails all 79 losses with exactly the same actual value or serialized DOM and the same assertion message as the final candidate. Fully passing pages remained 2/44. All 10,092 named results on the separate ten multitest shards held (8,791 passing). The ledgers include the 36 Chrome-only formatting names. They are absent in every Pyro arm and excluded from the 43,408 matched-name denominator. Parent/candidate WPT used the single-worker wptserve testdriver profile. The pinned signed WPT receipts were reused; the owner and independent QA reran each candidate head.

| Trial | Gains vs parent | Losses vs parent | Chrome-passing losses |
|---|---:|---:|---:|
| c83149043 | 27 | 118 | 45 |
| 094fc49d2 | 94 | 83 | 4 |
| 3d4d8e0e3 | 124 | 91 | 12 |
| 936cd0844 | 125 | 79 | 0 |

All 27 original gains were retained. All 45 Chrome-passing first-trial losses, six mixed-line targets and 12 third-trial wrapper losses passed individually. Compared with the third trial, the final revision gained 13 and lost zero, all 13 passing Chrome. The full ledger columns are URL, subtest name, occurrence, parent, first trial, second trial, third trial, final and Chrome. The changes file preserves all original 118 losses as well as later outcomes.

## Why the 79 pinned losses remain

The upstream WPT fixture available from wpt.live during this handoff and the file in pinned WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603 had identical SHA-256 6d97bdba2cf8acf3635a80bc89aecc13aa0b6bd95db9be4ce2afda24a940c924. The fixture's bracket notation marks a range boundary; HTML parsing turns the two adjacent P start tags into sibling paragraphs. The historical editing reference algorithm block-extends the range and looks up the first visible editable leaf for the command value. Its wrapping rules for DIV/P explicitly decline to reuse a sibling wrapper. These support the fixture's expectations. The product choice here follows measured Chromium behavior for this legacy command. [Pinned expectations](https://github.com/web-platform-tests/wpt/blob/a421f2fea5eb7f91e0db399e44b0fb0779ee1603/editing/data/formatblock.js) · [Reference value algorithm](https://github.com/web-platform-tests/wpt/blob/a421f2fea5eb7f91e0db399e44b0fb0779ee1603/editing/include/implementation.js#L6766-L6807) · [Reference wrapper algorithm](https://github.com/web-platform-tests/wpt/blob/a421f2fea5eb7f91e0db399e44b0fb0779ee1603/editing/include/implementation.js#L6688-L6702) · [Upstream fixture](https://wpt.live/editing/data/formatblock.js)

| Occurrences | Operation and input | Pinned/upstream expected; parent passed | Final and signed Chrome |
|---:|---|---|---|
| 52 | Before formatting a selection from the first P into a sibling P, repeated across command/paragraph-separator variants | Query returns p, the first paragraph's tag | Empty string: neither paragraph encloses the whole selection |
| 21 | After the same selection receives an unsupported block name (20 cases) or its existing P format (one) | Query still returns p | The selection still spans sibling paragraphs, so the query still returns an empty string |
| 2 | Format as DIV across an already formatted DIV and following bare text, with either default separator | Two DIVs and query value div | The query returns an empty string for the resulting selection across the separate DIVs |
| 4 | Convert H1/H2 or P/H1 siblings to DIV, with either default separator | Two sibling DIVs, one per line | One DIV containing foo, a BR, and bar |

The 75 query outcomes and four serialized-structure outcomes are direct conflicts on the same measured inputs. Recovering the pinned outcomes would give a different result from Chrome on those inputs. The 45, four and twelve Chrome-passing losses that appeared during development were avoidable implementation defects and were corrected. No test expectation was edited. 79-pinned-losses.json stores the matched failure message and exact occurrence for every remaining loss.

## DOM, selection, identity and undo

- Independent query-only checks improved from 11/27 to 27/27. Reading the value preserved DOM, selection and history and emitted no input events. The owner's 90-case matrix improved from 30/90 to 89/90. The outstanding case is selecting all host contents ending in a noneditable island: parent returned section, final returned an empty string, Chrome returned p.
- Across 28 edits, post-edit markup, value, anchor and focus each matched Chrome in 28/28; parent agreement was 3, 10, 12 and 2 respectively. Selection.toString() matched in 8/28, up from 4/28. Across 48 expanded forward edits and 48 backward edits, each final arm matched markup in 44/48, value and anchor in 48/48, focus in 44/48, and selected text in 8/48. The forward parent counts were 3, 22, 24, 2 and 4. Four locked-neighbor cases retain a different DOM arrangement. Undo recovered starting markup throughout. The three independent backward H1 mixed-line probes matched DOM/value/anchor/focus in 3/3, while stringification matched in 0/3.
- For the 24 wrapper probes, parent and final both matched Chrome on post-edit markup, query value, selected text and offsets. Those metrics alone miss object replacement. Original wrapper retention was 16/24 on the parent, 17/24 on the final candidate and 1/24 in Chrome. Independent final measurements found the original text node retained in 24/24 on Pyro and 1/24 on Chrome. The candidate deliberately preserves the original listing, xmp, display-block custom and span objects and their content. Undo restored starting markup and both offsets in all 24 on the parent, final and Chrome. Post-undo node identity was not measured in this fixture.
- Three exact native guards passed, covering custom block/text identity and ordered selection after formatting, existing formatting/structure commands, and undo/redo restoration in controls and editing hosts.
