# HTML insertion into plaintext editors

`execCommand("insertHTML")` converts a detached HTML fragment into text for plaintext editing hosts and text controls. Single-line inputs collect source text in tree order, with line feeds and HTML breaks becoming spaces. Plaintext hosts and textareas use rendered-text collection for block separators, breaks, whitespace and hidden elements. The detached conversion uses element defaults and supported inline styles. It does not cascade document stylesheets onto the fragment.

| Target WPT | Parent | Candidate | Signed Chrome 158 |
|---|---:|---:|---:|
| Plaintext insertHTML, normal | 9/20 | 20/20 | 8/20 |
| Plaintext insertHTML, pre | 9/20 | 20/20 | 13/20 |
| Plaintext insertHTML, pre-line | 9/20 | 20/20 | 12/20 |
| Plaintext insertHTML, pre-wrap | 9/20 | 20/20 | 13/20 |
| Text control command page, password (passes) | 606 | 616 | 632 |
| Text control command page, text (passes) | 610 | 620 | 642 |
| Text control command page, textarea (passes) | 610 | 620 | 648 |

Independent QA verified +74/−0 over 4,277 named assertions on 15 testdriver pages, from 3,796 to 3,870 passes. Fully passing pages increased from two to six. Signed Chrome 158 passed 54 of the gained names. All six beforeinput checks that failed on the first candidate pass on the final one, individually, for standalone and nested text, password and textarea controls. The other 54 static guard pages retained all 13,060 named outcomes.

## Event, selection and content checks

* A scripted insertHTML into a mutable text control applies the change, updates selection and fires input. Keyboard replacement retains its cancelable beforeinput dispatch. All four direct keyboard scenarios preserved content on cancellation and restored it on undo.
* Fresh readonly and disabled input and textarea probes matched signed Chrome on all four cases. Query and execution returned false, the value and selection stayed unchanged, undo remained unavailable, and no editing events fired. Pyro blurred disabled controls sooner than Chrome at the synchronous observation.
* Twelve direct HTML insertion and undo probes covered rich, plaintext, input and textarea destinations, each with formatted, hidden and preformatted content. Final Pyro matched Chrome on content and selection in ten of twelve cases. Both engines restored original content and selection on undo in all twelve.
* The two content differences involved preformatted source markup in rich and plaintext hosts. Pyro kept the pre element in the rich host and used nonbreaking spaces plus a BR in the default plaintext host. Chrome flattened the rich pre and preserved literal whitespace/newlines in the plaintext host.
* Insertion event traces matched Chrome in the three single-line input probes. Chrome emitted only input for each script command. Pyro still emitted beforeinput for rich/plain hosts. Textarea commands now suppress beforeinput and retain the WPT-expected insertText input type; measured Chrome reported an empty input type. Direct undo also produced an additional beforeinput(historyUndo) in Pyro.

The builder reran all 12 insertion and four keyboard probes on the exact final binary. Every captured row matched the independently verified preceding candidate. QA independently reran the final WPT cohort and immutable-control probes. Four native filters each ran one passing test in the pyro-webapi integration target: rich paste sanitization, keyboard clipboard events, formatting/structure commands and undo/redo.

## Reproduction and receipts

WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. The parent was built from clean `59916483290abb28e78387cb22e9d79dad2f53b9`, binary SHA-256 `6d40cd31fa7926a287b81ddd83c69eaf743372057a01ef8a73e0b644ffcb45b8`. The candidate was built from clean `fff149719e170b4b2a5ef23e838fadfb87d686d5`, binary SHA-256 `c3675f12f59508d7d48e42644bcc588871dcc2de3a86e5964933f490838efa22`. Both used the private wptiter target and pinned JSC archive `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029`.

The testdriver comparison used the archived signed Chrome 158.0.8127.37 launcher against wptserve. Base and Chrome results were measured during the first comparison and reused after the event and eligibility fixes. Both exact Pyro builds ran the static guard set; its signed Chrome comparator comes from the same WPT revision and static profile in the earlier void-insertion results.

The two compressed ledgers contain URL, subtest name, occurrence, parent, candidate and signed Chrome status. The six restored assertions, immutable-control results and direct DOM/selection/event traces are provided alongside them.

```sh
gzip -dc testdriver-named.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing --server wptserve --testdriver \
  --jobs 1 --tests urls.json --bin "$LABEL=$BIN" --out "$LABEL.json"

gzip -dc static-guards-named.jsonl.gz | jq -sc 'map(.[0]) | unique' > guards.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing,selection,dom/ranges \
  --jobs 1 --tests guards.json --bin "$LABEL=$BIN" --out "$LABEL-guards.json"
```
