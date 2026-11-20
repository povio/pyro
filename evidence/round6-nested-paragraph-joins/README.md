# Joining a leading nested editing paragraph

Source `33ea5eaf3e5870addf7f3d6ad3249b1d4d380523` builds on `aa0268b479a6b463da16910b4b743cf38901d962`. A deletion at the outer paragraph boundary now joins the first rendered line of the leading nested block. Backspace can reach the same boundary from inside the first child. Range extraction retains formatting elements and the existing editing typography, leaves later lines and unrelated siblings in place, and removes only the emptied ancestor chain. The implementation is bounded to leading editable HTML paragraph boxes with different source and destination white-space modes.

The pinned WPT revision is `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Both LP builds ran all 710 ordinary editing URLs with the same four-job manifest runner, wptserve, HTTP/2, and testdriver. LP increased from 102,109 to 103,693 passing named assertions and from 151 to 211 fully passing URLs. All 1,584 gains passed the frozen pinned local Chrome Stable 157.0.8103.80 macOS arm64 reference. No named or page pass was lost. The LP name inventories matched. Chrome passed 108,138 named cases and 426 full pages. LP had six harness errors on each build; Chrome had one timeout and one harness error. The paired rows preserve these results explicitly.

The paragraph-to-paragraph variants gained 918 names and retain 216 Chrome-only names. Anonymous-left-line / right-paragraph variants gained 666 and have no remaining Chrome-only names here. The separate anonymous-right-line family retains 468 Chrome-only cases.

The exact sources passed independent review. The final private build passed eight direct DOM and undo checks, compared with two on the baseline and six in Chrome. The extra two controls require retaining an existing inline node and restoring a contextual selector's formatting when its text moves.

## Reproduction

Build the control and source commits with the CI arm64 JavaScriptCore `0e0dd7038a7a-6f4649e395e0`, library SHA-256 `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029`. Run each binary with the reviewed manifest runner:

```sh
bun bench/wpt/run.mjs --wpt "$WPT" --kind testharness --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests selection.json \
  --bin "lp=$BIN" --out "$OUT"
```

Control executable SHA-256 `9623217e05de0c6866256ce4f855edabf2ca810eb0d6a3ae51a93fd5ba50f7a8`. Candidate executable SHA-256 `bd3c35894c6b9f165edad4e77d200b14013225a37bb77fbb19f52fea0fd6e2de`.

The compressed named rows contain the URL, original name, one-based occurrence, measured base, measured candidate, and frozen local stable status. Null denotes an unreported result. Page rows retain each browser's terminal status, harness status and fully-passing flag in the same order. Summary suffixes 0, 1, 2 correspond to control LP, candidate LP, and local Chrome.
