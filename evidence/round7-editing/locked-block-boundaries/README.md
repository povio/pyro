# Deleting at a noneditable block boundary

At a collapsed caret, Backspace or forward Delete toward a locked block now checks for reachable content beyond the block in the same editing host. When that side ends, the block stays connected and the control does not create an undo step. Rendered content on the far side permits atomic removal of the locked neighbor. The deletion plan is also used for beforeinput target ranges. Explicit range deletions follow their existing path.

## Direct outcomes

Independent QA ran command and real keyboard probes on the final binary. Both directions are included.

| Cases | Final comparison with signed Chrome 158 |
|---|---|
| 24 one-sided collapsed carets at block, flex, grid, inline, inline-block and contents boxes | DOM, original node identity and undo eligibility matched in 24/24; caret endpoints matched in 12/24. At a terminal text edge the candidate keeps locked block-level boxes and removes inline neighbors as before. |
| 16 far-side probes | DOM, identity and undo eligibility matched in 16/16; endpoints matched in 12/16. Visible content permits removal; hidden, empty or absent content keeps the block. |
| 24 explicit selections | Candidate behavior was unchanged from the parent. Resulting DOM differed from Chrome in 24/24. |
| 8 empty-placeholder cases | Candidate behavior was unchanged from the parent. Resulting DOM matched Chrome in 4/8. |

Undo, or the original no-op, restored starting markup in all 72 independent scenarios. In the builder's broader collapsed-text matrix, final DOM, locked-node identity and undo eligibility each matched in 88/88 one-sided cases versus 32/88 on the parent. Far-side identity and undo eligibility matched Chrome in 96/96; complete DOM matched 58/96 because block joining and placement of surrounding content can differ. The separate 48 element/empty-text probes are in the raw ledger. On four physical-key cases starting in an empty text node, the candidate preserves the locked block and the original empty-text caret while Chrome places the caret at host offset zero.

## Two-sided WPT regressions restored

The first candidate protected a block even when editable content existed on both sides. Two occurrence-1 assertions failed on that candidate and passed on the parent, final candidate and signed Chrome:

* `editing/run/delete.html?6001-7000` — `[["delete",""]] "foo<div contenteditable=false>bar</div>[]baz" compare innerHTML`
* `editing/run/forwarddelete.html?6001-7000` — `[["forwarddelete",""]] "foo[]<div contenteditable=false>bar</div>baz" compare innerHTML`

The actual initial host contained `foo<div contenteditable="false">bar</div>baz`. Backspace began at offset zero in `baz`, forward Delete at offset three in `foo`. Both final operations removed the original div, produced `foobaz` and enabled undo, as did Chrome. Final Pyro preserved its parent's caret at offset one in the host, between the two surviving text nodes; Chrome reported offset three in the first text node. The first candidate had left the div and caret untouched. The raw command/keyboard traces are in `two-sided-selection.json.gz`.

## WPT, native guards and provenance

* Primary WPT: 31/31 pages completed with harness status zero. The parent and candidate both passed 15,164/16,112 assertions with identical named outcomes and ten fully passing pages. Chrome passed 15,615/16,114 and reported two names missing from both Pyro runs. The behavioral gain is measured by the direct controls. There is no net WPT gain.
* Independent QA confirmed all 2,000 names in the two affected WPT shards were identical to the parent, with 1,879 passes and both previously lost assertions restored. An additional 54 static guard pages retained all 13,060 named outcomes.
* Native pyro-webapi integration tests for locked boundaries, caret editing, and undo/redo each executed one test and passed. The new native test checks both terminal orientations, two-sided content, node identity and inline removal.
* Immutable parent `ce264703546a80c68ee890ace9d47e0fe0dc5a2c`, binary SHA-256 `c8d73b30f33b44f250afed476cf2b5a6ffddfe6859e23146c593d89c062a0961`. Final runtime `6bbfc6c8a13467e6897ac636a3fa09945c06d79c`, tree `970943c154deeff795e1d481d46ea46dc1fe2c28`, binary SHA-256 `01e283c8d238566c2733ab31ad79127547c2a101b42d650a1116ca98beec42e3`. Both were built from clean source in the private wptiter target with pinned JSC SHA-256 `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029`.
* WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. The primary suite used wptserve, testdriver and one worker. The clean parent and signed Chrome 158.0.8127.37 runs were reused during refinement. The static suite compared the exact Pyro builds with signed Chrome receipts from the preceding matching static profile.

Named rows contain URL, subtest, occurrence, parent status, final status and Chrome status.

```sh
gzip -dc testdriver-named.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing,input-events --server wptserve \
  --testdriver --jobs 1 --tests urls.json --bin "$LABEL=$BIN" --out "$LABEL.json"
cargo test -p pyro-webapi --test integration editing_js::deletion_at_text_edge_preserves_locked_blocks -- --exact
```
