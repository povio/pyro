# Joining an editable paragraph to an anonymous line

Source `f803b29722e45cb1264a93ada2014db106f56746` builds on `b26c9619e2b8f9dde519a7a714d91829d051174a`. Backspace, forward delete, and a selected paragraph boundary now move the first editable inline line into the preceding paragraph. The join consumes its BR or preserved newline, keeps the following line outside the paragraph, and restores the moved text’s inherited white-space and core font properties where their resolved values change. The bounded implementation handles editable text and supported inline formatting elements with a different source and destination white-space mode.

All 710 ordinary editing URLs ran on both exact-source LP builds. Passing named assertions increased from 101,150 to 102,109. All 959 gains also passed local Chrome stable. LP gained 12 fully passing pages (139 to 151), with zero lost passes or names and no new names. Both LP runs completed 704 pages with an OK harness and reported six harness errors. Chrome passed 108,138 named assertions and 426 whole pages. It recorded one timeout and one harness error. The left-paragraph/right-line family still has 468 Chrome-only named assertions.

The source passed independent read-only review and 10 direct DOM probes on the measured binary. Chrome passed seven of those probes. Three direct checks additionally require fully selected nodes to keep their identity, which Chrome replaced. The official case comparisons use the original WPT names and expectations.

## Reproduction

Pinned WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, local Chrome stable 157.0.8103.80 macOS arm64, CI JSC `0e0dd7038a7a-6f4649e395e0`. The JSC static archive has SHA-256 `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029`. Chrome’s named and page records come from the frozen pinned WPT-profile census. Each LP run used the same four-job runner and the same selection.

```sh
bun bench/wpt/run.mjs --wpt "$WPT" --kind testharness --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests selection.json \
  --bin "lp=$BIN" --out "$OUT"
```

The compressed named rows contain URL, original name, one-based occurrence, measured base status, measured candidate status and frozen local Chrome status. Null means unreported. The page rows retain page and harness status and the fully-passing flag for all three browsers in the same order. Summary suffixes 0, 1 and 2 correspond to base LP, candidate LP and local Chrome.

Base binary SHA-256 `a61a7e773d7c7ccda039cb2c76e9c11f95905b5befa2fa7ffa7a227974a4c080`. Candidate binary SHA-256 `26447a84f35b8950350d50955b5161cb4fc4511abe883598b4afbec4d9f18961`.
