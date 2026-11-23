# Invalid foreColor return values, typing style and selection-only undo

Tested source `8d85f4a722239d9aa4a16721ee96c1e7f3aa21a3`, tree `42db39ede3a082f8f998b4febb95a51d893d7db6`, executable SHA-256 `0ef7f19f9200eae37ded4b1a1728c2dc70e65b29dede2e415c487cd5bd6a4130`. Exact parent `ce41434b10874a6a4e6537b02390ac3cd47b357b` required a build for changed navigator, secure-context and Web Audio code. First staging compile stopped before producing a binary after an existing snapshot caller required its original signature. The three runnable candidate builds have separate provenance receipts. Signed comparator was Chrome 158.0.8127.37.

Once a rich host accepts foreColor, a color the editing parser cannot resolve returns true. A noncollapsed selection dispatches one input event (empty inputType, null data) and records a selection-only undo step. Undo/redo preserve the DOM and the original text objects. Both endpoints must still resolve within the host before restoration. Otherwise the current selection stays in place. At a collapsed caret the command clears pending typing overrides, retaining inherited/author styling, with no input event or new history entry. Unsupported hosts retain their existing command handling.

## WPT and direct controls

Of 33,316 matched assertions, all five frozen command-return cases recovered. One former pinned pass, the currentColor return, now fails: Chrome and candidate return true, while the pinned test expects false. Ten newly observable input-event assertions (five Pass, five Fail) match signed status. No names disappeared relative to the parent. These ten are counted separately from gains on matched rows. [Changed occurrences](changed-names.json)

Independent command returns match on all 284 main, exact, markup, external, pending and glyph cases. Initial event traces match in 144/160 mixed main cases, including all 80 instances of the five target strings. In the 50 exact cases all undo results, restored markup, endpoints and events match. All 12 external cases preserve the script-modified DOM on undo and redo. All 24 invalid pending-glyph controls reproduce the signed style on subsequent insertion. Twelve valid-color glyph controls retain their prior behavior, with full glyph-vector agreement in 11. All 16 pending foreground-plus-other-style fixtures reproduce the typed markup. Ordinary insertText and history of actual typing still emit additional beforeinput events versus Chrome.

In four external replacement cases, Chrome already places the selection at the host boundary when innerHTML changes; Pyro already has no resolvable endpoints. This predates undo. The corrected history keeps that selection and the replacement DOM. Queried value/enabled consequently differ after undo and redo in those four. Explicit removeAllRanges leaves selection empty in Chrome through undo and redo. A valid moved caret in the intended host or another host remains in place when original endpoints are stale. No unconditional host-zero fallback was added.

## Separate color parsing differences

Matching the return does not guarantee matching the applied color. Bare `000000fff` yields `#000fff` in Chrome, including the standards-document probe. Pyro currently treats it as unparseable. The other four frozen return fixtures keep their original color/markup in the exact selected-text probe. CurrentColor and CSS-wide inherit/initial can also change markup or color in Chrome on authored FONT/SPAN targets. The standards and keyword receipts preserve exact HTML, queried values, events and selection for this parser follow-up.

Both native selection/history tests passed. Final candidate runs retained all 10,092 command multitest and 13,060 static neighboring statuses from the exact parent.
