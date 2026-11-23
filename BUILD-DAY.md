# Round 7 batch-1000: build-day package

Documentation only. **Do not build until the user authorizes it** (standing ZERO-compile order).

- Batch branch: `origin/integration/round7-batch-1000` at `9317be985` (when this file was generated).
- Ledger: `origin/ledger/round7-fix-queue` (`ledger.jsonl`, `estimates.json`).
- Expected, unverified: MacBook in-scope window pages implemented-unbuilt **1225**; Studio **504**; MacBook worker variants (separate column) 220.
- Every figure is an estimate from source reading. Tentative pages are counted only where the owner gave a figure or the coordinator ruled.

## 1. JSC archive

Build the JSC archive through patch **0041**:

| Patch | What | Batch commit |
|---|---|---|
| 0038 | inline module | (pre-batch archive) |
| 0039 | module-fetch-referrer | 2dbda236a |
| 0040 | deferred-work (async wasm, worker loops via c6295b18e) | a8c9d7350 |
| 0041 | named-properties-chain | f47805459 + fix 92579f9cd |

- The held HTMLCollection-delete patch (4f4684042, also numbered 0039) is **not** on this branch. No renumbering is needed for this build.
- If global-lookup or realm-startup regressions appear, drop 0041 first.

## 2. Pre-build branch hygiene

- The evidence commit 5c95c55d1 (18 files under `evidence/round7-inline-module-base-url/`) is removed by **3047718ed**, which is an ancestor of the tip. Verified: 0 files under that path at the tip.
- 0206f4243 compile break fixed by ea7daf4c4 (integrator; rustfmt parses frames.rs at tip).
- Run `git show --stat` and confirm the tip matches what is being built.

## 3. Security smoke tests (mandatory)

