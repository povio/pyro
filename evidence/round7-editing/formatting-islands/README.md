# Inline formatting across editing boundaries

Rich-text formatting now skips locked content, reaches text inside explicitly re-enabled descendants, and preserves forward and backward selections. When a fully enclosed plaintext island lies between two selected text runs, their shared formatting wrapper can go around the island. Clearing existing formatting splits a shared carrier at the selection edges, so the middle can be unwrapped without cloning or detaching the island. State and indeterminate queries exclude locked text.

## WPT results

Both Pyro builds reported 33,316 named assertions across 44 completed pages. Passes rose from 29,391 to 29,402, with 11 gains, zero losses, no name changes and two fully passing pages on each build. Ten gains in the first bold shard also passed signed Chrome 158. The eleventh gain is an exact DOM assertion in the last bold shard; Chrome preserves the same locked markup but serializes an additional empty style attribute.

The first candidate, `965372c97`, gained ten and lost one assertion when it skipped a re-enabled rich descendant. The restored occurrence-1 assertion is in `restored-assertion.json`. It passed on the parent and final Pyro builds. Chrome failed its exact serialized HTML check.

## Boundaries and markup

One direct selection starts at offset two in `left` and ends at offset two in `right`, selecting `ftinnerri` across the intervening span. The probes exercise both directions. These bold examples omit the unchanged island ID for readability.

| Island input | Final result after applying bold |
|---|---|
| `<span contenteditable="false">inner</span>` | `le<b>ft</b><span contenteditable="false">inner</span><b>ri</b>ght` |
| `<span contenteditable="false"><i contenteditable="true">inner</i></span>` | `le<b>ft</b><span contenteditable="false"><i contenteditable="true"><b>inner</b></i></span><b>ri</b>ght` |
| `<span contenteditable="plaintext-only">inner</span>` | `le<b>ft<span contenteditable="plaintext-only">inner</span>ri</b>ght` |

The parent inserted a bold element inside the locked and plaintext spans. It did format the re-enabled rich descendant. The first candidate protected the lock but skipped that rich descendant. The final candidate matches the signed reference on all three examples.

Additional probes clear bold from a complete carrier and from a middle selection within a larger carrier. For the latter, the initial outer markup is `<b>left…right</b>`. Pyro and Chrome preserve bold on `le` and `ght`, clear it on `ft` and `ri`, and keep the original island. A genuinely locked island keeps its inner bold markup. The re-enabled rich child and fully enclosed plaintext override each lose their existing inner bold carrier. Neither the locked span nor the re-enabled host is removed. The intermediate `e94595543` left an outer bold clone around the island and reported a mixed bold state in the nested-rich case. The tested tip clears that clone and reports the same state as Chrome. Chrome additionally leaves `style=""` on the island after these removals. The direct ledger includes the exact parent, intermediate, final and signed Chrome DOM, selection endpoints and events.

The pinned bold WPT explicitly distinguishes preserving a noneditable element from clearing formatting on its editable descendant. The editing draft defines formattable nodes in terms of editability and visibility. The observed wrapper placement around plaintext islands is a signed-browser comparison. [Pinned WPT data](https://github.com/web-platform-tests/wpt/blob/a421f2fea5eb7f91e0db399e44b0fb0779ee1603/editing/data/bold.js#L1029) · [Editing draft](https://w3c.github.io/editing/docs/execCommand/#formattable-node)

Independent QA reran the exact tip and confirmed every WPT name and all 36 regular formatting probes. Regular post-edit markup matched signed Chrome in 36/36. All six partially selected carrier removals matched Chrome markup after omitting only its empty style attribute. Across those 42 probes, selected text, direction, query state, indeterminate state, element/child identity observations and edit event sequences all matched Chrome immediately after the command.

Undo restored original markup and selected text in 42/42. All 21 originally backward selections became forward on undo in Pyro, a parent behavior. Pyro's serialized undo restoration can also replace nodes rather than retaining their object identity, and its undo event traces differed in all 42 scenarios. This batch preserves direction across the formatting edit itself. The previous deletion batch's two-sided caret distinction (host offset one versus Chrome text offset three) remains separate.

Four native integration filters each ran one test and passed: inline_formatting_skips_noneditable_islands, removing_bold_preserves_editability_boundaries, exec_command_formatting_and_structure, and undo_and_redo_restore_text_controls_and_hosts.

## Verification and provenance

The clean base was `8e47c7148ea89dc8646c7e0b5efe48e4bb97aab5`, binary SHA-256 `88fd20729b7843c85f3f78dd55017800bffa7761218190d56ba9ed7fd1873cf7`. The tested tip was `49683701cd46f9b853cd978f5ab1aad0dbcdff1c`, tree `db8c352c17bc1588d44cf18e41ae42f6e6f8297e`, binary SHA-256 `4535189009fcb70d916d2c131712b7db6196a3031ed8afa0552b522225d8bdf8`. The private wptiter build used the tracked source at that commit, one Cargo job, and pinned JSC SHA-256 `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029`. Only evidence files were untracked at build time.

WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Exact parent and signed Chrome 158.0.8127.37 ran the primary pages under wptserve, testdriver and one worker. Those receipts were reused during refinement. The 36 additional names emitted by Chrome on input-events-exec-command.html are marked by empty Pyro statuses. The two exact Pyro builds also ran the three existing guard selections. The 13,060 static, 4,597 control-command and 2,852 Enter outcomes held. The guard ledgers reuse the signed reference from each corresponding profile.

```sh
gzip -dc testdriver-named.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing,input-events --server wptserve \
  --testdriver --jobs 1 --tests urls.json --bin "$LABEL=$BIN" --out "$LABEL.json"
```
