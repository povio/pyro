# Window opening features on auxiliary browsing contexts

The source implements HTML feature tokenization, aliases, integer prefixes, duplicate handling, boolean values for popup/noopener/noreferrer and the legacy popup classification rules. Feature geometry changes the actual initial viewport and reported position. All six replaceable Window bars have distinct BarProp objects. Closing a script-opened top-level window works independently of whether it retains an opener. Named target reuse carries the calling document's referrer and policy; ordinary location assignment keeps its existing behavior.

HTML says a BarProp whose relevant Window has a null browsing context returns true. The signed Chrome and pinned WPT instead expect false after the iframe is removed. The direct probe records this difference. The headless placement policy clamps explicit dimensions to the available screen with a one CSS-pixel minimum, and positions to the screen's available area. Default platform window placement can differ. Target-specific emulation remains local to each target. The direct named navigation tests isolate an opener-only print override. [HTML window features and BarProp](https://html.spec.whatwg.org/multipage/nav-history-apis.html#popup-window-is-requested) · [CSSOM View window setup](https://drafts.csswg.org/cssom-view/#the-features-argument-to-the-open()-method)

## Measurement

Before product edits, the selector froze 29 eligible window testharness URLs covering BarProp, popup decisions, boolean features, geometry/alias handling and tentative split controls. The frozen non-CSS census had 338 Chrome-only names here; the neighboring scoped-registry suite had 372 across a broader lifecycle and SVG length-list names had 56. The source had no BarProp class and ignored feature values. Worker environments, withheld variants, and the rest of non-CSS have no new measurements here. Split-view behavior is outside this implementation.

The signed Chrome stable 158.0.8127.37 WPT projection reused all 29 original pages across 25 four-job shards at ee24e07f6. Twenty-eight pages completed and window-open-windowfeatures-values.html timed out with 87 passing names observed. The signed all-page totals are 391 passes out of 445 observed, 19 strict pages. The projection has no per-selection restart or watchdog totals. LP uses the same WPT and manifest revision, wptserve, testdriver, HTTP/2, fresh page contexts, four jobs, and the no-native-retry runner. Provenance and the raw signed shard hashes are included.

## Commands

Use the sibling cache layout described by run.py and import-reference.py. Build both exact source trees with the pinned CI JSC archive, Cargo profile wptiter inheriting release-fast, disabled LTO, 16 codegen units, incremental compilation, feature render and one Cargo job. Place the executables under base-pyro and pyro.

    python3 import-reference.py
    python3 run.py base
    node probe.cjs base ./base-pyro
    node probe.cjs chrome158
    python3 run.py candidate
    node probe.cjs candidate ./pyro
    python3 check-probe.py
    python3 report.py

named.jsonl.gz lists URL, original name, occurrence (one-based), base, candidate and Chrome statuses. Null means a name was unobserved. pages.json records outcome, harness status and strict-pass flag in the same engine order. A strict pass requires a completed page, OK harness and a nonempty set of all passing named tests. summary.json includes receipt digests and gains by page. The direct probes use ordinary HTTP pages, real auxiliary windows and CDP for the opener-only print emulation. Each run uses its own loopback port, so referrer URLs include run-local ports.

## Protocol and remaining behavior

The opener's Page.windowOpen event reports parsed geometry, chrome flags, userGesture and the original windowName before target creation. Playwright uses this metadata to initialize popup viewport emulation. Missing metadata caused Playwright to overwrite the precursor's requested geometry with the context defaults. An explicit debugger metrics override still changes the surface and clearing it restores the original native dimensions. The fixture captures the real event stream, including zero, negative and noninteger requests and the absence of a new-target event when reusing a named window.

The author-provided name remains observable in a newly created noopener child even though the caller receives null. The browser skips lookup of existing named targets in that case. A native guard creates a same-name iframe and verifies that repeated noopener/noreferrer calls create fresh targets. The positive named-reuse paths retain their opener, carry the calling document's referrer, and keep unrelated opener print emulation out of the new target.

Twenty-six assertions on window-open-popup-behavior.html expect a tab when noopener or noreferrer occurs alongside additional popup-requesting features. The living HTML algorithm removes these two tokens and classifies the remaining features for both “new and unrestricted” and “new with no opener” targets. The raw page ledger retains each browser's observed result for these cases. [HTML window open steps](https://html.spec.whatwg.org/multipage/nav-history-apis.html#window-open-steps)

The separately attributed e9c57a694 precursor gained 242 named passes with zero losses. At 4a519b51 the creation metadata corrected ordinary geometry and the name fix made all sequential popup behavior names observable. At that step 12 negative-size guards lost prior passes: the native 100px minimum disagreed with the headless emulation path's one-pixel minimum. The final source applies the headless minimum to both negative and small positive native sizes. [CSSOM View dimension clamping](https://drafts.csswg.org/cssom-view/#the-features-argument-to-the-open()-method)

## Final outcome

| Engine | Named passes / observed | Strict pages / 29 | Page outcomes |
|---|---:|---:|---|
| LP 1d846ee53 | 63 / 209 | 4 | 19 complete, 10 timeout |
| LP 064ca605d | 396 / 451 | 24 | 28 complete, 1 timeout |
| Chrome stable 158.0.8127.37 | 391 / 445 | 19 | 28 complete, 1 timeout |

The 333 gains include 135 former failures and 198 newly observable passing names. All 63 original passes were retained. Of the 333, Chrome passed 310, failed 17 and had no observation for six. The resulting sets have 33 LP-only and 28 Chrome-only names; the latter comprise 26 popup behavior names and the two BarProp page assertions. LP's timeout on BarProp.window.html and Chrome's timeout on window-open-windowfeatures-values.html remain visible. No LP run restarted or killed a browser. The 12 negative-dimension losses from the preceding build all recovered. The six newly visible Chrome-unobserved names are reported only as LP observations.

The final CDP direct guard navigated a real HTTP child, observed the 310×207 native surface, explicitly changed it to 424×256 and cleared the override back to 310×207. Its creation event reported userGesture false on a Runtime.evaluate call with that flag disabled. A preliminary auxiliary about:blank diagnostic did not complete its page/load wait; this extra fixture uses a bounded DOMContentLoaded wait on an ordinary navigated child. The original WPT selector and comparison are unchanged.

Independent source review cleared 064ca605d, including the normal named-referrer, emulation isolation, parsed target metadata, fresh naming and headless minimum-size corrections.

Independent runtime QA reused the original exact parent and signed stable receipts, repeated the 29 feature URLs and the signed-oracle direct fixture. All 451 named rows and 29 page tuples matched exactly. Direct fields matched after normalizing the ephemeral loopback port and the generated author name. The independent run's candidate receipt digest is in qa/summary.json. Eight focused native guards passed: four parser/protocol tests and four existing or new browser lifecycle tests. Run native-check.sh from the cache layout above to reproduce them.
