# Native contenteditable grapheme deletion

Source `67b603d1cc60fc321543c876e3ad457cb8378826` builds on `85150a49c78ea1e15a5266407556e22f4eb3a111`. Forward Delete uses ICU4X extended grapheme boundaries within the editable Text node. Backspace removes a trailing scalar from ordinary combining text and treats emoji modifier, keycap, flag, variation-selector, tag and ZWJ sequences as units. Programmatic offsets inside a grapheme resolve to its trailing Unicode boundary before the operation. Trusted beforeinput snapshots and the actual character deletion share the UTF-16 interval. A listener may still cancel or move the selection. The ICU segmenter and Unicode properties were already present in the lockfile through the text renderer.

Deletion planning now retains the rendered editing host and identifies atomic and structural boundaries. The mutation consumes the planned interval, preserves hidden and noneditable descendants from unintended text mutation, and retains surrounding paragraphs when joining a hard line. Same-list sibling items can preserve hard breaks under one list marker.

## Matched results

* Input Events, 23 URLs: 2,319 → 2,339 LP passes, +20/−0. Chrome 158 passed 2,476. 11 of LP's new passes were shared with Chrome. Full pages: 1 → 1, Chrome 6.
* Ordinary editing, 710 URLs: 103,840 → 103,962 LP passes, +122/−0. Chrome 158 passed 108,113. 98 new LP passes were shared with Chrome. Full pages: 211 → 211, Chrome 424.
* Each LP pair had identical named inventories. The page rows preserve incomplete/error outcomes. In the core native-keypress set, exact matches with signed stable rose from 6/20 to 20/20, comparing trusted event ranges, resulting text and caret. The scripted-offset set matched 73/78, with five differences on CRLF under collapsing white-space. The separate sibling Text-node probe matched 73/78.
* The ten prior cancel, retarget, host-change and ordinary editing event records match the LP control. Nine complete event records and all ten final DOMs match Chrome. Structural probes also record remaining differences in exact DOM range representations: Chrome can return an element boundary where LP returns the first text offset. Chrome may discard hidden nodes during editing, while LP retains those nodes. The Thai and Devanagari scalar-backspace assertions already passed on the control.

## Reproduction

WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, upstream manifest SHA-256 `34a67512a84d43630f7adbecebd4378df5f410446591015cc6a20a3034e02819`. Both LP binaries use CI JSC `0e0dd7038a7a-6f4649e395e0`, archive SHA-256 `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029`. Control binary SHA-256 `30553f2c3759046eb43d4c4cae47d174db0a9a60e918cc390d5661d64aaab40d`, candidate SHA-256 `0107240485d2267ba7471862fe762acca2bb3f29f054c857fe2415eb89c2c3f6`. The signed Google Chrome Stable 158.0.8127.37 macOS arm64 executable, framework and profile hashes are in chrome-identity.json.

The LP controls and 23-page signed Chrome result come from the preceding exact-base input-target-range measurement. The candidate ran both selections, and signed Chrome 158 ran the entire 710-page selection freshly. The runner uses Playwright 1.65.1, fresh contexts, the pinned WPT-stable profile for Chrome, four jobs and no native-exit retry.

```sh
bun bench/wpt/run-no-native-retry.mjs --wpt "$WPT" --kind testharness --dirs non-css --jobs 4 \
  --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" --manifest-inventory \
  --measure-reviewed-skips --tests "$SELECTION" --bin "$LABEL=$BIN" --out "$OUT"
bun measure.cjs lp "$BIN"
CHROME_BIN="$SIGNED_CHROME" bun measure.cjs chrome158 chrome
```

Use urls.json and editing-urls.json for the two official selections. Named rows retain URL, original name, one-based occurrence and control/candidate/Chrome status. Page rows retain terminal status, harness status and fully passing outcome for each engine. Null denotes an unreported observation. Two native Rust tests cover cluster/scalar and UTF-16 caret rules. Independent review examined Unicode boundaries, locked widgets, hidden siblings and visibility overrides, native target/mutation agreement, placeholder BRs, list ownership and paragraph/formatting preservation.
