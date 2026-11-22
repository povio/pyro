# Enter in plaintext-only editing hosts

Pressing Enter in a plaintext-only editing host with a plaintext caret dispatches `insertLineBreak`. For preformatted text before an existing padding BR, the inserted linefeed preserves that BR. Rich editing hosts and explicitly rich descendants retain the paragraph action on Enter. Direct `execCommand("insertParagraph")` continues to use the paragraph command.

| WPT insertLineBreak variant | Base | Candidate | Signed Chrome 158 |
|---|---:|---:|---:|
| normal | 105/147 | 147/147 | 93/147 |
| pre | 93/147 | 132/147 | 129/147 |
| pre-line | 93/147 | 132/147 | 129/147 |
| pre-wrap | 93/147 | 132/147 | 129/147 |

Across 27 WPT pages with testdriver enabled, 2,444/2,852 passes rose to 2,603/2,852. All pages completed with harness status 0. The candidate gained 159 named passes with zero losses or membership changes. Signed Chrome 158 passes 138 of the 159 gained names. The 54 prior static-server editing, Range and selection pages retained all 13,060 outcomes. The ledgers are separate because their profiles and selected URLs differ.

The first behavior-verified candidate `e1a29d483` gained 150 and lost four assertions asking for a paragraph on Enter within a plaintext descendant of a rich host. The final candidate passes all four, covering both `contenteditable=""` and `"true"` rich hosts with and without explicitly focusing the descendant. The signed Chrome direct keyboard probe chooses a line break in this mixed-host case. The implementation retains Pyro's earlier paragraph behavior there. This is a compatibility tradeoff.

Independent QA reproduced all 159 gains, zero losses and seven fully passing pages, up from six. Sixteen direct keyboard/command controls are recorded in `direct-keyboard-controls.json`. Cancellation preserved the DOM in all four host configurations. Shift+Enter produced a line break in all four. Candidate event sequences matched signed Chrome on 10/16 controls, and serialized DOM matched on 5/16. Differences include paragraph wrappers, BR versus a literal newline, and an additional beforeinput event on the Pyro direct command path.

## Provenance

Base `1f1be032436bfeee9f48cdb5acbc0f71d21d5af9` has identical runtime source to tested `f23c31dde` (binary SHA-256 `c5132ef675c396c8cd07fa31055e0f3b4919702e6917d1ac26fc3bc305a342e2`). Final candidate `7b418ec9ee3626394e6a7e2f11a33806b4ffe75c`, tree `c2b3cc246de14518171508041f135384be881567`, has binary SHA-256 `8b263637e2e15017385f81743ecb148210b045b8d481b0855b45bd6c57602f33`. Clean source, pinned JSC archive SHA-256 `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029`, wptiter renderer profile, one Cargo job, private target directory.

An earlier shared-target Cargo invocation returned the unchanged combined parent executable, SHA-256 `543a7b85e3ecd8b99400d8c0c6861ff4756621b106d86407ca9d9098ca9b0d4d`. Its candidate results were withdrawn. The isolated first candidate executable had SHA-256 `c299b0878af728385ff3d37509fe9512170cdf579cc9e335443f1a2cbc2df2bd`; its plaintext beforeinput transition was verified before the nested-host correction.

WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. The 27-page comparison used the signed archived Chrome 158.0.8127.37 launcher, wptserve, testdriver and one worker. Exact base and Chrome runs were reused during candidate refinement. Static guard base and signed Chrome receipts came from the frozen void-insertion batch. Each ledger row gives URL, subtest, occurrence, base, candidate and Chrome status.

```sh
gzip -dc testdriver-named.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing --server wptserve --testdriver \
  --jobs 1 --tests urls.json --bin "$LABEL=$BIN" --out "$LABEL.json"

gzip -dc static-guards-named.jsonl.gz | jq -sc 'map(.[0]) | unique' > guards.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing,selection,dom/ranges \
  --jobs 1 --tests guards.json --bin "$LABEL=$BIN" --out "$LABEL-guards.json"
```

The exact native filters `editing_js::contenteditable_line_breaks_arrows_and_words` and `dom_js::contenteditable_typing_deleting_and_caret_keys_match_chromium` each ran one passing test in the pyro-webapi integration target.
