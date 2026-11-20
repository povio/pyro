# Editing paragraph joins

This guard measures all 710 ordinary editing URLs from the pinned non-CSS census. The source topic is 475eb95c506522135d6ca891a79f8c221a22df11 with control 902fe811fbef4ff0f3e39ca1f896de3a275d20b5. Independent review covered editing-host boundaries, nested blocks, editable and visible break elements, and computed display.

Deleting between two adjacent editable paragraphs now retains the moved text's inherited white-space and consumes the first hard line boundary. The measured implementation handles direct text in ordinary paragraph boxes with distinct computed white-space values. Inline ancestry, nested paragraph containers and richer style preservation remain gaps. Both backspace and forward delete use this path; it also applies when the user selects the boundary between paragraphs.

Both LP builds use the CI arm64 JSC 0e0dd7038a7a-6f4649e395e0, library SHA-256 3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029. The runner, four jobs, exact selection and WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603 were matched. The stable reference reuses the WPT-profile Chrome 157.0.8103.80 macOS arm64 records from the frozen census. Browser version, settings and test sources are recorded there. [Stable provenance](https://github.com/povio/pyro/tree/57a9450abdaa792dafae09f6e84d4236fa72a2ea/evidence/round6-noncss-census)

The named rows contain URL, original name, one-based duplicate occurrence, control, candidate and Chrome status. The page rows contain URL followed by each browser's page outcome, harness status and fully-passing flag in that same order. A null names a missing observation. The count keys ending in 0, 1 and 2 correspond to control, candidate and Chrome respectively. The 12 historical inline nowrap losses already pass on the control. They are excluded from new gains.

Use the census runner at topic/round6-reviewed-testharness-census and point the environment variables below to the pinned WPT, its manifest, and the browser to measure.

```sh
bun bench/wpt/run.mjs --kind testharness --wpt "$WPT" --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests all-editing.json \
  --bin "lp=$BIN" --out "$OUT"
```

All 710 URLs were observed on each browser. Named pass counts were 96,887 on control, 98,021 on the candidate and 108,138 on Chrome. There were 1,134 gains and zero losses, missing names or newly registered names between the LP runs. Both LP builds fully passed 139 pages with six harness errors; Chrome fully passed 426 pages with one timeout and one harness error.
