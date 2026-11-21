# Scripted rich-text formatting events

Scripted rich-host inline formatting, link creation/removal, list toggles and block formatting now apply the edit directly and dispatch input after the change. createLink reports the supplied absolute or relative URL in InputEvent.data. The same edit path records undo and preserves its existing selection behavior.

Across 33 WPT pages, both exact Pyro builds reported 24,098 named assertions and two fully passing pages. The candidate gained 14 previously failing assertions with no losses or membership change, taking passes from 20,400 to 20,414. All 14 gains also passed on signed Chrome 158. Twelve cover beforeinput suppression and two cover the data on createLink. All gains were on input-events/input-events-exec-command.html, where Pyro improved from 231 to 245 passes. The reference reports 36 additional names on that page, which appear with empty Pyro statuses in the ledger.

Independent QA reproduced all 14 gains. For 20 direct rich/plain host probes, post-edit DOM, selected text and undo eligibility matched signed Chrome in every case. Actual edit event sequences matched in 18. For rich formatBlock, Pyro reported inputType formatBlock and Chrome reported an empty string. Keyboard cancellation remained effective on both kinds of hosts, and plain hosts continued to suppress formatting commands. On scripted rich undo with a canceling beforeinput listener, Pyro retained formatting in four cases where Chrome restored the old content. Complete post-undo state matched in 16/20 probes and selected text matched in 20/20.

The 54-page static cohort retained all 13,060 outcomes. The 27 Enter pages retained all 2,852 outcomes. The prior control-command cohort contained the same 14 gains and retained every prior pass across its 4,597 Pyro names. The native pyro-webapi integration filters exec_command_formatting_and_structure and undo_and_redo_restore_text_controls_and_hosts each executed one test and passed.

## Provenance and reproduction

The frozen parent topic `0736e531e` added only evidence to the tested runtime `bacb54f825cd2795346e9247890ec52116c11181`, binary SHA-256 `c96794446f9b11c5673235847531785df9d9b6edea48739c994645b5e851c29d`. The clean candidate `514c4c941deff6766b49574c2ceb1e4765b47031`, tree `18327178b3a5d2e882d612330aa36d43b6ac4d83`, has binary SHA-256 `3c0381690b2fb09551fea252372cc037d84763380850b5faf93f043eaef3957b`. It used the private wptiter renderer build and pinned JSC archive `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029`.

WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Both Pyro versions and archived signed Chrome 158.0.8127.37 ran the main selection with wptserve, testdriver and one worker. Guard comparisons reran the candidate and reused the preceding batch's parent-runtime and signed Chrome receipts. Named rows contain URL, subtest, occurrence, parent status, candidate status and Chrome status.

```sh
gzip -dc primary.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing,input-events --server wptserve \
  --testdriver --jobs 1 --tests urls.json --bin "$LABEL=$BIN" --out "$LABEL.json"
```
