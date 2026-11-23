# Text insertion at HTML void elements

On WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, the void-element WPT rose from 0/48 to 41/48. Signed Chrome 158.0.8127.37 passed 40/48. The two implementations share 37 passes, with four Pyro-only and three Chrome-only. Across 54 WPT URLs the final candidate gained 41 and lost zero among 13,060 matched named checks.

The parent topic `e3b4e7e40` differs only in evidence from the tested base binary built at `d20cdcc4e` (SHA-256 `663f057002859a28d49196d9a6b07cbaf23304c75172d778d1942c135dbcb5a2`). Runtime candidate `f23c31dde8e07f386c7b80ce197cc6d8129a7001` has binary SHA-256 `c5132ef675c396c8cd07fa31055e0f3b4919702e6917d1ac26fc3bc305a342e2`. Both Pyro versions and signed Chrome used the static WPT server with one worker. The 11-page base and Chrome comparisons were reused from the first candidate. The other 43 URLs were run on all three binaries after the final edit.

## Six gains over the first candidate

Each of these assertions failed at `d9d8eeffe`, passed at `f23c31dde` and passed in the archived signed Chrome 158 binary on the matched static profile.

- Inserting text when selection is collapsed in &lt;br&gt; which follows a text node and an empty &lt;span&gt; element
- Inserting text when selection is collapsed in &lt;embed&gt; which follows a text node and an empty &lt;span&gt; element
- Inserting text when selection is collapsed in &lt;hr&gt; which follows a non-empty &lt;span&gt; element
- Inserting text when selection is collapsed in &lt;img&gt; which follows a text node and an empty &lt;span&gt; element
- Inserting text when selection is collapsed in &lt;input&gt; which follows a text node and an empty &lt;span&gt; element
- Inserting text when selection is collapsed in &lt;wbr&gt; which follows a text node and an empty &lt;span&gt; element

## Preceding-wrapper probes

These three QA-authored probes are additional to WPT. Each starts with a focused contenteditable host containing `<span style="…">original</span><img>`, collapses the selection at `(img, 0)`, then calls `execCommand("insertText", false, "X")`. All commands returned true. The original span remained attached in every case. The original base inserted a text child inside the image, which its HTML serialization omits. Both candidates and Chrome left the image childless.

| Preceding span style | Final Pyro DOM and collapsed caret | Signed Chrome 158 DOM and collapsed caret |
|---|---|---|
| `display:none` | `<span>original</span>X<img>`, sibling X offset 1 | `X<span>original</span><img>`, leading X offset 1 |
| `visibility:hidden` | `<span>original</span>X<img>`, sibling X offset 1 | `X<span></span><img>`, leading X offset 1 |
| `display:inline-block` | `<span>original</span>X<img>`, sibling X offset 1 | `<span>originalX</span><img>`, span text offset 9 |

The table omits the unchanged style attribute for readability. The first candidate appended X to the text inside each span. The final candidate preserves the neighboring text and places the caret outside that formatting box. The six official cases above cover different markup. HTML defines void elements, and the editing draft describes insertion around existing text. We found no requirement establishing the probe's exact placement for these three styled neighbors. Preserving the neighbor is an implementation choice and differs from observed Chrome behavior. [HTML void elements](https://html.spec.whatwg.org/multipage/syntax.html#void-elements) · [Editing draft](https://w3c.github.io/editing/docs/execCommand/#the-inserttext-command)

## Verification

Independent QA reproduced +41/−0 across the 11 primary URLs and 10/10 supplemental controls on the candidate. Signed Chrome passed 7/10 supplemental controls. The native guard `dom_js::contenteditable_typing_deleting_and_caret_keys_match_chromium` passed with exactly one test executed and 654 filtered out.

`neighbor-traces.json` records the before and after DOM, image child count, original span identity and text, return value, and both caret endpoints for all four binaries. `neighbors.html` is the observation probe. The named WPT rows contain URL, subtest name, occurrence, parent status, candidate status and signed Chrome 158 status.

```sh
gzip -dc named.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing,selection,dom/ranges --tests urls.json \
  --jobs 1 --bin "$LABEL=$BIN" --out "$LABEL.json"
cargo test -p pyro-webapi --test integration \
  dom_js::contenteditable_typing_deleting_and_caret_keys_match_chromium -- --exact
```
