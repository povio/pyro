# Joining preformatted text and neighboring blocks

When Backspace, forward Delete or a boundary selection joins a preformatted block with a div, paragraph or blockquote, Pyro now moves the first rendered line, consumes its separator and leaves following lines in their original block. The edit preserves the source's authored text formatting. The pre element's default monospace font and whitespace apply to the pre element itself.

Across 206 testdriver pages, the exact parent passed 8,972/10,691 named assertions, the final candidate passed 9,359, and signed Chrome 158 passed 10,170. All 206 pages completed with harness status 0 on each binary. Fully passing pages increased from 81 to 90. The nine target pages account for all 387 gains, with zero lost, added or missing names. Signed Chrome passed every gained assertion. Fifty-four static guard pages retained all 13,060 parent outcomes.

| Method | Blockquote | Div | Paragraph |
|---|---:|---:|---:|
| Backspace | 3 → 46/46 | 3 → 46/46 | 3 → 44/44 |
| Forward Delete | 4 → 52/52 | 4 → 52/52 | 4 → 50/50 |
| Selected boundary | 6 → 46/46 | 6 → 46/46 | 6 → 44/44 |

The parent passed 39/426 across these nine variants. The candidate and signed Chrome each passed 426/426.

## Source formatting and undo

Independent supplemental QA measured the resulting DOM, selection and computed styles. The table shows the moved text's color, weight and whitespace after each join.

| Source case | Method | Parent d1cd467fd | Held d9398ba8f | Final f9f88ea05 | Signed Chrome 158 |
|---|---|---|---|---|---|
| Green/700 inherited from the host; destination blue/400 | Forward Delete | green / 700 / pre | blue / 400 / normal | green / 700 / normal | green / 700 / normal |
| Same inherited style | Selected boundary | blue / 400 / normal | blue / 400 / normal | green / 700 / normal | green / 700 / normal |
| Stylesheet red !important; inline blue | Forward Delete | red / 400 / pre | red / 400 / normal | red / 400 / normal | blue / 400 / normal |
| Same cascade | Selected boundary | black / 400 / normal | red / 400 / normal | red / 400 / normal | blue / 400 / normal |

Before the cascade probe's join, the source computed to red on both engines. The candidate preserved that visible red; Chrome copied the inline blue declaration. The final inherited-green result matched Chrome in DOM, selection and computed style for both methods. All eight CSS-wide keyword checks and four inline/variable checks matched Chrome. Undo restored the candidate's exact original DOM and selection in all 22 supplemental cases.

All six locked-element and distinct-host probes produced the same results as the parent. A pre-existing difference remains when forward-deleting from an adjacent paragraph into a noneditable pre: Pyro removes the locked block, while signed Chrome preserves it. Other locked-inline and selected-range differences are recorded in the traces. The two native integration filters dom_js::contenteditable_typing_deleting_and_caret_keys_match_chromium and editing_js::undo_and_redo_restore_text_controls_and_hosts each ran one test and passed.

## Provenance and reproduction

| Artifact | Source | SHA-256 |
|---|---|---|
| Exact parent | d1cd467fd659e12f7b0bbecd6413e7e66418056d | 46411d377277c8a02cd0ea3b41c4cee021b325ff5af614145ac8f1c3cdba8c98 |
| Held style revision | d9398ba8f7492783a79e877fa36b33618778add1 | 352135fdf543265ad6298d6b920338c75d9e8b03cae1839a47f95aa311aed750 |
| Final candidate | f9f88ea05bc5a22972e9869662b437014ac1fcea | 9bbb6aad0dd818eeb06401f2b2a2e42a7c3685aae08c19543e3619023dc6ef20 |

The clean final source tree was `e50105fa28a6d0d206e6de26b300f40958c537e3`. The private wptiter render build used one Cargo job and the pinned JSC archive `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029`.

WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. The primary comparison used wptserve, testdriver and one worker. The archived signed Chrome 158.0.8127.37 executable ran through the launch-158 wrapper under the wpt-stable profile. The clean parent and signed Chrome receipts from the initial comparison were reused for the final comparison. Both exact Pyro builds ran the static guard pages; their signed Chrome reference was reused from the earlier matching static profile. The independent read-only audit matched every gained URL, subtest name and occurrence and checked every guard outcome.

The compressed named rows contain URL, subtest, occurrence, parent status, final status and signed Chrome status. The supplementary JSON and compressed traces include the parent, held and final outcomes and the signed Chrome observations.

```sh
gzip -dc testdriver-named.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing,input-events --server wptserve \
  --testdriver --jobs 1 --tests urls.json --bin "$LABEL=$BIN" --out "$LABEL.json"

gzip -dc static-guards.jsonl.gz | jq -sc 'map(.[0]) | unique' > guards.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing,selection,dom/ranges \
  --jobs 1 --tests guards.json --bin "$LABEL=$BIN" --out "$LABEL-guards.json"
```
