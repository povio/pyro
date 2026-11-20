# Deletion within a visible whitespace run

Source `b8a21852262b1c679e949388681e088b32bc501d` builds on `33ea5eaf3e5870addf7f3d6ad3249b1d4d380523`. When deleting one space from a visible run inside editable HTML, LP now repairs two cases: an additional surviving space would collapse, or removal of the sole ASCII space would eliminate a soft-wrap opportunity between words. It uses the editing implementation’s existing canonical space sequences. Equivalent surviving spellings, noncollapsing white-space modes, ambiguous formatting boundaries and previously collapsed spaces stay outside this repair.

A direct control measures the rendered result of deleting the NBSP in `a␠NBSP␠b`. The base lost two space widths for one deletion. The candidate retains the two surviving widths, the text node, UTF-16 caret and undo behavior. The direct batch passed 19/19 on the candidate, 11/19 on the base and 16/19 in Chrome. The direct checks include inherited wrapping behavior, noneditable boundaries and offsets inside a surrogate pair. The WPT chrome-compat pages document historical Chrome behavior. Their exact serializations are comparison evidence, rather than normative requirements for all whitespace runs.

Across 710 ordinary editing URLs, the candidate gained 110 named passes with zero lost, missing or newly reported names. All 110 also pass local Chrome stable. Four pages gained: the two chrome-compat deletion pages gained 46 each and the two editing/run deletion pages gained 9 each. LP now passes 103,803 named assertions and 211 complete pages; the matched local Chrome Stable 157.0.8103.80 macOS arm64 reference passes 108,138 and 426. Both LP runs reported six harness errors. Chrome reported one timeout and one harness error. The two primary deletion pages retain 806 Chrome-only assertions.

## Reproduction and provenance

Use the tested source with the exact CI arm64 JavaScriptCore `0e0dd7038a7a-6f4649e395e0`, static library SHA-256 `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029`. Control binary SHA-256 `bd3c35894c6b9f165edad4e77d200b14013225a37bb77fbb19f52fea0fd6e2de` and candidate binary SHA-256 `8612c06bf10d690c18d7df9785a1ddc8cfc6f35275413ad99ffb7ae8f478185d`. Reused the preceding topic’s measured exact-base control and frozen local stable records at WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; the candidate used the same checked manifest runner, URL list, server, four jobs and testdriver settings. Source review independently checked UTF-16 boundaries, visibility, protected inline boundaries and nowrap handling.

```sh
bun bench/wpt/run.mjs --wpt "$WPT" --kind testharness --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests selection.json \
  --bin "lp=$BIN" --out "$OUT"
```

Named rows contain URL, original name, one-based duplicate occurrence, control LP status, candidate LP status and local Chrome status. Null denotes an unreported result. The page rows retain terminal status, harness status and a fully-passing flag for the same three browsers; summary suffixes 0/1/2 use that order.