- RELEASE BLOCKER — MITIGATED at cd4ef7dea (blanket taint after every drawElementImage; coordinator 2026-11-22): drawElementImage (94fe1804b/9259003cd) leaks cross-origin pixels and :visited colours into an untainted canvas. HTML-in-Canvas worker …3a492bc9 is fixing it (filter non-origin-clean content in paint_element_subtree, taint fallback). No build ships without the fix verified (privacy/* pages + cross-origin getImageData smoke). If not landed by build day: taint the canvas after every drawElementImage. Privacy pages (~96) stay UNCREDITED until the worker's filtering commit replaces the blanket taint. UPDATE: b8b222900 (conditional taint on possible non-origin-clean content) replaced the blanket taint (016c83b79); verify the cross-origin getImageData smoke still throws and same-origin-only scenes stay readable. DECISION: conditional taint (b8b222900 + 016c83b79) accepted by reviewer B; privacy pages may now be credited per the HTML-in-Canvas owner's list once reported.
- LIKELY CLOSED by d28cf3778 (on the batch, later than 2cd3f80e0): classification now uses the response opaque flag (redirects, SW opaque answers, other-origin final URL) instead of the request URL. The same-origin→cross-origin redirect leak the canvas reviewer found at 2cd3f80e0 should be closed; REQUIRED smoke on build day: 302 to cross-origin PNG as <img> and CSS background → getImageData must not return the pixels. Stays a release gate until the smoke passes.
- SVG leak gate: reviewer B confirms 44120fbc2 closes it (cross-origin SVG <img> omitted, redirected SVG CSS background → SecurityError); keep smoke (b)+(c) + inline <svg> SecurityError as required build-day checks
- 15. (Studio) F01 1437a682e: patch-0037 credentials/startup tests, worker module fetch retry/blob/MIME, dynamic import credentials; runtime checks owed: dynamic import cookies omit/include, cross-origin credentialed CORS, module-phases capability on both engines, Sec-Fetch-Dest vs Chrome
- R-HTML-B-3 c9216028f: a deferred script may call document.open (remaining deferred scripts run in the opened document; DOMContentLoaded before close()) — check document-open pages + real-site smoke
- acf404f96 lazy-load capture parity: PDFs now wait up to 10 s for ANY pending image load (not only lazy); screenshots at load may lay out a just-approved lazy img without natural size (no width/height); lazy video/audio not started for captures — check Shippy/PDF smoke + capture timing
- b35d4961e: every DOMTokenList now reads/writes the null-namespace attribute — check classList/relList/sandbox behaviour (dom/nodes Element-classlist*, DOMTokenList-*) and real-site smoke
- drawElementImage privacy (d28cf3778, reviewed): run draw-element-image/privacy/* in full + the expected-results matrix on R-HTML-9f; external SVG filter flood colour must not be readable
- security smoke 04cc49dcf: captured ElementImage of iframe/cross-origin video content drawn into a second canvas must taint (getImageData SecurityError)
- security smoke 60b40b21e: cross-origin no-cors <img> loaded before any canvas use → createImageBitmap/drawImage → getImageData must throw SecurityError
- security smoke c289d09c4: bitmaprenderer canvas holding a cross-origin bitmap must keep toDataURL and downstream getImageData throwing SecurityError
- 7c34591e9: a cross-origin frame with allow="deferred-fetch" now shares the parent 512 KiB fetchLater pool instead of a reserved 64 KiB
- security smoke 62d68f95f: drawElementImage of content inside a visited link must taint (getImageData SecurityError)
- Studio CSP core v4 (a7e2f883e + 457992246): Pyro now enforces CSP — run all content-security-policy/, trusted-types/, import-maps/csp/, and the no-CSP pages (fetch/api/basic, xhr, eventsource, websockets) to catch over-blocking; 2 expected new failures match Chrome (unsafe-eval eval-allowed-by-hash*)
- 93cf2f04c (…06168c11) mixed-content blocking: REVERTED by 0729192ae (integrator), credit 0. Not in this build; smoke only that https→http fetches are not blocked.
- SC-CSP-reporting (61148b585, 1d7f94410): cargo test -p pyro-webapi (csp::, csp_report::, trusted_types), -p pyro-dom nonce, -p pyro-browser (child_frames, preload_scanner); WPT --server wptserve content-security-policy/{securitypolicyviolation,reporting,reporting-api}/; no-report guards: link-preload-from-header-report-only-nonce, object-src-*-allowed, style-src-multiple-policies-multiple-hashing-algorithms, plugin-types-does-not-block-pdf-embed, reporting-api-doesnt-send-reports-without-violation, TT check-report-*-create-policy; full content-security-policy/ + trusted-types/.
- SC-CSP-wasm-gate dfbe98f94: 16 blocks-wasm pages (12 pass, 4 .serviceworker fail); allows-wasm pages stay passing; wasm/jsapi/idlharness.any.js, wasm/jsapi/constructor/*, wasm/jsapi/module/constructor.any.js; content-security-policy/unsafe-eval/*, trusted-types/*eval*.
- drawElementImage cross-origin leak (9259003cd/94fe1804b): CLOSED by the taint series per reviewer B (no remaining path at a320eb224 after 44120fbc2; approved d28cf3778/eeb1ad1df, 62d68f95f). Security smokes stay on the build checklist: cross-origin <img>, redirected raster, cross-origin SVG <img>, redirected SVG background, visited-ancestor → blank or SecurityError.
- SC-CSP-violation-location 6659fee08 + 5187a2bb0: inline scripts now report document line numbers in error events (Chrome). cargo test -p pyro-dom -p pyro-webapi -p pyro-browser -p pyro-cdp (parser_scripts_start_at_their_start_tags_line, csp::, error_events, tier3_g2 console_calls_carry_previews_and_stacks); WPT securitypolicyviolation/, html/webappapis/scripting/processing-model-2/ (refresh baseline), event-handler-processing-algorithm-error/, trusted-types/, CSP reporting-api/. Check CDP console/exception line numbers in e2e.
- SVG leak smokes (b) cross-origin SVG <img> and (c) same-origin→cross-origin redirected SVG CSS background: CLOSED at source level at tip bf465986c (canvas reviewer …d0f2a87d): draw_svg returns early on opaque svg key (render.rs:7268); background Url layer skips Privacy::opaque (background.rs:706), opaque flag from response tainting / final-URL origin (net.rs bytes_from). Gate cleared pending build smokes: (b) SVG omitted, canvas readable; (c) background omitted, readable; privacy/svg/*-ignored WPTs should pass.
- History (…ad4efc0e): 33413ebc8 history getters throw SecurityError on a removed iframe's document; 44b6364ba Location of a removed iframe ignores navigation; 856d4b777 about:blank?query/#frag loads blank; ab1639abf scrollRestoration per entry — check Playwright goBack/goForward/reload and iframe e2e.
- R-HTML-9f privacy (14 tentative): if ANY privacy page fails at build, review the whole 9f credit (coordinator).
- b061fffe0: form.reset() moves a text input cursor to the end when the value changes (was 0). ecf4666d2: DataTransfer store roots script-added Files (Persistent) — check memory/leaks in file-upload e2e.

## 4. Watchlist (behaviour changes and regression risks)

- CLOSED (coordinator 2026-11-22): screenshot lazy-wait release blocker closed by d4afe87e2 + f630e4c3d; still verify the full-page CDP/MCP lazy-image screenshot on build day
- 1. Q03 fa918da33/309b1d3a7: rt no longer isolates (check css-ruby ruby-bidi-001..004 reftests); overflow:clip on text-like inputs (check Shippy/form reftests)
- 2. C02 9f2703e69 shadows: every shadowed draw rasterises twice (perf)
- 3. F02 grid regression: fixed by F02f 18c7e8ff2, superseded by review fix 41ddc247c ('grid' makes no focusgroup); verify the 3 grid-navigation pages still pass
- 4. R-ENC-3 8182ff6fa BOM change: html5ever discard_bom off; verify script-pause feeds
- 5. R-ENC-5 45204abf7 pending fetch/XHR cap 64->1024 and R-ENC-1 3208400de frame cap 128->1000 (memory)
- 6. Remove evidence commit 5c95c55d1 (18 files under evidence/round7-inline-module-base-url/) before the build
- 7. R-ENC-2 windows-1252 default is held on a side branch, NOT on the batch
- 8. (ledger owner) F09b 216b53f2d: scripts now wait for link @imports; all 16 execution-timing/106-* and 150-* pages already pass on RC5, so watch them for regressions (0 credited)
- 9. (ledger owner) R-FETCH-3 fa832c81f: XHR shares the raised body limits; R-FETCH-13 3c98fcd83: a subresource with an unknown frame origin is blocked by same-origin/same-site CORP
- 10. (ledger owner) P07 396afe524: new crates ruzstd 0.9.1 + twox-hash 2.1.5 with hand-written Cargo.lock entries; Accept-Encoding now includes zstd
- 11. (ledger owner) C04 fd9e97f2d: Stylo API not compile-checked; calc() font sizes now rejected
- 12. (ledger owner) Batch JSC = the 0038 archive (POLICY-jsc-0038); 0039 excluded
- 13. (ledger owner) F02 regression-watch: 29 pages LP already passes (on rows F02a/b/c/F02f)
- 14. (Studio) F01 module SharedWorker: if it lands, the two patch-0037 SharedWorker module tests (link failure vs evaluation throw; startup/credentials) are owed at the single batch validation
- 16. (Studio) SC-Q09 4d8b32f52: no-frame scroll/hidden state, stale layouts, deep-tree work, sizing WPT and reftests
- 17. C05e 2d62e0765: synthesized small caps even when the font has smcp; check the fontVariantCaps reftests
- 18. JSC: the batch needs the archive through 0039-module-fetch-referrer (2dbda236a), not the 0038 archive
- 19. 88224f9d8 reverted focusgroup hunks in input.rs (it broke compilation: 41ddc247c calls focusgroup_top_layer); restored by 8f0a9f993 (integrator confirmed). Verify the build compiles and F02 pages run
- 20. Per-commit build checklists from Reviewer A (…660de521) are on rows P06, OWN-mse-mp4, F02a, Q03, C04, R-FETCH-9b, OWN-studio-margin-trim, F08c, F04c, F04b, Q02 (review.build_checklist). Run P06's bea87c3eb test first.
- 21. Canvas per-commit build checklists from the canvas source reviewer (…d0f2a87d) are on rows C01-C15, C05a-e, CG1-4, CM1/2/4 (review.build_checklist). Run element, offscreen and worker variants. Unit: pyro-dom canvas_color, pyro-canvas, pyro-webapi canvas_path
- 22. R-HTML-23 a39d0a6b3: loaded images now start native drags on press-and-move; check Playwright/e2e drag flows on images
- R-HTML-9a 69c012965: run_paint_step per-frame journal/ancestor walks on any page that ever had a <canvas>; time a canvas-heavy animated page; no-canvas pages must return early on canvas_seen — UPDATED by ec8fb83dc: gated on drawable_canvas_seen (content=drawable ever set), so ordinary canvas pages skip the step; verify that
- F09b 216b53f2d: the 8 execution-timing *-noimport pages are regression guards (scripts now wait for <link> @imports)
- R-HTML-20 0d5d3c7bc: Resource gained last_modified; any Resource literal added later without it breaks the build (check before the first compile)
- Joint session history f10b98964 + ff740683f: run guard list ~/Library/Caches/lightningpanda/round7-local/history-nav-guards/iframe-or-traversal.txt (181 pages) — risks: two same-document back() in one task traverse once; navigated-away document drops its iframes' entries from history.length; a never-finishing navigation fetch keeps the old document from firing load
- F03p1 edb28212b: CSS.supports('appearance','base-select') is true while the picker is not rendered (F03p2 deferred) — if p2 is not done in the build window, turn the pref off before any release
- F04h 147f68d86: TLA import() in a module service worker still allowed (spec gap, not a regression)
- R-HTML-9c f219b1c51 (vendored blitz drawable-canvas layout): 20 guard pages passing in census c3372ad21 — html/canvas/element/manual/draw-element-image/content-drawable-no-hit-test.tentative.html; html/canvas/element/manual/draw-element-image/onpaint-zindex.tentative.html; html/canvas/element/manual/draw-element-image/hit-test/elementsFromPoint-no-content-drawable.tentative.html; html/semantics/embedded-content/the-canvas-element/canvas-descendants-focusability-001.html; …-002.html; …-004.tentative.html; …-005.html; html/semantics/embedded-content/the-canvas-element/imagedata.html; …/historical.html; html/canvas/element/the-canvas-state/2d.state.saverestore.fillStyle.html; …lineWidth.html; html/canvas/element/manual/imagebitmap/createImageBitmap-premultiplyAlpha.html; html/semantics/embedded-content/the-img-element/relevant-mutations-lazy.html; …/dimension-getter-transform.html; html/semantics/embedded-content/the-video-element/intrinsic_sizes.htm; html/rendering/replaced-elements/svg-inline-sizing/svg-inline.html; …/svg-embedded-sizing/svg-in-img-auto.html; …/svg-in-iframe-auto.html; …/svg-in-object-auto.html; html/rendering/replaced-elements/attributes-for-embedded-content-and-images/img-aspect-ratio-alt-text.html
- 2b99c5fcc lazy img from first layout: verify no forced layout during parsing (v0.0.44 render-sync NO-GO lesson), lazy images no longer block load, full-page screenshots approve all lazy content
- F03p2 d7ee74c1b (blind Blitz picker layout): verify the 6 picker pages + F03p3 tentative 4; the base-select CSS.supports pref item now depends on whether p2 actually renders
- cc806a11f: Playwright drop-in disables the navigation rate limit via PYRO_NAVIGATION_RATE_LIMIT (TS only syntax-checked with the Bun transpiler — typecheck + run the drop-in e2e on build day)
- RELEASE DECISION 0facc5a71: HTMLInstallElement, InstallResultEvent, oninstallresult and usermedia/camera/microphone elements are exposed ungated (beyond Chrome stable; no runtime experimental switch) — decide before release whether to keep
- CONDITIONAL 69c012965: CanvasPaintEvent/requestPaint/onpaint ship together with drawElementImage (94fe1804b, on the batch) — no unregister needed while 94fe1804b stays; if 94fe1804b is dropped or reverted, unregister those three before release
- a799ad8bb: Playwright goBack/goForward now traverse the joint history (iframe entries count) — run CDP history tests + Playwright drop-in navigation e2e
- F03p2 d7ee74c1b guards: 10 select reftests/rendering + 6 testharness pages (list on F03p2) + manual stale-option-box hit test
- b35d4961e guards: dom/nodes/Element-classlist.html (1420), dom/nodes/attributes.html (67), dom/nodes/MutationObserver-attributes.html (42), dom/lists/DOMTokenList-{value,stringifier,iteration,Iterable}.html, dom/nodes/{Element,Document}-getElementsByClassName.html, dom/nodes/ParentNode-querySelectorAll-svg-class.html, dom/nodes/getElementsByClassName-10.xml, -11.xml; watch dom/lists/DOMTokenList-coverage-for-attributes.html (175, 1 known failure) not worse
- acf404f96/2b99c5fcc: full-page CDP + MCP screenshot of below-fold loading=lazy images must show painted images (after the screenshot parking fix)
- RELEASE NOTES (coordinator): the history/navigation rate limit is OFF under MCP (launches via chromium.launch), matching Playwright's Chromium; cc806a11f's commit message is inaccurate on this point
- Studio selectors user-action (12 commits incl. focus/:active/:hover): regression dirs html/interaction/focus, popovers, dialog, forms, pointerevents, uievents/mouse, quirks, css/selectors/i18n, css-shadow-parts
- MUST-PASS (Studio a320eb224 elementsFromPoint): css/CSS2/normal-flow/block-in-inline-hittest-002.html passes today; a failure at build points at a320eb224
- Studio b84cec3ac (scrollable overflow zero axis): medium risk — css/css-overflow + flex/grid scrollWidth/scrollHeight pages
- Studio S04 e2a672f65 responsive iframe: the 14 allow-origins expect=150 variants MUST stay passing; resize-observer iframe pages
- Studio scoped VT 193dc92e4 changes the shared animation runtime: run css/css-view-transitions (~64 pass today), css/css-animations, web-animations
- c8288f15a (taffy % grid tracks floored to 1/64): Shippy screenshot A/B + css-grid reftests; reviewer expects off-by-one column edges on % grids (e.g. 13% of 450px) until the float-noise snap fix — note f07bf214a "a percentage track within float noise of a layout unit snaps to it" is already on the batch
- 20aed722f (scroll methods return ScrollResult promises): run the wright/Playwright drop-in e2e that uses scrollIntoViewIfNeeded — must still resolve to undefined (9a244304e on the batch targets this)
- lazy captures d4afe87e2/f630e4c3d: full-page CDP + MCP screenshot of below-fold loading=lazy images painted; a hung iframe holds the shot up to 10 s (expected, bounded) — refined by 012d066bd: screenshots wait only on lazy content they started (iframes ≤2 s)
- Studio 8c117ae60 smooth scroll rewrites scroll-promise settling (promises settle at end of smooth scroll): cssom-view scroll promise pages, scroll-snap events, scrollend, scroll-animations (75 pass today), Playwright drop-in scrolling e2e
- JSC build for the batch needs patches through 0041: 0038, 0039 module-fetch-referrer, 0040 deferred-work, 0041 named-properties-chain (all git apply --check clean per owner); the off-batch collection-deletion 0039 is not part of this build
- JSC 0041 named-properties chain (P16, f47805459): on global-lookup or realm-startup regressions, drop 0041 first (integrator 2026-11-22). P16 is only 2 pages.
- VT build checklist: Studio scoped VT 193dc92e4 (SC-scoped-VT, 7) + VT-2, with 4c9151ea3 (Pyro-only VT skip keeps ready handled) on top. Check together. CSP wasm-unsafe-eval allow pages (d01495c1b + a8c9d7350/JSC 0040).
- f47805459 (JSC 0041): run named-access-on-the-window-object/window-named-properties-object*.html + WebIDL global-object/legacy-platform-object pages before vs after; expect lost subtests for defineProperty/delete/preventExtensions/setPrototypeOf on Object.getPrototypeOf(Window.prototype) until 0041 gains false-returning overrides (batch reviewer …660de521).
- 8c117ae60 (smooth scrolling): drop-in e2e with html{scroll-behavior:smooth}: elementHandle.scrollIntoViewIfNeeded() then boundingBox()/screenshot show the final position.
- 3db74cb02: img.src="http://["; await img.decode() must reject, not hang — fixed at tip by ffd49eb93 (settle_image_decodes on the invalid path); verify on build day.
- 0206f4243 compile break FIXED by ea7daf4c4 (integrator; history handling computed into a local; rustfmt parses frames.rs at tip).
- SC-CSP-audioworklet 491b3a3eb: webaudio/idlharness.https.window.html passes ≥358; audioworklet-audioparam-size, audioworkletnode-automatic-pull, -construction, audioworkletprocessor-process-frozen-array, -process-zero-outputs expect harness ERROR not timeout; the 14 worklet-paint CSP pages + worklets/ no paint regression; pyro-webapi paint-worklet tests.
- SC-CSP-frame-ancestors 6d12c0e4b (blocked child frame → opaque chrome-error doc firing load; touches MacBook html nav): cargo test -p pyro-net csp::tests; pyro-browser --test child_frames --test loading; pyro-cdp --test engine_cdp; WPT frame-ancestors/, CSP frame-src/* child-src/*, blob/frame-src-*, sandbox/iframe-self-all-directives.sub, form-action-src-default-ignored.sub, reporting-api-works-on-frame-ancestors.https.sub, html …/navigating-across-documents/failure-check-sequence.https, resource-timing/iframe-failed-commit; also MacBook sandbox/B-7 iframe pages.
- R-HTML-RENDER-a (32e2affad etc.): fieldsets are now flow roots — text no longer wraps around outside floats; check fieldset-heavy e2e/screenshots (Shippy).
- SC-CSP-inheritance 8901cc5b4: inheritance/ (incl. inherited-csp-list-modifications-are-local, blob-inherits-from-meta-http-equiv-with-invalid-characters, window-open-local-after-network-scheme.sub); meta/sandbox-iframe, sandbox/meta-element.sub, blob/frame-src-blob-matches-blob.sub; frame-src/child-src about-blank-allowed-by-*; the-iframe-element/sandbox-inherit-to-blank-document-unsandboxed (MacBook sandbox row); trusted-types inheriting-csp-for-local-schemes, DOMParser-parseFromString-regression, Element-toggleAttribute, Element-setAttribute-*-adoption-*; html/browsers/origin/inheritance/about-blank-iframe, about-srcdoc.
- 6b5540ca0 vs event-handler-sourcetext.html: handler toString loses the name (trade-off for no self-binding); page stays failing (was failing in RC5, not a regression vs baseline).
- SC-CSP-object-src-no-url 3ddaa086c: object-src-no-url-allowed unchanged; html the-object-element/ + the-embed-element/ unchanged (MacBook html).
- SC-CSP-timer-strings 8514c1841: gated TT setTimeout/setInterval pages keep passes (may need baseline refresh for gains); html/webappapis/timers/ (MacBook html), workers/WorkerGlobalScope_set{Timeout,Interval}, dynamic-import string-compilation-nonce-* unchanged.
- JSC 0041 after 92579f9cd: overrides added, so window-named-properties-object should no longer lose subtests — verify before/after; perf risk: new window-property assignments take JSC prototype-put path (OverridesPut) — check startup/global-assignment microbench.
- SC-CSP-prefetch efcb7144e (pyro now actually prefetches): cargo test -p pyro-net prefetch_is_allowed_by_default_src_or_any_fetch_directive, -p pyro-browser -p pyro-dom -p pyro-cdp; the 9 pages; preload/prefetch-load-event (stay passing), preload/prefetch-events; resource-timing/initiator-type/link; MUST KEEP 21 "Expects blocked" subtests in mixed-content/gen/*/link-prefetch-tag.https.html; fetch/metadata/generated/element-link-prefetch.optional.sub; referrer-policy/generic/link-rel-prefetch. Accepted loss: connection-allowlist link_rel_whitespace_bypass 13/13 → 2/13.
- SC-reporting-observer 2fecb1f3e: cargo test -p pyro-webapi csp_report; pyro-browser --test bindings review_; guards webidl/ecmascript-binding/attributes-accessors-unique-function-objects, dom/interface-objects, html/webappapis/animation-frames/* (MacBook html).
- 4930d2f7d (width/height → aspect-ratio for img/video/canvas/input image): changes before-load layout for every <img width height>; check Shippy screenshots + lazy-img/scroll e2e.
- Screenshot lazy-wait regression (d4afe87e2 + f630e4c3d waited on any frame pending_loader for 10 s): fix 012d066bd (…5b822e93) — screenshot waits only on lazy loads it started, iframes ≤2 s, poll-thread start/stop race fixed by generation counter. Blocker cleared at source level; verify on build day (screenshot latency on lazy-heavy pages, Shippy).
- 4930d2f7d source review PASSED (canvas reviewer …d0f2a87d). Build checks: WPT html/rendering/replaced-elements/attributes-for-embedded-content-and-images/ (canvas-aspect-ratio, img-aspect-ratio*, video-aspect-ratio); full reftest gate (pre-load layout of every <img width height>); html/canvas pages sizing a canvas with CSS width only; also on Linux CI.
- e65964a0b (input.rs): trusted mouse clicks inside a disabled form control no longer reach ancestor listeners (Chromium). Run the Playwright drop-in e2e (Danex/ToS) — click on disabled buttons, aria-disabled vs disabled.
- SC-CSP-embedded-enforcement 942c26f23: cargo test -p pyro-browser --lib csp_embedded; pyro-browser --test integration (csp_embedded, child_frames, preload_scanner, child_frame_network); pyro-cdp; regression html/semantics/embedded-content/the-iframe-element/* (MacBook), CSP frame-ancestors/*, inheritance/*, resource-hints prefetch pages.
- b7ea8cdd0: a right click outside an open popover no longer closes it (Chromium) — e2e check for context-menu flows.
- 27bc62615: document.activeElement falls back to body or null (Chromium) — null in SVG/XML docs with nothing focused; check Playwright focus flows and SVG pages.
- 581fecf5b: focusing a text field moves the document selection (collapses before the field, Chromium) — check Playwright fill/selectText/press flows and editing pages; rustfmt unchecked on editing.rs (coordinator).
- SC-CSP-style-src 8a51aa133 guards: style-src/{style-src-hash-allowed, -hash-case-insensitive, -injected-inline-style-allowed-with-content-hash, -multiple-policies-multiple-hashing-algorithms, stylehash-default-src.sub, inline-style-attribute-on-html.sub, -inline-style-with-csstext, inline-style-allowed-while-cloning-objects.sub}; style-src-attr-elem/{attr-allowed-src-blocked, elem-allowed-src-blocked}; unsafe-hashes/style_attribute_allowed; nonce-hiding/nonce-hiding-move-document; html …/the-select-element/select-csp-style-src-attr (MacBook); cargo test pyro-dom, pyro-webapi. Inline styles/style attrs under CSP now blocked — check e2e apps with CSP headers.
- ff95c542b: load event waits for postMessage deliveries queued during load (one task later than before, Chrome order) — check readiness/waitForLoadState e2e and fork readiness.
- 8aa5ea888: UA rule now hides a <form> directly inside table/tr/thead/tbody/tfoot (Chrome) — check Shippy/screenshot + e2e for layout changes on real pages.
- c6295b18e: worker engines run JSC deferred work (needs 0040) — verify CSP wasm-unsafe-eval worker variants and wasm/jsapi async pages settle; worker teardown unregisters callback.
- ec8fb83dc (review passed): <canvas content=drawable onpaint> fires; pages without content=drawable never run the paint step — perf check on canvas-heavy pages.
- 3de941ab4 (review passed): html-in-canvas hit-test pages (draw order, pointer-events:none/inert skipped, laid-out children not hit); regression: elementFromPoint unchanged on pages without drawable canvases.
- dca25d3de: setSelectionRange/selectionStart on input[type=email] now throw/null (HTML/Chrome); select() kept for Playwright fill — check Playwright fill/type/press/selectText on type=email and the Danex e2e login forms (coordinator).
- 3e46da0ca: blur fires change only after a user edit (value set by script no longer fires change on blur) — check Playwright fill + blur/change listeners in e2e.
- dbfde500e: response MIME via Fetch extract-a-MIME-type; malformed Content-Type now yields empty MIME (Chrome) — check script/style loading on sloppy servers + full e2e suites (coordinator); charset also taken across all Content-Type values.
- 63b205937 (review passed): WPT 2d.text.drawing.style.fontKerning + fontKerning.with.uppercase (element/offscreen/worker); regression: other 2d.text.measure.* width pages with default kerning; cargo test -p pyro-canvas.
- 1ab086809: navigations with unknown MIME (*/*, unknown/unknown, application/unknown) are now sniffed (HTML/XML/text) — check download-vs-render behaviour in e2e; nosniff, error documents and binary bodies exempt (coordinator).
- 07194d15f: structured clone detects length-tracking views by resizing the source resizable ArrayBuffer by ≤8 bytes during serialization — verify no observable side effects (resize events/limits) at build.
- REGRESSION 781e7100a: run 2d.pattern.svgimage.{nonexistent,zeroheight,zerowidth}, drawimage_svg_image_1, imagebitmap createImageBitmap-* vector-SVGImageElement subtests; zeroheight/zerowidth expected FAIL until the reviewer's fix (SVG bodies in image::dimensions) lands — FIXED by 7da6b6939 (reviewer) — verify at build.
- d4fee3806: modal dialog with author position static/relative now computes absolute, display:contents → block (top-layer UA rule now effective) — check dialog-heavy e2e/screenshots. e275a4bb2: execCommand throws in XML documents (Chromium).
- a6706c23b grid-to-CSS scale: fixed by d6178adf9 + 7bb67f14e (drawElementImage transform stored without bitmap/CSS scale; updateElementGeometry stays unscaled per WPT) — probe a width=300/CSS 600 canvas on build day.
- 8f92c1072: window.stop() now aborts the document (no load/pageshow after a page's own stop) — CDP/Playwright clients waiting for load after window.stop() get none (Chrome); check e2e.
- 12d84f4cf: byte parser may wait up to 64 KB for the meta-charset decision — run crates/pyro-dom/tests/loader.rs; check streaming/slow-network pages and e2e time-to-first-parse; verify charset/with-inheritance + xhr.
- 28b24967f: Location navigations run beforeunload listeners synchronously inside assign()/href setter (Chrome) — check Playwright goto/click-navigation with beforeunload dialogs.
- 5526cff79: number inputs drop a typed +/- unless valid at that position — check Playwright type/fill/press on type=number (negatives, exponents).
- f86a2669b: click on an editable (contenteditable) link no longer navigates, incl. element.click() — check e2e.
- f27581dc4: textarea hard wrap inserts LF in the value; submitted bodies still CRLF — check form-submit e2e.
- 1c9e5ceee: label.focus() forwards unless the label is itself focusable (refines f354df9fd).
- 7aa109627: an iframe with a javascript: src now holds its parent's load until the script ran (and the string-result document loaded) — check load timing in e2e with javascript: iframes.
- d6c79a73d: UTF-16 sniff from "<?x" without BOM — check XML/XHTML loading.
- ca09913e3: an inserted <img> keeps its request (no re-resolve on base URL change) — check lazy/insert img e2e.
- c0d4f513b: a popup's window.opener is now the window of the frame that opened it (not the top window) — check Playwright popup/opener flows opened from iframes.
- 1dce34479: <link rel=compression-dictionary> now fetches; image preloads use imagesrcset and the preload scanner skips such links — check preload behaviour on real sites.
- 83e7fe8fe: noscript/template now appear in the render tree (display:none) — structural selectors (:nth-child, +, ~) count them in rendering (spec/Chrome); check reftests that relied on the old omission; render_html.rs expectations updated.
- 8901cc5b4 regression guards (Studio 2026-11-23: none expected to flip, do not credit): html/semantics/embedded-content/the-iframe-element/sandbox-inherit-to-blank-document-unsandboxed.html (keep its 1 passing subtest; still needs MacBook CSP-sandbox-opaque-at-commit), html/browsers/origin/inheritance/about-blank-iframe.html and about-srcdoc.html (must stay unchanged; commit touches the initial about:blank/srcdoc commit path).
- bb6043563: a dialog whose first focus candidate is a delegatesFocus shadow host now focuses inside the host (spec), not the dialog itself. Build-day: also check focus-after-close and top-layer-removal-invalidates-ua-styles (owner thinks already fixed; not credited).
- e65964a0b (trusted mousedown/mouseup/click blocked at disabled form controls) must keep excluding fieldset/option/optgroup: d7a9963e6 (click() on disabled fieldset dispatches) and the native-click half of fieldset-event-propagation.tentative depend on it (…be6eb5a1).

Additional watchlist notes:
- 2026-11-22 coordinator: history navigation rate limit risk removed (cc806a11f adds an off switch used by the Playwright drop-in); R-HTML-25 now 7 (recorded on the row).

## 5. Per-commit test checklist

Taken from each ledger row's `review.build_checklist`.

### OWN-studio-margin-trim: used block-end margin-trim, self-collapsing/nested children
Commit: 51466498d (from 7a76e2b1e) (integration/round7-batch-1000)
- 51466498d: run with min-height on the trimmed child (edge case outside the collapse condition)

### Q02: Unitless-length quirk not applied in quirks-mode documents
Commit: e1924dc29 (from fdd7d2c6c) (integration/round7-batch-1000)
- e1924dc29: WPT quirks/unitless-length; style="" stays no-quirks by design, so expect those subtests to keep failing

### Q03: UA rendering-section rules: unicode-bidi by element/dir, and form-control resets
Commit: {'integration/round7-batch-1000': {'bidi': 'fa918da33', 'form_control_resets': '309b1d3a75ce15c29bfa4cb45bd7e6f473e2eb04'}, 'topic_originals': ['6aec573dd', '91aa9ca57']}
- fa918da33: css-ruby reftests (rt no longer isolated, so shaping crosses rt edges)
- 309b1d3a7: form-control reftests and the Shippy pixel run (overflow: clip on text inputs)

### Q12: Canvas display-p3 colour space: drawImage/ImageBitmap/getImageData conversions
Commit: 5da88db3e (integration/round7-batch-1000)
- 5da88db3e: 2d.color.space.*, getContextAttributes, getImageData/putImageData with display-p3 and the linear spaces. REGRESSION: every 8-bit sRGB pixel page (sRGB intended byte-identical)

### S03: min/max stretch sizing in flex/grid/block
Commit: f031dc059 (integration/round7-batch-1000)
- taffy grid/flex tests; the 8 stretch pages

### S04: responsive iframe content-width / requestResize
Commit: e2a672f65 (from f74e417d8; integration/round7-batch-1000)
- cargo test -p pyro-browser responsive_frames::tests::allow_origins_lists_admit_their_embedders
- pyro-browser --test child_frames an_embedders_script_reads_a_responsive_iframe_at_its_content_size
- pyro-dom --test render_html frame_documents_are_replaced_per_page
- all css/css-sizing/responsive-iframe/; the 14 allow-origins expect=150 variants MUST stay passing
- resize-observer/ iframe pages; css-sizing contain-intrinsic-size/auto-006

### C01: Canvas fill/stroke/shadow colour parsing: non-integer rgb dropped, invalid legacy forms accepted
Commit: 841e4bfbf (integration/round7-batch-1000)
- cargo test -p pyro-dom canvas_color
- 841e4bfbf: 2d.fillStyle.parse.*, 2d.fillStyle.colormix.currentcolor, 2d.fillStyle.relativecolor.currentcolor, 2d.gradient.object.current, 2d.shadow.attributes.shadowColor.*

### C02: Canvas shadows: alpha, transform independence, pattern/gradient/image shadows
Commit: 9f2703e69 (integration/round7-batch-1000; source fb6bef022)
- cargo test -p pyro-canvas
- 9f2703e69: 2d.shadow.* with off-canvas geometry, incl. 2d.shadow.alpha.4

### C03: Canvas compositing: 'clear' operator and uncovered areas with no-context canvases
Commit: 142925160 (clear) + bd86a940d (no-context source) on integration/round7-batch-1000
- cargo test -p pyro-canvas
- 142925160 + bd86a940d: 2d.composite.*.clear, 2d.composite.uncovered.nocontext.*

### C04: Canvas text: font shorthand parsing and fontVariantCaps
Commit: fd9e97f2d (integration/round7-batch-1000)
- fd9e97f2d: there are NO tests. Add dom_js cases for small-caps serialization, fontStretch reset on font set, inherit/var() rejected, and % against a styled canvas; check calc()/vw sizes (now rejected)
- fd9e97f2d: 2d.text.font.parse.*, 2d.text.font.relative_size, 2d.text.font.weight, 2d.text.fontVariantCaps*, fontStretch/fontVariant settings, OffscreenCanvas-ctx-font-sibling-index-invalid. REGRESSION WATCH: parse.system, relative_size, percentage.default and sibling-index pass today

### C05a: Canvas measureText TextMetrics from font tables: fontBoundingBox, actualBoundingBox, baselines, width.basic
Commit: 02245b282 (integration/round7-batch-1000; counted together with C15's ae993d5b2)
- 02245b282: 2d.text.measure.baselines, 2d.text.draw.baseline.{hanging,ideographic}

### C05b: Canvas text align start/end resolved via direction (rtl)
Commit: 1933ab2dd
- 1933ab2dd: 2d.text.draw.align.{start,end}.{ltr,rtl}

### C05c: Canvas measureText width with a null character
Commit: e016f96f4
- e016f96f4: 2d.text.measure.width.nullCharacter, 2d.text.draw.space.collapse.nonspace

### C05d: Canvas measureText lang (lang and lang.inherit)
Commit: b957b5caf
- cargo test -p pyro-canvas
- b957b5caf: 2d.text.measure.lang

### C05e: Canvas fontVariantCaps small caps (fontVariantCaps2, taken from C04)
Commit: 2d62e0765
- cargo test -p pyro-canvas
- 2d62e0765: 2d.text.fontVariantCaps2 and other small-caps pages

### C05f: Canvas: a vertical tab does not end the text's line (space collapse and measure.width.space)
Commit: fd362425b (with ae993d5b2)
- fd362425b: 2d.text.draw.space.collapse.nonspace, 2d.text.measure.width.space, plus 2d.text.draw.space.collapse.* (regression). BUILD-TIME RISK: U+0001 must get no font fallback in parley; if it falls back, the 'EE' run shifts and nonspace fails

### P06: scheduler.yield and priority inheritance
Commit: bea87c3eb, e2e3821f1, 0b747ed47, 74d54863e, 6a567af8c, 400ad99f4, 65b515e5b, 23a9a7b82 (from a54567b0e, 485d9a9fd, 42dcf2313, 224047aad, cabf949cb, eeb9e46ec, 5868f207e, 126069a7d) (integration/round7-batch-1000)
- bea87c3eb: run pyro-runtime user_visible_continuations_resume_before_ready_ordinary_tasks FIRST (asserts log[3]=='bg-cont'; a 0 ms timer that becomes ready only on a later tick puts bg-cont at [2])
- e2e3821f1: WPT scheduler/yield-inherit-* from requestIdleCallback after an await (idle.rs wraps only callback.call, so microtasks may see the user-visible priority)

### F02a: focusgroup arrow/Home/End navigation (no Tab, key-conflict, popover or shadow pages)
Commit: f73eb76b0 (integration/round7-batch-1000)
- focusgroup (f73eb76b0/c4e0e43b9/b4c716018/18c7e8ff2/41ddc247c/1215c90fb): WPT html/interaction/focus/focusgroup/tentative, after the 88224f9d8 revert is fixed
- focusgroup_entry signature changed (one caller updated); per-step Tab scope rebuild is a perf candidate

### F02d: focusgroup IDL: focusGroup ([SameObject, PutForwards=value] DOMTokenList, 22 supported tokens) and focusGroupStart reflection on the HTMLOrSVGOrMathMLElement mixin
Commit: cd4abb412 (integration/round7-batch-1000; also origin/source/f02-focusgroup)
- b35d4961e touches every DOMTokenList: run dom/nodes Element-classlist*, DOMTokenList-*, html reflection pages; set_attribute_ns does not lowercase (all token-list attribute names are lowercase literals)
- GUARDS b35d4961e (0 failures today in bench/wpt/baseline.json): dom/nodes/Element-classlist.html (1420), dom/nodes/attributes.html (67), dom/nodes/MutationObserver-attributes.html (42), dom/lists/DOMTokenList-{value,stringifier,iteration,Iterable}.html, dom/nodes/{Element,Document}-getElementsByClassName.html, dom/nodes/ParentNode-querySelectorAll-svg-class.html, dom/nodes/getElementsByClassName-10.xml, -11.xml; watch dom/lists/DOMTokenList-coverage-for-attributes.html (175, 1 known failure) not worse

### F03p1: Customizable select: base-select preference, UA sheet, picker :open/showPicker/mouse toggle
Commit: edb28212b (integration/round7-batch-1000)
- CSS.supports(appearance, base-select) true while picker unrendered: turn pref off before release if p2 not done

### F03p3: Customizable select picker keyboard (Home/End/PageUp/PageDown, arrows, hidden options)
Commit: 8e7a9f52f (integration/round7-batch-1000)
- handled picker keys suppress keypress: check other select/keyboard pages
- send_keys to an option without a layout box may be refused (why 4 pages are tentative)

### F03p2: Customizable select: lay out options in the open picker (select not a Blitz leaf box, out-of-flow picker box, hit testing, top-layer paint)
Commit: d7ee74c1b (integration/round7-batch-1000)
- picker paints/hits as select children (no top layer/z-index): a later overlapping sibling can paint over or catch the click
- the select overflow grows while open (ancestor scroll areas may grow)
- ::picker(select) styles not applied
- closed selects take no new path
- F03p3 tentative keyboard pages may now pass (options have boxes)
- GUARDS (pass today in bench/wpt/full + baseline.json; any change = regression): html/rendering/replaced-elements/the-select-element/select-1-block-size.html; …/select-display-inline.html; …/select-setting-selected-option-from-js-updates-visible-state.html; …/customizable-select/min-size-empty-001.html; …/customizable-select/min-width-in-flex.html; html/rendering/bindings/the-select-element-0/listbox-option-in-div.html; html/rendering/non-replaced-elements/form-controls/select-sizing-001.html; css/css-contain/contain-size-select-elem-001.html; css/css-flexbox/select-element-zero-height-001.html; css/css-ui/appearance-listbox-001.html; html/semantics/forms/the-select-element/customizable-select/select-click-picker-light-dismiss.tentative.html; …/select-button-behaviors.html; …/select-base-appearance-computed-style.html; …/customizable-select-listbox/select-listbox-option-optgroup-client-rects.html; …/customizable-select-multiple-popup/select-multiple-popup-mouse-behavior.tentative.html; html/semantics/forms/the-select-element/common-HTMLOptionsCollection.html
- manual: open base-select picker, pick an option, elementFromPoint at the old option position must hit the page (option boxes cleared on close)

### F03e: List box option press picks it (select-user-selection-dirtiness)
Commit: ca9829603 (integration/round7-batch-1000)
- no Ctrl/Shift range selection for multiple list boxes (Chrome extends with modifiers)

### F04b: Module graph load rejects with the cached evaluation error / unresolvable specifier
Commit: 2dc16d142 (integration/round7-batch-1000)
- 2dc16d142: WPT module error-to-rethrow pages (cached evaluation error, unresolvable specifier)

### F04c: Module map: failed fetches not cached; worker fetch joining
Commit: 6778ba036 (integration/round7-batch-1000)
- 6778ba036: check whether JSC's module registry caches a rejected fetch (a re-import() in the same realm should refetch)

### F04j: Module fetch referrer + importer's fetch options (credentials) via new JSC patch 0039-module-fetch-referrer
Commit: 2dbda236a (integration/round7-batch-1000)
- needs JSC rebuild with 0039; inert without the patch
- 0039 number clashes with the held collection-deletion patch (see POLICY-jsc-0038)

### F04h: import() in a module service worker rejects once its script has run (JSON module SW test)
Commit: 147f68d86 (integration/round7-batch-1000)
- TLA import() in module SW still allowed (spec gap)

### F05: Dedicated worker script fetch referrer policy (classic + module)
Commit: af91e97df195bee7c1cbb363f62eaa8d9f9d610c (integration/round7-batch-1000; nested dedicated Worker)
- af91e97df: worker cap/reuse incl. shutdown; nested echo/error at the creator; close and page teardown; redirected Referer; system vs patched JSC. Uncanceled nested errors don't propagate further (documented)

### OWN-mse-mp4: MSE bare MP4 support on actual buffered audio (06f043fa0)
Commit: c6ae4d6cc (= 06f043fa0, already present) (integration/round7-batch-1000) + review fix 0c7c960d4
- c6ae4d6cc + 0c7c960d4: cargo test -p pyro-webapi --features test-hooks media_source_js (rewritten assert_attached_tone)

### C06: strokeRect with zero width/height
Commit: 78842e176 (integration/round7-batch-1000, push a3d8fe523..4578ebc4a)
- 78842e176: 2d.strokeRect.zero.4/5

### C07: roundRect winding
Commit: 476797372 (integration/round7-batch-1000, push a3d8fe523..4578ebc4a)
- 476797372: 2d.path.roundrect.winding; all 2d.path.arc.* and 2d.path.roundrect.* (regression)

### C08: conic gradient rotation
Commit: c06affd5c (integration/round7-batch-1000, push a3d8fe523..4578ebc4a)
- c06affd5c: 2d.gradient.conic.*

### C09: ImageData [EnforceRange] arguments
Commit: 2edfe96e8 (integration/round7-batch-1000, push a3d8fe523..4578ebc4a)
- 2edfe96e8: 2d.imageData.{create2,get,put}.nonfinite

### C10: canvas text letter/word spacing
Commit: 8d4b9ff1b (integration/round7-batch-1000, push a3d8fe523..4578ebc4a)
- 8d4b9ff1b: 2d.text.drawing.style.{absolute,font-relative,invalid,nonfinite}.spacing, letterSpacing/wordSpacing.measure

### C11: canvas text lang
Commit: 0b3990bf5 (integration/round7-batch-1000, push a3d8fe523..4578ebc4a)
- 0b3990bf5: 2d.text.lang.default

### C12: canvas filter attribute value
Commit: b39aa4bf0 (integration/round7-batch-1000, push a3d8fe523..4578ebc4a)
- b39aa4bf0: 2d.filter.value

### C13: canvas pixelFormat
Commit: 7cb231709 (integration/round7-batch-1000, push a3d8fe523..4578ebc4a)
- 7cb231709: 2d.imageData.object.properties

### C14: canvas image source usability (img)
Commit: 4578ebc4a (integration/round7-batch-1000, push a3d8fe523..4578ebc4a)
- 4578ebc4a: 2d.drawImage.incomplete.*, 2d.drawImage.zerosource.image, 2d.pattern.image.{nosrc,zeroheight,incomplete.*}

### C15: Canvas text ignores document/worker web fonts (FontFace/@font-face) when drawing
Commit: 7e2d92d6d + d00b2fe6c + ae993d5b2 (integration/round7-batch-1000)
- d00b2fe6c + 7e2d92d6d + ae993d5b2: 2d.text.draw.fontface*, 2d.text.draw.baseline.*, 2d.text.draw.fill.maxWidth.bound, any CanvasTest-font page; worker variants must pick up self.fonts faces
- SMOKE: a text-heavy canvas page with a named web font, timed (per-draw face lookups)

### P16: Window global prototype chain: window -> WindowProperties -> Object.prototype instead of via Window.prototype/EventTarget.prototype
Commit: f47805459 (+92579f9cd) (integration/round7-batch-1000; JSC patch 0041-named-properties-chain)
- needs JSC rebuild with 0041; inert without it
- global property misses now pass through Window.prototype before WindowProperties; EventTarget members still copied onto window; worker globals unchanged
- window-named-properties: 3/5 failing subtests should pass, page not counted (Dynamic/Ghost name need child window.name tracking)
- overlaps P11 only in helpers.js installGlobal

### R-FETCH-9b: HTTP cache stale-while-revalidate
Commit: f61dc654c
- f61dc654c: no unit test for Lookup::StaleWhileRevalidate. Add one, plus must-revalidate once fixed

### F08c: Script force async flag + in-order list for script-inserted async=false external scripts
Commit: 4149e5a6f (integration/round7-batch-1000) + b6382bb69 (reviewer-A follow-up)
- 4149e5a6f: WPT script-inserted async=false ordering; createContextualFragment scripts keep force async (as in Chrome)

### R-HTML-B-1b: Event handler attribute parameters shadow the element's members (lexical scopes, sourcetext)
Commit: 75ae37859 (integration/round7-batch-1000; builds on 77316c216)
- medium confidence on compile-event-handler-lexical-scopes test 3 (baseline SyntaxError; Window-scope fix should resolve)
- scope chain element / form owner (form-associated only) / document, none for Window handlers; body/frameset onerror 5 params; source text function on<type>(params) {\n<body>\n}

### R-HTML-B-3: document.open/write while loading (ignore-destructive-writes, abort the parser)
Commit: c9216028f (integration/round7-batch-1000; pyro-webapi html.rs, pyro-browser loader.rs)
- deferred script document.open: check document-open pages + real-site smoke
- 6a90a0811 supersedes part of the deferred-script watchlist note: remaining deferred scripts are now dropped after document.open, DOMContentLoaded skipped for the old load

### R-HTML-B-7: No 'replace' for navigations while loading; abort the old document's load on navigation (medium confidence)
Commit: f10b98964 + 1ae2525f6 + 1215ead74 (integration/round7-batch-1000)
- steps of a document navigated away from (its iframes' entries) drop out of history.length; Chrome keeps them
- same-document back();back() in one task now does one traversal (first wins)
- GUARD: ~/Library/Caches/lightningpanda/round7-local/history-nav-guards/iframe-or-traversal.txt (181 pages passing in census c3372ad21 that use iframes / history.back/go/forward / location navigation / .src); full set ~/Library/Caches/lightningpanda/round7-local/history-nav-guards/passing-today-c3372ad21.txt (302)
- a799ad8bb guard: CDP history tests (pyro-browser/tests/loading.rs history_entries, pyro-cdp engine page tests); single-frame pages must list the same entries/ids as before
- build check: joint_session_history/001, same-url, assign_after_load, iframe-nosrc, iframe-src-204 (firm); history-traversal-navigates-multiple-frames (census showed length 3 after one top location.hash), browsing_context_name (tentative)
- 1215ead74: any navigation of a frame with iframes now runs beforeunload in those iframes too (as Chrome)

### R-HTML-B-7b: No load event for a document its frame is leaving
Commit: ff740683f (integration/round7-batch-1000)
- a navigation fetch that never finishes now keeps the old document's load from firing (as Chrome)
- GUARD: ~/Library/Caches/lightningpanda/round7-local/history-nav-guards/iframe-or-traversal.txt (181 pages passing in census c3372ad21 that use iframes / history.back/go/forward / location navigation / .src); full set ~/Library/Caches/lightningpanda/round7-local/history-nav-guards/passing-today-c3372ad21.txt (302)

### R-HTML-9a: HTML-in-Canvas stage 1a: requestPaint(), paint event / CanvasPaintEvent.changedElements, onpaint
Commit: 69c012965 (integration/round7-batch-1000)
- WPT html-in-canvas paint-event pages: CanvasPaintEvent constructor, changedElements [SameObject], requestPaint, initial paint, reverse tree order, onpaint content attribute
- perf regression: canvas-heavy animated page mutating the DOM each frame, timed before/after (run_paint_step does per-frame journal + ancestor walks once a <canvas> was ever seen)
- pages with no canvas never enter the paint step (early return on canvas_seen)
- conditional: only if 94fe1804b (drawElementImage) is dropped/reverted, unregister CanvasPaintEvent/requestPaint/onpaint before release
- ec8fb83dc supersedes the canvas_seen per-frame cost item: now gated on drawable_canvas_seen; known gap: a drawable canvas made only by cloneNode is not noticed until some element gets content=drawable

### R-HTML-9b: HTML-in-Canvas stage 1b part 1: canvas.content IDL + UA styles for content=drawable children
Commit: 108eae481 (integration/round7-batch-1000)
- WPT html-in-canvas: canvas.content reflection (missing/invalid → "fallback"); computed position/isolation on children and [drawable] descendants of a content=drawable canvas
- regression: html/dom reflection pages enumerating HTMLCanvasElement attributes

### R-HTML-9c: HTML-in-Canvas stage 1b part 2: drawable canvas children laid out inside the replaced box, not painted in place (vendored blitz-dom/blitz-paint + UA contain/isolation)
Commit: f219b1c51 (integration/round7-batch-1000)
- layout-canvas-children "fallback" assert may still fail (old child layouts not reset when switching back to fallback)
- run the 20 canvas/replaced-element guard pages listed in POLICY-build-watchlist
- WPT html-in-canvas layout pages: getBoundingClientRect of a canvas child inside the canvas box; children not painted (reftest: canvas with children ≈ empty canvas)
- confirm the canvas DOM children get layout boxes at all (construct.rs has no canvas special case)
- reftest gate on ordinary <canvas> and replaced elements (img, video, object): paths must be byte-identical
- CI vendor-patches workflow: regen-patch --check --all --base HEAD^1
- ec3524937 + fd8e172a4 (review passed): reftests with content=drawable canvas whose children are inline text, a float child (phased path) and multicol — nothing paints over the bitmap; regression: ordinary canvas fallback content and list markers elsewhere unaffected

### R-HTML-9d: HTML-in-Canvas stage 2 items 1-3: blitz-paint paint_element_subtree, Renderer::paint_element, GeometrySource::element_image
Commit: 9259003cd (integration/round7-batch-1000)
- blitz-paint UPSTREAM.patch regenerated (regen-patch --check --all passes)
- WPT draw-element-image functional pages (returned transform, InvalidStateError before first paint event, top layer, display:none)
- SECURITY GATE: draw-element-image/privacy/* (html-images-ignored, background-*-ignored, iframes-ignored, video-ignored, visited-link/*) expected FAIL until the security fix; smoke: drawElementImage of a canvas child holding a cross-origin <img>, then getImageData — must be blank or SecurityError after the fix
- CI vendor-patches (blitz-paint)

### R-HTML-9e: HTML-in-Canvas stage 2 item 4: CanvasRenderingContext2D.drawElementImage (3/5/9 args, InvalidStateError, returns DOMMatrix)
Commit: 94fe1804b (integration/round7-batch-1000)
- ~30 pixel pages depend on blitz element-subtree paint fidelity and on privacy omissions (not counted)
- canvas.rs hunk additive, acked by C05 …c83c4608 and C15 …4e7af93f
- WPT draw-element-image functional pages (returned transform, InvalidStateError before first paint event, top layer, display:none)
- SECURITY GATE: draw-element-image/privacy/* (html-images-ignored, background-*-ignored, iframes-ignored, video-ignored, visited-link/*) expected FAIL until the security fix; smoke: drawElementImage of a canvas child holding a cross-origin <img>, then getImageData — must be blank or SecurityError after the fix
- CI vendor-patches (blitz-paint)
- cd4ef7dea (blanket taint, source review passed): security smoke now expects getImageData to throw SecurityError after drawElementImage, and toDataURL/toBlob to throw too
- under the stopgap, privacy/*-ignored pages FAIL by design and functional pages that read pixels back FAIL as expected; the 6 counted 9e pages do not read pixels (no getImageData/toDataURL/toBlob, not reftests) so they are unaffected
- b8b222900 (subtree_may_taint, source review passed; cd4ef7dea leftover removed by 016c83b79): text-only / solid-background element → getImageData readable; element with <img> (same- or cross-origin, both over-taint for now), CSS background-image, video, iframe or visited link → SecurityError
- draw-element-image functional pages reading back text-only content should pass; privacy/*-ignored stay FAIL until omission lands
- CI vendor-patches (blitz-paint)
- 016c83b79 (source review OK): the b8b222900 checks now apply — text-only readable; image/video/iframe/visited-link content → SecurityError

### R-HTML-9f: HTML-in-Canvas privacy stage part 1: omit other-origin CSS image layers (background, mask) and <img>; taint only unclassifiable content; WebIDL arg order; renderer sync
Commit: d28cf3778 + eeb1ad1df (omission by response / opaque SVG refs) (integration/round7-batch-1000); 2cd3f80e0 taint series
- same-origin-only canvas content stays readable; other-origin backgrounds/masks/<img> omitted (not tainted)
- still tainting (privacy pages keep failing): video, iframes/sub-frames, embed/object, :visited, feImage, other-origin border/list-style images, srcset/picture, generated-content images
- WPT privacy/*: html-images-ignored, background-png-images-ignored, box-mask-png-ignored; picture-images-ignored expected to taint (not omit); html-images-cors-allowed expected FAIL until classification uses the opaque flag
- REQUIRED SECURITY SMOKE: same-origin URL that 302s to a cross-origin PNG, as <img> and as CSS background in a drawable child → drawElementImage → getImageData must NOT return the pixels (omitted/transparent or SecurityError)
- REQUIRED SECURITY SMOKE (reviewer, at 194f06a78/1d63703f3): (a) same-origin→cross-origin PNG redirect as <img> and CSS background → SecurityError; (b) <img src=other-origin x.svg> in a drawable child — reviewer saw it drawn untainted (leak); (c) same-origin→cross-origin SVG redirect as CSS background — leak
- WPT privacy/html-images-ignored should pass for raster; draw-element-image/privacy svg/* expected FAIL until the SVG fix
- 44120fbc2 (source review passed): smoke (b) cross-origin SVG <img> → omitted; (c) redirected SVG CSS background → SecurityError; inline <svg> in a drawable element → SecurityError (over-taint by design)
- privacy/svg/*: omission-expecting pages may pass; readable-canvas-expecting pages fail by design until opaque-flag work (d28cf3778) applies
- d28cf3778: html-images-cors-allowed should pass; same-origin/data:/CORS images readable; redirect/SW-opaque images omitted
- EXPECTED at tip (d28cf3778): same-origin <img>/CSS raster readable; data: readable; crossorigin=anonymous + CORS readable (html-images-cors-allowed passes); cross-origin no-cors or same-origin→cross-origin redirect in <img>/background/mask omitted (transparent), canvas readable; cross-origin SVG <img> omitted; self-contained inline <svg> readable; inline <svg> with <image href=http…> or <use href=other.svg#x> → SecurityError; canvas/video children → SecurityError
- run draw-element-image/privacy/* in full: most *-ignored and *-allowed should pass
- filtered-div-ref-external-svg-ignored: a cross-origin external SVG filter flood colour must not be readable
- eeb1ad1df (source review passed): drawable child with filter / clip-path / mask: url(https://other/f.svg#x) → SecurityError (privacy/filtered-div-ref-external-svg-ignored, filtered-div-ref-svg-*); same-origin external references stay readable
- the 14 credited privacy pages (list on this row) must pass at the tip; run with the security smoke tests
- 62d68f95f SECURITY SMOKE: drawable canvas inside a visited <a href>, and a drawable child wrapped in a visited link → drawElementImage → getImageData must throw SecurityError

### R-HTML-9g: HTML-in-Canvas integrator minors: drawElementImage sized from the painted border box; paint_element uses screenshot text threshold and glyph-grid wrappers
Commit: 1d63703f3 (integration/round7-batch-1000)
- non-atomic inline drawable still paints almost nothing
- transformed drawable child (rotate/scale) draws at its untransformed border-box size; text rasterizes like screenshot text

### R-HTML-9h: HTML-in-Canvas: captureElementImage + ElementImage, getElementImageDefaultSize, getElementTransform, updateElementGeometry/clearElementGeometry
Commit: 04cc49dcf (integration/round7-batch-1000)
- updateElementGeometry geometry not applied to getBoundingClientRect or hit-testing
- no OffscreenCanvas/worker variants
- WPT html-in-canvas method pages: InvalidStateError cases, size from the last paint, grid scale (bitmap size / CSS box), 2D vs 3D transform round-trip, a new DOMMatrix on each read
- SECURITY SMOKE: captureElementImage of an element with an iframe or cross-origin video → drawElementImage(snapshot) into a second drawable canvas → getImageData must throw SecurityError

### R-HTML-9i: HTML-in-Canvas: getBoundingClientRect of a drawable element follows its canvas transform (updateElementGeometry / drawElementImage unless preserveElementGeometry); drawElementImage options dictionary
Commit: a6706c23b (integration/round7-batch-1000)
- canvas-transform-get-bounding-client-rect CSS-transform subtests are the riskiest
- WPT html-in-canvas geometry pages (drawElementImage then gBCR, preserveElementGeometry, update/clearElementGeometry)
- probe: canvas width=300 with CSS width:600px — reviewer expects the drawn element rect off by 2x until the grid→CSS scale is applied
- regression: gBCR on pages without a drawable canvas unchanged (early return on empty transforms)
- d6178adf9 (review passed): the width=300 / CSS 600px probe should now match Chrome for drawElementImage-placed rects; canvas-transform-get-bounding-client-rect stays unscaled

### R-HTML-B-12: Fire pagereveal (pageswap part not done: needs the navigation API)
Commit: 03ca275aa (integration/round7-batch-1000; pyro-webapi lifecycle.rs, page.rs; pyro-browser window.rs)
- pagereveal iframe test: rAF requested before load can fire pagereveal before pageshow — timing check

### R-HTML-B-12b: pageswap with NavigationActivation (entry/from/navigationType; null cross-origin), fired before pagehide; pagereveal as PageRevealEvent
Commit: ade05aef5 + 2956fc1c4 + 1ae2525f6 (integration/round7-batch-1000)
- pagereveal iframe page depends on the first frame not beating load
- 1ae2525f6: iframe src after load now pushes a history entry: check history/navigation pages that assumed replace, and R-HTML-JSH interplay

### R-HTML-B-13: Muted errors ('Script error.') for cross-origin scripts
Commit: 587041d48 (integration/round7-batch-1000) + 4f6386134 (review fix)
- two in-flight fetches of the same URL in different CORS modes share one muted verdict (later wins)

### R-HTML-10a: img loading=lazy: fetch after load, near the viewport
Commit: ee78275fd (integration/round7-batch-1000)
- 2b99c5fcc: image-loading-lazy, in-viewport-dynamic, below-viewport-load-event must still pass
- no layout forced during parsing (v0.0.44 lazy-img render-sync NO-GO): check page-load CPU and Verge-type sites
- after the screenshot parking fix: full-page CDP screenshot and MCP screenshot of a page with loading=lazy images below the fold must paint the images, not placeholders
- screenshot that started lazy content can take up to 10 s if an image never finishes; viewport capture with nothing newly approved stays immediate
- 012d066bd: a lazy iframe slower than 2 s is captured unloaded

### R-HTML-10d: Cancel a lazy iframe's pending load when it is navigated otherwise
Commit: 46abfec47 (integration/round7-batch-1000)
- navigate_frame_with_handling drops a lazy iframe's pending lazy load unless the navigation is its own attribute processing
- already-passing nav-* lazy iframe pages must not regress under 471824a27

### R-HTML-10e: Lazy-load leftovers expected to pass with ee78275fd / 471824a27 (no new commit; source re-read)
Commit: ee78275fd + 471824a27 (integration/round7-batch-1000), no new commit
- verify each page on build day; source-only reasoning: deferred request rebuilt from current attributes at approval, iframe lazy images wait for that iframe load

### R-HTML-13: Autofocus candidates flushed in update-the-rendering
Commit: 5225838c7 (integration/round7-batch-1000)
- 9ff0bf71c may schedule one extra animation frame on a popup sharing the DOM

### CG1: Canvas geometry: Arc sweep (round line joins)
Commit: 0baa26ab6 (integration/round7-batch-1000)
- 0baa26ab6: 2d.line.join.round; all 2d.path.arc.* (regression)

### CG2: Canvas geometry: isPointInStroke in user space with dashes
Commit: f1f589cf4 (integration/round7-batch-1000)
- cargo test -p pyro-webapi canvas_path (stroke_outline semantics changed)
- f1f589cf4: 2d.path.isPointInStroke.scaleddashes plus all 2d.path.isPointInStroke.* (regression)

### CG3: Canvas geometry: isPointInPath with non-invertible/overflowing transforms
Commit: cda220e33 (integration/round7-batch-1000)
- cda220e33: 2d.path.isPointInPath.basic plus all 2d.path.isPointInPath.* (regression)

### CG4: Canvas geometry: Stroke width cap vs far miters
Commit: 9dd317440 (integration/round7-batch-1000)
- 9dd317440: 2d.line.miter.obtuse plus 2d.line.* (regression)

### R-HTML-14a: Permission element interfaces: geolocation, install, usermedia, camera, microphone (HTML*Element classes, InstallResultEvent, handlers)
Commit: 0facc5a71 (integration/round7-batch-1000)
- <geolocation>/<install>/<usermedia>/<camera>/<microphone> no longer HTMLUnknownElement: check tests listing unknown elements
- GlobalEventHandlers gains oninstallresult: check tests with exact handler sets
- event.rs union conflict resolved by hand after the PageReveal/PageSwap commit
- release decision: elements exposed ungated beyond Chrome stable

### R-HTML-14b-landed: Permission element style restrictions (bounded sizes/css, negative offset, invalid css properties, parent mask validity events, children not rendered, icon color) — landed part
Commit: d0803b57b + 0dd6c3a0a + 6bda1ffdf (integration/round7-batch-1000) (d0803b57b needs reviewer compile fix f0c162a8a)
- font hash recomputed after clamping
- UA !important resets: even author !important cannot style background-image/filter/transform etc. on the five permission elements (intended, Chromium)
- invalid-css-properties ×3 depend on getComputedStyle serializing border-image / mask shorthands as "none" and corner shapes as superellipse(1)
- vendor stylo + blitz-dom UPSTREAM.patch regenerated (regen --check passes)
- 0dd6c3a0a: innerText treats permission elements as replaced (children still laid out visually)
- 6bda1ffdf: new IframeChange::PermissionElement post-connection step (validationstatuschange, invalidReason style_invalid under a masked ancestor); per-node tag check in post_connection_subtree on every connected insert

### R-HTML-15a: interestfor slice 1: interest-delay / interest-delay-start/end CSS properties (discrete) in vendored Stylo
Commit: a47ddc9d7 (integration/round7-batch-1000)
- unbuilt stylo derive code: GenericInterestDelay ToCss/ToComputedValue/ToResolvedValue/SpecifiedValueInfo/ToShmem derives, manual ToTyped
- interestfor-css-properties not expected to pass (interpolation rows need continuous animation; properties made discrete)

### R-HTML-16r: sizes attribute parsing: (min-width:calc(0)) media condition in sizes (Stylo media-query change)
Commit: 05e0149f1 (integration/round7-batch-1000)
- @media (min-width: calc(0)) now valid in stylesheets too (Chrome agrees)
- vendored Stylo UPSTREAM.patch regenerated (--check verified)

### R-HTML-20: document.lastModified missing
Commit: 0d5d3c7bc (integration/round7-batch-1000)
- compile: any Resource literal added after 0d5d3c7bc without last_modified fails to build (all on the tip have it at report time)
- value formatted via the realm Date.prototype getters: a page overriding them changes the output

### R-HTML-24b: Enumerated presentational hints ASCII case-insensitive (lists type, embedded/images align): UA-sheet work
Commit: 4bfdd7f5c (integration/round7-batch-1000)
- ol/ul/li type → list-style-type and img/iframe/embed/object/input[type=image] align → vertical-align now change rendering (spec + Chrome); vendored blitz-dom UPSTREAM.patch regenerated

### R-HTML-25: hidden=until-found and beforematch reveal
Commit: b0d21d7f8 (integration/round7-batch-1000)
- 93a1ffbc5 reverted the stylo hint: hidden=until-found is display:none again (animation_events tests keep their basis); un-ignore review_misc2_fragment_navigation_reveals_until_found_with_beforematch after the build

### R-HTML-26r-landed: URL query encoding leftovers (location/navigation/attributes, css/xml/svg includes) + frameElement + iframe-src-encoding — landed part
Commit: c4d9a5064 (integration/round7-batch-1000)
- Location href/assign/replace/[PutForwards] now resolve against the entry realm document (spec); differs from before only for cross-document same-origin calls

### CM1: Canvas: createImageBitmap resolves in a task
Commit: 01b84fe5d (integration/round7-batch-1000)
- 01b84fe5d: element/manual/imagebitmap/createImageBitmap-resolves-in-task.any (window + worker); all createImageBitmap-* (regression)
- SMOKE: a page that draws with fake timers installed before first canvas use

### CM2: Canvas: OffscreenCanvas getContext('2d') converts its settings in dictionary order
Commit: a36597e0b (integration/round7-batch-1000)
- a36597e0b: offscreen/canvas-context/2d.getContext.options.any (window + worker)

### CM2b: Element getContext("2d") converts context settings on every call, in dictionary order (shared canvas2d.js contextSettings)
Commit: 841dd2fad (integration/round7-batch-1000; review follow-up to a36597e0b)
- offscreen 2d.getContext.options.any must keep passing
- colorSpace/pixelFormat use ${raw} DOMString conversion: a Symbol throws TypeError
- WPT: the-canvas-element/2d-getcontext-options.html; offscreen canvas-context/2d.getContext.options.any (window + worker)
- regression: getContextAttributes, colour-space getContext pages, ImageData ctor pixelFormat/colorSpace pages (String() conversions became template literals)

### CM3: Canvas: Filters coloured from a visited link's currentcolor taint the canvas
Commit: 0b04361d1 (integration/round7-batch-1000)
- WPT html/canvas/element/manual/filters/svg-{flood,diffuse,specular,drop-shadow}-filter-visited-color-taints.html
- regression: 2d.filter.* pages with url(#f) filters outside links must stay origin-clean

### CM4: Canvas: drawFocusIfNeeded outlines the path of focused fallback content
Commit: e160f4174 (integration/round7-batch-1000)
- e160f4174: element/manual/drawing-paths-to-the-canvas/drawFocusIfNeeded_{001,004,005}

### R-HTML-31: Text track leftovers (selection task order, metadata, parser-blocked default load timer, active cues, empty/mutable cues, disabled mode, TextTrack activeCues/cues)
Commit: cbc5b3ab3 (integration/round7-batch-1000)
- default track mode now set one task later (queued selectDefaults): check media/track pages that read mode synchronously
- JS re-entry from load()
- track-active-cues counts load + cuechange + canplaythrough == 3; loadText now produces the cuechange itself — a second cuechange before canplaythrough would shift the count

### R-HTML-31b: Text track leftovers that time out (default load timer during parser block, mutable cue fragment, mode disabled)
Commit: c9827e928 (integration/round7-batch-1000)
- a selected track whose fetch never ends now holds canplay/canplaythrough (spec behaviour)

### R-HTML-32-landed: createImageBitmap from canvas/video/SVG ('source image could not be decoded') — landed part
Commit: bd1b6f5ee + 60b40b21e (integration/round7-batch-1000; owner confirmed)
- 60b40b21e: createImageBitmap-bounds, -drawImage-closed; SECURITY: cross-origin no-cors <img> loaded before any canvas use → createImageBitmap → drawImage → getImageData must throw SecurityError
- bd1b6f5ee: imagebitmap-replication-exif-orientation (window, worker, structuredClone); regression: drawImage image-orientation pages (image-orientation: none still unrotated)

### R-HTML-35: Media pause on removal must be queued
Commit: 84b274e4a (…5b822e93) + 259ac1d06 (b8ddcf00) (integration/round7-batch-1000)
- a removed playing element keeps playing until the queued pause task runs
- 259ac1d06 relies on media::flush running synchronously during removeChild

### F16: ImageData pixelFormat rgba-float16 and Web IDL argument order
Commit: 6ec5bdf43 (integration/round7-batch-1000)
- 6ec5bdf43: pixel-manipulation/2d.imageData.object.ctor.{pixelFormat,basics}; 2d.imageData.create*, get*, put* (regression, incl. the colour-space pages from 5da88db3e); structured clone of a float16 ImageData via postMessage; createImageBitmap/drawImage from a float16 ImageData

### R-HTML-B-20: Whitespace-only <script src> fires an error event instead of fetching the document URL
Commit: f4150a963 (integration/round7-batch-1000)
- a src with surrounding whitespace now resolves from the trimmed value (same URL)
- promise-resolution-order also checks unhandledrejection ordering after the error (researcher flagged it tentative)

### R-HTML-AO: location.ancestorOrigins: new object per Location, inactive document, referrerpolicy on unsupported elements
Commit: 4ba31fc7d (integration/round7-batch-1000) + 45add0139 + 7586d0400 + dd48f6082
- ancestorOrigins masking now from the snapshotted container referrerpolicy (7586d0400); navigations the container did not start fall back to the live attribute
- dd48f6082: WPT location-ancestor-origins*.html incl. the A1→B1→B2→A2(no-referrer) = [null,null,A] and about:blank(A)→B1(no-referrer)→B2 = [B,null,null] cases

### R-HTML-JSURL: window.open("javascript:...") runs in the prebuilt popup realm, not the opener
Commit: e06c257ce (integration/round7-batch-1000)
- a string result of such a javascript: URL does not replace the popup document
- 91f25aac4 reverted the 5b02727aa CDP javascript: change: Page.navigate to javascript: behaves as before 5b02727aa

### R-FETCH-META: Fetch metadata request headers (Sec-Fetch-Dest/Mode/Site/User) on element, form, worker, worklet, preload, redirect loads
Commit: None
- 4a8d973cc: new Destination::Worker/SharedWorker/ServiceWorker variants; nosniff check via is_script_like for worker main scripts

### R-FETCH-ORB: ORB/CORB: block opaque cross-origin responses by MIME sniffing (script/img/preload)
Commit: 227a9b4f3 (cross-origin blob: URL not resolved for subresources; author unreported) + dd166b5d8 (nosniff script blocking, …72613886)
- 227a9b4f3: a subresource blob: URL resolves only for a frame of the blob's origin; a document whose frame origin differs from the blob's serialized origin (opaque/about:blank edge) loses blob subresources

### R-FETCH-MISC-landed: Fetch misc single pages: request-init-001/003, range/general, redirects/data, redirect-navigate preserve-fragment, cors-preflight-referrer, content-type/script, redirect-keepalive, CORP in iframes, dangling markup, embedded credentials, canvas remote redirect, split cache — landed part
Commit: 1ccc4238d, 1694192a5, 76e61d4e9, 87f0ec899 (…ae5abf41) + f399664be / 88224f9d8 (range/general, redirects/data), dbfde500e (integration/round7-batch-1000)
- 87f0ec899: redirect without fragment inherits the fragment (live + cached paths); navigation scroll-to-fragment after a redirect now happens (low-medium risk)
- 1ccc4238d: window.caches removed on insecure windows (global + Window.prototype)

### SC-TOM-timing-keywords: Typed OM keeps timing-function keywords (step/ease keywords stay keywords in specified timing functions); separate from cubic-bezier easing
Commit: a332b50f3 (integration/round7-batch-1000; remote verified a332b50f3a10722e37670970ce8f1f802ac9077a)
- both WPT pages (historically 38/40 subtests each)
- unit test typed_step_keywords_stay_keywords_in_specified_timing_functions

### R-HTML-SANDBOX-landed: iframe sandbox enforcement (popups inherit/escape sandbox, allow-top-navigation(-by-user-activation), navigate ancestor/other frame, downloads blocked, forms/popups/document.open/javascript: window.open, blank-document inheritance) — landed part
Commit: 7932deb17, 0b6966d73, 165c3fa32, 917004324, e449e43fd, c73e9e3ff (integration/round7-batch-1000)
- 7932deb17: a sandboxed document's window.open to an existing frame name (not a popup) now also returns null
- 165c3fa32: a CSP sandbox does not make the origin opaque yet (sandbox-inherit-to-blank-document-unsandboxed still needs it)
- 0b6966d73: same-origin (allow-same-origin) sandboxed frames navigating via the ordinary Location not checked yet; no "one permitted sandboxed navigator" exception (a sandboxed frame navigating its own popup counts as blocked)
- e449e43fd: a sandboxed frame without allow-same-origin now gets SecurityError on localStorage/sessionStorage instead of an isolated area (Chromium)

### SC-CSP-paintworklet-eval: CSP paint-worklet evaluation rejection
Commit: ebe91c87e + 6f2c7a9ce (integration/round7-batch-1000)
- cargo test -p pyro-cdp --test engine_cdp --no-fail-fast paint_worklet
- content-security-policy/gen/top.{http-rp,meta}/{script-src-wildcard,worker-src-{none,self,wildcard}}/worklet-paint{,-import-data}.https.html (14)
- regression: worklets/ (paint-worklet-import), css/css-paint-api/; the 12 script-src-self/-wildcard import-data "blocked" cases unchanged

### SC-scoped-VT: Scoped view transitions
Commit: 193dc92e4 (from 80a70a609; integration/round7-batch-1000)
- all css/css-view-transitions (~64 pass today); hardest: scoped/negative-playback-rate, callback-order.tentative, display-none-during-transition, fill-forwards, hit-test*, view-transition-scope-*, reftests nested-scope, scope-during-transition-2, scope-replaced-elements
- css/css-animations + web-animations (shared animation runtime change)
- cargo test pyro-render (tests/mirror.rs), pyro-dom, pyro-webapi

### SC-grid-track-sizing: CSS grid track sizing / resolved values
Commit: c85ec64b1, 47aa9f2f6, 3f3af5dee, c8288f15a, 5ce7623dd, 43e7fcf45 (integration/round7-batch-1000)
- taffy unit tests; pyro-layout --test computed_style and --test flow (offset_parent*)
- all css/css-grid + css/css-align reftests (1/64px percentage-track flooring); cssom-view/offsetParent*

### SC-transitions-cancel: CSS transitions cancel/reverse
Commit: a7b148613, d4e377163, 13625eebb, b71b671ed, 2c095884b (integration/round7-batch-1000)
- pyro-browser --test web_animations; css/css-transitions; web-animations getAnimations order; dom/nodes/moveBefore/continue-css-transition-*; css/css-animations/event-dispatch.tentative

### SC-selectors-user-action: CSS selectors user-action state (:hover/:active/:focus*, top layer, lang, quirks, ::part)
Commit: 56915a791, 88528da0b, ff3e834cd, 128060bcd, c1ea139c5, 64ae9c927, 23ca36d6d, 12c292733, 9b5cf3ec9, aefd14747, 4c2782b4f, 4b98e6a34 (integration/round7-batch-1000)
- cargo test -p pyro-dom -p pyro-webapi; regen-patch --check --all; all css/selectors
- regression dirs: html/interaction/focus, html/semantics/popovers, the-dialog-element, forms, pointerevents, uievents/mouse, quirks/, css/selectors/i18n, css/css-shadow-parts
- known follow-up: :empty not restyled when a text node flips empty/non-empty

### SC-VT-idl-skip: View transitions IDL + skip sequencing
Commit: b3f272201, 0e9ee2c56 (integration/round7-batch-1000)
- pyro-webapi document_root_element_js; pyro-js realm object_realm_gone; all css/css-view-transitions; css-values/attr-invalidation-after-view-transition; css-pseudo *view-transition*

### SC-overflow-zero-axis: Scrollable overflow for boxes empty in one axis
Commit: b84cec3ac (integration/round7-batch-1000)
- MEDIUM RISK: all css/css-overflow + flexbox/grid scrollWidth/scrollHeight pages

### SC-anchor-overflow-limit: Anchor position-area default overflow limit includes original containing block
Commit: e22e68a34 (integration/round7-batch-1000)
- all css/css-anchor-position; taffy oof tests

### SC-snap-directional: Scroll snap: directional scrollBy + snap-stop + oversized targets + logical alignment
Commit: 235c4d421, cbb4d1a73, 6086ada32 (integration/round7-batch-1000)
- all css/css-scroll-snap with --testdriver; pyro-webapi dom_js snap tests

### SC-elementsFromPoint-order: elementsFromPoint lists every box holding the point (Chrome list hit test)
Commit: a320eb224 (integration/round7-batch-1000)
- MUST-PASS regression: css/CSS2/normal-flow/block-in-inline-hittest-002.html (passes today; if it fails, a320eb224 is the cause)
- pyro-layout --test hit_descent and --test flow; shadow-dom DocumentOrShadowRoot-prototype-elementFromPoint

### SC-CQ-C123: Container queries: unknown features, ineligible containers (tables), invalidation on container-type/name change
Commit: bc1d28385, 877dfd082, 1bdb367b5 (integration/round7-batch-1000)
- pyro-render --test container_queries; all css/css-conditional/container-queries

### SC-CQ-scroll-state: scroll-state(scrollable) container queries
Commit: 02e010b51 (integration/round7-batch-1000)
- pyro-render --test container_queries; css/css-conditional/container-queries/scroll-state/ (two reviewer compile fixes folded in)

### SC-wasm-streaming: WebAssembly.compileStreaming/instantiateStreaming (CSP wasm allow cases)
Commit: d01495c1b (Studio) + a8c9d7350 (…68889b70, JSC 0040) (integration/round7-batch-1000)
- content-security-policy/wasm-unsafe-eval: expected to stay failing (async wasm promises never settle)

### SC-scroll-initial-target: scroll-initial-target
Commit: 57023b90c (from 78b05e689; integration/round7-batch-1000)
- the 8 pages + parsing/-invalid must stay passing
- regression: all css/css-scroll-snap; cssom-view/scrollIntoView-* (esp. scrollIntoView-should-treat-slot-as-scroll-container); css-overflow targeted-column-scroll-marker-selection-001, scrollbar-gutter-scroll-into-view; cargo test -p pyro-dom -p pyro-webapi

### SC-snap-events: scroll snap events + re-snap after style change
Commit: 7fb25f233 (from f35d7983e) + bfe194f6c (from 7e6b2213f) (integration/round7-batch-1000)
- the 10 pages incl. scrollsnapchange-with-proximity-strictness (expected to still FAIL)
- regression: all css/css-scroll-snap (no-snap-position, scroll-snap-type*, resnap-on-*, scrollTo-scrollBy-snaps, scroll-snap-stop-*, scroll-initial-target/*)
- cargo test -p pyro-layout --test flow
- work counter: idle mandatory-snap page runs clamp_scroll_offsets once per DOM version

### SC-cv-auto-state: content-visibility auto-state event + anchor relevance
Commit: c0d6f0006 (from 2a28b3de4; integration/round7-batch-1000)
- cargo test pyro-webapi, pyro-render (tests/content_visibility_auto.rs), pyro-dom, pyro-layout, blitz-dom
- css/css-contain/content-visibility/ incl. svg reftests, in-svg-000-crash, with-top-layer-00*
- all css/css-anchor-position (compare count before the change); css-view-transitions/content-visibility-auto-relock-crash
- TODO work counter: idle page with a listener queues no second check

### OOS-imagebitmap-renderingcontext: ImageBitmapRenderingContext (getContext("bitmaprenderer"))
Commit: c289d09c4 (integration/round7-batch-1000)
- alpha:false option ignored (test expects alpha kept, as Chrome)
- bitmaprenderer <canvas> not painted into page screenshots (reads only)
- transferControlToOffscreen after a bitmaprenderer context does not throw
- toBlob-origin-clean also relies on 60b40b21e
- WPT bitmaprenderer pages (html/canvas/element/manual/imagebitmap/*, offscreen/*bitmaprenderer*): transferFromImageBitmap, null reset, detached InvalidStateError, one context per canvas, transferToImageBitmap, convertToBlob
- SECURITY SMOKE: transferFromImageBitmap of a bitmap from a cross-origin <img> → toDataURL throws SecurityError; drawImage(canvas) into a 2D canvas → getImageData throws SecurityError
- known gap: drawImage of a bitmaprenderer canvas from another same-origin iframe draws transparent black

### R-WASM-ASYNC: Async WebAssembly promises never settle (Studio-requested)
Commit: a8c9d7350 + c6295b18e (integration/round7-batch-1000; JSC patch 0040-deferred-work after 0039)
- needs JSC rebuild with 0040; inert without the patch
- worker realms not wired in a8c9d7350 (c6295b18e "settle in workers too" is on the batch, author unreported)

### R-HTML-38: HTMLTableElement tree APIs: caption/tHead/tFoot setters, sectionRowIndex
Commit: 5e371d9d4 (integration/round7-batch-1000)
- caption-methods (18 subtests) may miss a cross-document adoption detail

### BUCKET-embedded-media-canvas-landed: BUCKET-embedded-media-canvas run 1 (lazy-loading worker)
Commit: cd21430ab, be84ba523, 3db74cb02, 8d0dc8a39, 4d224634b, 665f374a3, c5fe0532b, 7697f40b0, 931311668 (integration/round7-batch-1000)
- 8d0dc8a39: 4xx/5xx responses whose body is an image now count as loaded (Chromium)
- cd21430ab: update-the-source-set may need more than this

### BUCKET-forms-covered: BUCKET-forms pages already fixed by earlier batch commits
Commit: 5ed810d48 (Q06: type-change-text-to-file-to-text, type-change-state-weekmonth) + b230f1d34 (R-HTML-6: willValidate output/object) + e9b1d47bc (SubmitEvent submitter type check; author unreported) (integration/round7-batch-1000)
- e9b1d47bc: form-fired submit event is now composed — scripts checking event.composed === false on submit now see true

### BUCKET-forms-landed: BUCKET-forms run 1 (html researcher-implementer)
Commit: e78f7de78, d527b612e, 2e163c50a, 6e4010b84, 0f28fc202, 86d2744a8, dca25d3de, f09e4e33e, 3c82e094d, b5245bbbf, b061fffe0, ecf4666d2 (integration/round7-batch-1000)
- 0f28fc202 (medium risk): :valid/:invalid/:required matching changes for disabled required checkboxes/radios/selects (HTML + Chrome); readonly helper
- d527b612e: labels is a cached live NodeList, null on input[type=hidden]

### SC-CQ-anchored-fallback: anchored(fallback …) container queries + PositionArea::flip_logical_axis (Chrome semantics)
Commit: eb60ce3b2 (from f5d0f0ecf; integration/round7-batch-1000)
- cargo test -p pyro-render --test anchor_positioning --test container_queries
- the 8 css-anchor-position/container-queries pages
- regression: anchored-fallback-{basic,name,scroll,display-change,tree-scope,color-change}, try-tactic-*, last-successful-*, anchor-scroll-position-try-013..015, position-area-scrolling-005..008, position-try-fallbacks-003, position-try-order-logical, position-area-change, position-area-in-position-try, chrome-*-crash
- css-conditional at-container-parsing/-serialization, container-selection-unknown-features, query-evaluation
- if -position-area still fails, test without writing-mode on #target

### SC-anchor-interpolation: anchor() / anchor-size() interpolate in CSS transitions and animations
Commit: c33183561 (from cec3588e2; integration/round7-batch-1000)
- cargo test pyro-dom, pyro-webapi
- must not regress: anchor-animation-iacvt, anchor-transition-002/003/004/focus, position-try-transition-basic/flip, position-try-initial-transition, popover-anchor-backdrop-transition, anchored-child-transition-with-selection, container-queries/anchored-child-transition, anchor-center-fallback-transition-behavior, property-interpolations, chrome-379758203-crash, anchored-transition-display-none-001/002 reftests
- interpolation suites: css-position/animations/*-interpolation, css-logical inset/size, css-sizing width/height/min/max

### SC-smooth-scroll: Smooth scrolling (scroll-behavior / behavior:"smooth") on the rendering-frame clock; scroll promises settle at the end of a smooth scroll
Commit: 8c117ae60 (from 41023a362; integration/round7-batch-1000)
- cargo test -p pyro-dom -p pyro-webapi -p pyro-browser -p pyro-cdp
- css/cssom-view: the 9 + element/window-scroll-promises, -promise-interruption, scroll-promise-after-removal, scroll-arguments, background-change-during-smooth-scroll, position-sticky-root-scroller-with-scroll-behavior, scroll-behavior-smooth-after-hiding-iframe, scrollIntoView-smooth, scrollIntoView-fixed-outside-of-viewport, smooth-scrollIntoView-with-unrelated-gesture-scroll
- css-scroll-snap snap-after-relayout/*, snap-events/* (scrollsnapchange-on-interrupted-scroll, instant-resnap-to-target, resnap-on-oveflow-hidden-container)
- dom/events/scrolling scrollend-*, scrollIntoView-in-onscroll-to-sticky; reftest add-background-attachment-fixed-during-smooth-scroll; crashtest overflow-hidden-smooth-scroll-crash
- all scroll-animations (75 pass today)
- TODO work-count test: 15 steps, no frame requested once idle

### SC-CSP-core-v4: CSP core v4: fetch-check core + script/worker/worklet hooks (a7e2f883e) + dependent directives (457992246)
Commit: a7e2f883e (from 6ee8f1815) + 457992246 (from 8ae418767, merged with 91f25aac4 in bindings.rs) (integration/round7-batch-1000)
- cargo test -p pyro-net csp; pyro-browser --lib preload::tests; --test child_frames csp_; --test preload_scanner stylesheet_blocked_script; pyro-cdp engine_cdp::context_options
- gated must-not-regress: dynamic-import propagate-nonce-*, string-compilation-nonce-*, code-cache-nonce, workers/modules/*-import-csp, trusted-types/, dom/nodes/moveBefore/nonce
- content-security-policy/ (both lists, scriptnonce-changed-1/-2, meta/sandbox-iframe, sandbox/, nonce-hiding/, strict_dynamic*, connect-src/*-import-*, style-src/import-style-*); import-maps/csp/; preload nonce pages; speculative-parsing meta-csp-*
- no-CSP pages unchanged: fetch/api/basic, xhr, eventsource, websockets, css-font-loading; re-check javascript: noopener popup pages (91f25aac4)
- EXPECTED NEW FAILURES matching Chrome 158 (update baseline): unsafe-eval/tentative/eval-allowed-by-hash.sub.html and …-long-script.sub.html

## 6. Guard lists (must not regress)

- `~/Library/Caches/lightningpanda/round7-local/history-nav-guards/iframe-or-traversal.txt` (181 entries)
- `~/Library/Caches/lightningpanda/round7-local/history-nav-guards/passing-today-c3372ad21.txt` (302 entries)
- `~/Library/Caches/lightningpanda/round7-c02-shadow-offcanvas/C15-shared-credited-71.json` (71 entries)
- F02 focusgroup: the 29 pages that already pass (census c3372ad21) are a regression watch.
- WPT gates: full reftest gate, testharness gates, Linux CI.

## 7. Rows to verify that are credited on source reading only

- F05: 0 pages (af91e97df195bee7c1cbb363f62eaa8d9f9d610c (integration/round7-batch-1000; nested dedicated Worker))
- C15-shared: 71 pages (ae993d5b2 (C15) + C05 1933ab2dd, e016f96f4, b957b5caf, 02245b282, fd362425b)
- R-HTML-1: 18 pages (7d5405d12)
- R-HTML-B-1b: 2 pages (75ae37859 (integration/round7-batch-1000; builds on 77316c216))
- R-HTML-B-3r: 2 pages (c9216028f (covered; integration/round7-batch-1000))
- R-HTML-B-7c: 3 pages (ff95c542b, 0b9041595 (+ f10b98964) (integration/round7-batch-1000))
- R-HTML-21: 4 pages (2ba8826df (integration/round7-batch-1000))
- R-HTML-B-18: 0 pages (b9f5a8e4b (integration/round7-batch-1000; opener as event.source))
- R-FETCH-MISC-landed: 7 pages (1ccc4238d, 1694192a5, 76e61d4e9, 87f0ec899 (…ae5abf41) + f399664be / 88224f9d8 (range/general, redirects/data), dbfde500e (integration/round7-batch-1000))
- BUCKET-embedded-frames-covered: 5 pages (b230f1d34 (R-HTML-6), 7247854c9 (integration/round7-batch-1000), dba597871)
- BUCKET-embedded-media-canvas-covered: 7 pages (ee78275fd, acfc8b0e0 (R-HTML-10, integration/round7-batch-1000) + 841e4bfbf, 8a9618149, ffd49eb93)
- BUCKET-forms-covered: 5 pages (5ed810d48 (Q06: type-change-text-to-file-to-text, type-change-state-weekmonth) + b230f1d34 (R-HTML-6: willValidate output/object) + e9b1d47bc (SubmitEvent submitter type check; author unreported) (integration/round7-batch-1000))
- BUCKET-markup-infra-c-covered: 2 pages (ab8a664c4 (…5b822e93), 99c9c0fb3 (author unreported))
- BUCKET-markup-infra-b-covered: 2 pages (4c65e104f (R-HTML-28, form/link rel opener/noopener with named target) (integration/round7-batch-1000))
- R-HTML-SC-RESIZABLE: 4 pages (07194d15f, 194d5c78a, 1812d34a3 (integration/round7-batch-1000; author unreported))
- BUCKET-markup-infra-covered: 1 pages (59f1e5086 (F03c, …0b9ae05a) (integration/round7-batch-1000))
- BUCKET-scripting-events-covered: 12 pages (23a9a7b82, 1df64a1fa, 6a567af8c, 74d54863e, 4f94a5f1f, 400ad99f4, 65b515e5b (integration/round7-batch-1000))
- BUCKET-scripting-events-covered-studio: 2 pages (6659fee08 + 5187a2bb0 (Studio, inline scripts report start-tag line) (integration/round7-batch-1000))
- BUCKET-window-origin-landed-b: 6 pages (4155c5577, ac2cd04f9, 2a82174d0, 2250fb12c (integration/round7-batch-1000; reported as c83f24cd8, 1635f89c6, 1b9abe384, 48a3dedbb pre-rebase))
- C-fontKerning-covered: 3 pages (63b205937 (integration/round7-batch-1000))
- C-singletons-landed: 2 pages (781e7100a (+60b40b21e) + fix 7da6b6939 (integration/round7-batch-1000))

## 8. Excluded or held (not counted)

- Mixed content: 93cf2f04c, reverted by 0729192ae. Fetch-metadata, permissions-policy, SRI, COOP/COEP, security taint pages.
- Policy decisions pending: document.domain (13 pages), cross-origin exotic WindowProxy/Location (JSC), real back-forward cache.
