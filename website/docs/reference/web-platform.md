# Web platform support

What a page running in Pyro can use: JavaScript language features, DOM and HTML, CSS and layout, networking, storage, workers, media and the other Web APIs, with the known gaps. Pyro follows headless Chrome where Chrome and the specification differ.

This page summarizes the repository's own records (`docs/PARITY.md`, `docs/COMPATIBILITY.md`, `docs/webapi/*`, `docs/browser/*`). Where those records are older than the code, the presence of an interface was checked against the published 0.0.33 browser, and the newer result is given. For the CDP protocol surface see [CDP coverage](/reference/cdp); for the Playwright API surface see [API coverage](/playwright/api-coverage).

## Status values {#status}

| Status | Meaning |
| --- | --- |
| **Yes** | Implemented and tested; gaps are edge cases. |
| **Partial** | A useful subset; the notes name what is missing. |
| **No** | Not available (the global is `undefined` or the feature does nothing). |

## Web platform tests {#wpt}

A pinned subset of [web-platform-tests](https://github.com/web-platform-tests/wpt) runs on every pull request in CI (`.github/workflows/wpt.yml`, scripts in `bench/wpt`). The gate fails a pull request that loses a passing subtest or breaks a page that completed. The numbers below come from the committed baseline (`bench/wpt/baseline.json`, WPT revision `a421f2fe`, last updated 2026-11-10). They count the pages the baseline records (pages that pass at least one subtest or complete), tested on the fork build (patched JavaScriptCore).

| WPT directory | Pages | Subtests passing |
| --- | --- | --- |
| `dom` | 625 | 59,689 / 60,737 (98.3%) |
| `html/dom` | 246 | 64,067 / 65,412 (97.9%) |
| `css/selectors` | 228 | 5,158 / 5,803 (88.9%) |
| `shadow-dom` | 185 | 8,700 / 8,877 (98.0%) |
| `encoding` | 1,258 | 1,139,158 / 1,139,218 (100.0%) |
| `url` | 48 | 9,990 / 9,999 (99.9%) |
| `xhr` | 146 | 429 / 469 (91.5%) |
| `fetch/api/basic` | 9 | 90 / 96 (93.8%) |
| `uievents` | 17 | 115 / 116 (99.1%) |
| `webstorage` | 51 | 1,271 / 1,288 (98.7%) |

The subset does not cover every area on this page; areas outside it are tested by the repository's own suites.

## JavaScript engine {#javascript}

Pages run on JavaScriptCore (the WebKit/Safari engine), so the language is Safari's: ES2025+, `Intl`, `Temporal`, `WebAssembly`, `WeakRef`/`FinalizationRegistry`.

Two engine builds exist (see [Platforms](/reference/versions#platforms)): the **patched JavaScriptCore** (macOS arm64 and both Linux builds) and the **system framework** (macOS Intel). Features marked "patched" below need the first.

| Feature | Status | Notes |
| --- | --- | --- |
| ES modules | Yes | Static and dynamic `import`, top-level await, `import.meta.url`, `modulepreload`, SRI `integrity` on module scripts. Module responses must have a JavaScript, JSON or CSS MIME type. |
| Import maps | Partial (patched) | `imports`, `scopes`, trailing-slash packages, merging later maps, bare specifiers, `import.meta.resolve`. Only parser-inserted inline maps; maps added by script or `document.write` and external maps are ignored; `integrity` in a map is not enforced; a rejected map fires no `error` event. The system framework ignores import maps with a console warning. |
| JSON and CSS module scripts | Yes (patched) | `with { type: "json" }` and `with { type: "css" }` (a constructed `CSSStyleSheet`); a typed import fails with a TypeError on the system framework. CSS module `url()`s resolve against the document, not the module. |
| Timers | Yes | `setTimeout`/`setInterval` with the HTML 4 ms nesting clamp (windows only, not workers), `queueMicrotask`, `requestAnimationFrame`, `requestIdleCallback`. |
| `unhandledrejection` | Yes | The event fires (checked on 0.0.33; older records say it did not). |
| Console | Partial | Levels, `count`, `time*`; format specifiers such as `%s` are not applied. |
| `Function.prototype.toString` on interface objects | Differs | Reports `function () { [native code] }`. |
| `Debugger`/`Profiler` (inspector) | No | |

### Runtime deviations {#runtime-deviations}

Deliberate, tested deviations (from `docs/COMPATIBILITY.md`):

| Deviation | Effect |
| --- | --- |
| Window global shape | `window` is JSC's default global: Window members are own properties and `Object.getPrototypeOf(window)` is `Object.prototype`. `window instanceof Window` still holds and the tag is `[object Window]`. Patching `EventTarget.prototype.addEventListener` does not change `window.addEventListener`. |
| Interface objects are Proxies | `Node`, `HTMLElement` and the other interface objects are Proxies whose members are installed on first use. Descriptors and inheritance match; property operations on interface objects (including `instanceof`) pay a proxy-trap cost. |
| Termination | Code that catches a termination (`Runtime.terminateExecution`, a timeout) inside native frames is terminated again within about 50 ms. |
| Inline module URLs | Inline module scripts get `import.meta.url` = `<document URL>#inline-module-N`. |
| Deferred modules and `DOMContentLoaded` | `DOMContentLoaded` waits for each deferred module to finish evaluating, including top-level await. Browsers wait only for the fetch. |
| Root-relative module URLs (system framework) | `import "/x.js"` loads from the page origin but reports `import.meta.url` as `file:///x.js`. |
| Named window properties | `window.foo` finds elements by `id`/`name` (patched only), but the names are not listed by `Object.getOwnPropertyNames`. |

## DOM and HTML {#dom}

| Area | Status | Notes |
| --- | --- | --- |
| DOM tree, `Node`, `Element`, `Document`, `Attr` | Yes | Mutation records, templates, `Attr` as a Node. Character data is UTF-8: lone UTF-16 surrogates are not preserved. |
| HTML parser | Yes | html5ever, incremental around scripts; `document.write` from parser-reached scripts parses at the insertion point (writes from async, deferred or module scripts are ignored). |
| Selectors | Yes | Stylo's matcher, `:has()` with state invalidation, `:state()`, `:focus-visible`, `:user-valid`/`:user-invalid`. |
| Custom elements | Partial | `define`, upgrades, all lifecycle callbacks, form-associated elements with `ElementInternals`, `CustomStateSet`. No customized built-ins (`is=`), no `formStateRestoreCallback`. |
| Shadow DOM | Yes | Declarative roots, `getHTML`, `setHTMLUnsafe`, `Document.parseHTMLUnsafe`, named and manual slot assignment, `delegatesFocus`. |
| Range, StaticRange, Selection | Partial | Range per the DOM spec; `StaticRange` exists; `AbstractRange` does not; Selection is not backed by a live Range. |
| NodeIterator, TreeWalker | Yes | |
| Collections | Partial | `HTMLCollection`, `NodeList`, `NamedNodeMap`, `DOMTokenList` follow the legacy platform object rules where the JSC C API allows; index descriptors report `enumerable: false` and `defineProperty` on an index does not throw. |
| HTML attribute reflection | Yes | All reflection kinds of WPT `html/dom/reflection-*`, obsolete elements included. Accessors only: `<track>` and `<frame>` load nothing. |
| `Node.moveBefore` | Yes | Preserves focus, iframes, modal dialogs and popovers. |
| Tables | Partial | `insertRow`/`deleteRow`/`insertCell`/`deleteCell` and section helpers; the `caption`/`tHead`/`tFoot` setters are missing. |
| XPath | Yes | XPath 1.0, all axes and the 27 core functions, `document.evaluate`, `XPathEvaluator`/`XPathExpression`/`XPathResult`; 32-level nesting cap. |
| DOMParser, XMLSerializer | Partial | HTML and bounded XML 1.0 with namespaces; DTDs and XML 1.1 return a `parsererror` document. |
| XML documents, XSLT | Partial / No | XML documents load and stop at each script (0.0.31); `XSLTProcessor` is undefined. |

## Events and input {#events}

| Area | Status | Notes |
| --- | --- | --- |
| Event dispatch | Yes | Activation behaviour, retargeting across shadow trees, `window.event`. |
| Event handler attributes | Yes | `on*` IDL attributes and `setAttribute('on…')`; markup handlers such as `<button onclick="…">` set through `innerHTML` run (checked on 0.0.33; older records say parser-created handlers were not compiled). |
| Mouse, keyboard, wheel, touch | Yes | `MouseEvent`, `PointerEvent`, `KeyboardEvent`, `TouchEvent`, `DragEvent`, pointer capture, Chrome's touch compatibility events. Input usually comes from CDP or the SDK. |
| Focus | Yes | Tab order over shadow trees and slots, positive `tabindex`, `delegatesFocus`, focus scrolling, `:focus-visible`. |
| Clipboard and editing | Partial | Typing, deletion, caret and word movement with platform modifiers, `Selection.modify()`, multi-click selection, copy/cut/paste with trusted `ClipboardEvent`, `execCommand` and `queryCommand*`, undo/redo, `caretRangeFromPoint`/`caretPositionFromPoint`, `navigator.clipboard` (in memory per context). Lines are logical (no soft wraps); `execCommand('paste')` is unavailable to script, as in Chromium. |

## Forms {#forms}

| Area | Status | Notes |
| --- | --- | --- |
| Value sanitization, `valueAsNumber`/`valueAsDate`, `stepUp`/`stepDown` | Yes | All input types, including date/time kinds. |
| Constraint validation | Yes | Every `ValidityState` flag, Chrome's messages for common cases, interactive validation, `:user-valid`/`:user-invalid`, `:in-range`/`:out-of-range`. |
| Submission | Yes | `FormData` (with submitter), the `formdata` event, `method=dialog`, implicit submission on Enter, legacy encodings and `accept-charset`. POST to a new window fails explicitly. |
| Keyboard defaults | Partial | Number, range, radio groups, single `<select>` (one-character typeahead). No UI pickers, no multi-character typeahead. |
| File inputs | Yes | Set through CDP/SDK uploads; `FileList`, `File`, `FileReader`. |

## CSS, layout and rendering {#css}

Styles are computed by Stylo (Firefox's style engine) and laid out by Blitz and Taffy (block, inline, flex, grid, tables). Screenshots and PDFs are painted on the CPU. See [Rendering](/guide/rendering).

| Area | Status | Notes |
| --- | --- | --- |
| Cascade, computed style | Yes | `getComputedStyle` including pseudo-elements, custom properties, `matchMedia`, `CSS.supports`. |
| Layout geometry | Yes | `getBoundingClientRect`, `offset*`, `client*`, scrolling, `scrollIntoView` through nested scrollers, hit testing for input. |
| `innerText` | Yes | Follows computed `display` (blockification, tables, `display: contents`). |
| CSSOM | Partial | `cssRules` parses on first access; `CSSStyleRule` exists. The `media` and `disabled` CSSOM APIs are missing and `cssRules` is a live array, not a real `CSSRuleList` (per `docs/browser/STYLESHEETS.md`). Constructed stylesheets and `adoptedStyleSheets` work. |
| Linked stylesheets | Yes | Load with `load`/`error` events, block parser scripts and `load`, SRI enforced. Per document: 128 stylesheets, 4 MiB per response, 16 MiB total source. |
| Web fonts | Partial | `@font-face` with `unicode-range` selection (0.0.31). `FontFace` and `document.fonts`' `FontFaceSet` constructor are undefined; `document.fonts` exists. |
| Web Animations | Partial | `Element.animate`, `Animation` and `KeyframeEffect` exist; completeness is not recorded. |
| Top layer | Yes | Modal dialogs and popovers paint above the document with `::backdrop`. |
| `visualViewport` | Yes | Present on 0.0.33. Explicit screen orientation and `MediaQueryList` change events are missing. |

## Networking {#networking}

| Area | Status | Notes |
| --- | --- | --- |
| HTTP/1.1 and HTTP/2 | Yes | rustls with Mozilla's roots; HTTP/2 by ALPN (one connection per origin and context, 100 concurrent streams). No HTTP/3, no cleartext h2c, no server push. |
| `fetch`, `Request`, `Response`, `Headers` | Yes | Streaming bodies, abort, `keepalive`, `referrer`/`referrerPolicy`, SRI. Over HTTP/2 `statusText` is empty. The `Request` constructor works on 0.0.33 (older records list it as missing). |
| `XMLHttpRequest` | Yes | Sync and async, upload and download progress, `responseXML`, timeouts. Synchronous requests skip Fetch interception (Playwright routes do not see them). |
| `WebSocket` | Yes | Windows and dedicated workers, permessage-deflate, WebSockets over HTTP/2 (RFC 8441). Visible to Playwright's `page.on('websocket')`. |
| `EventSource`, `navigator.sendBeacon` | Yes | |
| Cookies | Yes | RFC 6265bis parsing, public suffix list, per-context jar. `CookieStore` is undefined. |
| CORS, Referrer Policy, Fetch Metadata | Yes | CORS with preflight, all eight referrer policies (default `strict-origin-when-cross-origin`), `Sec-Fetch-*` headers. Full Fetch response filtering is not claimed. |
| HTTP cache | Yes | Private in-memory cache per context (RFC 9111 subset, Fetch cache modes). Controlled by CDP `Network.setCacheDisabled` and `Network.clearBrowserCache`. |
| Authentication | Partial | Opt-in Basic challenges for servers and proxies; no credential cache or auth UI. |
| Proxies | Partial | HTTP forward proxy per context or `serve --proxy` (CONNECT for https). SOCKS and `https://` proxies are refused. |
| `file:` URLs | Yes | Navigations and the subresources of `file:` documents; not reachable from http(s) pages or through fetch/XHR. `PYRO_FILE_URLS=0` turns them off. |
| `blob:` and `data:` URLs | Yes | `blob:` loads through fetch/XHR and as script, stylesheet, image and iframe source. |
| URL, URLSearchParams | Yes | Current WHATWG URL Standard; every case of WPT `urltestdata.json`, `setters_tests.json` and `toascii.json` passes. `URLPattern` is undefined. |
| Encoding | Yes | `TextEncoder`/`TextDecoder` (streaming), legacy encodings for forms. |
| Downloads | Partial | With CDP `Browser.setDownloadBehavior` (or the SDK `download` step): `<a download>` links to `blob:`, `data:` or same-origin URLs, and navigations answered with `Content-Disposition: attachment`, are saved with Chrome's download events. `Browser.cancelDownload` is missing. See [CDP coverage](/reference/cdp). |
| CSP, cross-origin isolation | Partial | Content-Security-Policy is only honoured for hiding the `nonce` attribute of script-inserted elements (no other CSP enforcement is recorded). `crossOriginIsolated` is always false (no COOP/COEP). |

## Storage {#storage}

All storage is in memory: nothing is written to disk, and a new browser context starts empty unless seeded (see [`serve --local-storage`](/reference/serve) and [Profiles](/guide/profiles)).

| Area | Status | Notes |
| --- | --- | --- |
| `localStorage`, `sessionStorage` | Yes | Per context and origin (local) and per top-level page (session); per-origin quota. No cross-document `storage` events. Opaque origins get per-document ephemeral storage instead of Chrome's SecurityError. |
| IndexedDB | Yes | IndexedDB 3.0 with `getAll(options)`/`getAllRecords`, per context and origin, shared with dedicated workers. Blob/File values throw DataCloneError; opaque origins throw SecurityError. Works with Dexie, idb, localForage, Firebase. |
| Cache Storage | Yes | `caches` per context and origin, shared by documents and workers; requires a non-opaque origin. |
| `navigator.storage` | Partial | `estimate()` (fixed 10 GiB quota), `persisted()`/`persist()` resolve false; no `getDirectory()`. |
| Web Locks, File System Access | No | `navigator.locks` and `showOpenFilePicker` are undefined. |

## Workers and messaging {#workers}

| Area | Status | Notes |
| --- | --- | --- |
| Dedicated workers | Yes | Own thread and VM; `importScripts`, fetch, WebSocket, IndexedDB, streams, crypto, OffscreenCanvas, transfers both ways; visible as CDP worker targets. Module workers need the patched JavaScriptCore. Worker timers are not clamped. |
| Service workers | Partial | Registration per context in memory, install/activate, `skipWaiting`, `clients.claim`, `fetch` events for page fetches, XHR, subresources and navigations, Cache Storage, MSW 2.x browser mode. Missing: requests of workers a controlled page starts, redirects answered by the worker for navigations, streamed worker responses, navigation preload, Push/Sync, persistence. `navigator.serviceWorker` exists only in secure contexts. |
| Shared workers | No | `SharedWorker` is undefined. |
| `MessageChannel`, `postMessage` | Yes | Across frames, popups and workers, with transfer of ports, streams, `ImageBitmap`, `OffscreenCanvas`. No port `close` event. |
| `BroadcastChannel` | Yes | Across the pages of a browser context. |
| `structuredClone` | Yes | Ordinary graphs, Error/DOMException, Blob/File, CryptoKey, ImageBitmap, transferables. |
| Streams | Yes | Readable (default and byte), Writable, Transform, text and compression streams (gzip, deflate, deflate-raw), transferable. |

## Crypto {#crypto}

| Area | Status | Notes |
| --- | --- | --- |
| `crypto.getRandomValues`, `randomUUID` | Yes | |
| `crypto.subtle` | Yes | Digest, HMAC, AES-GCM/CBC/CTR/KW, PBKDF2, HKDF, ECDSA/ECDH (P-256/384/521), Ed25519/X25519, RSASSA-PKCS1-v1_5/RSA-PSS/RSA-OAEP; raw/spki/pkcs8/jwk; wrap/unwrap. No Ed448/X448. Exposed regardless of secure context. |

## Canvas, images and media {#media}

| Area | Status | Notes |
| --- | --- | --- |
| Canvas 2D | Yes | Rasterized on the CPU: paths, gradients, patterns, composite operators, shadows, text, `drawImage`, image data, `toDataURL`/`toBlob` (PNG/JPEG/WebP), `Path2D`, `DOMMatrix`, `ImageData`. Canvas `filter` and SVG image sources are missing. Canvases appear in screenshots. |
| OffscreenCanvas, ImageBitmap | Yes | Also in workers; `transferControlToOffscreen`. |
| WebGL | No | `WebGLRenderingContext` is undefined. |
| Images | Yes | `<img>` loading, `decode()`, `new Image()`. |
| `<audio>`, `<video>` | Partial | Container metadata (MP4, WebM, Ogg, WAV, MP3, FLAC, ADTS), ready-state events, playback on the page clock, Chrome's autoplay policy. Video frames decode on demand for screenshots and `drawImage` (AV1, H.264; no VP8/VP9/HEVC). No audio output. `MediaSource` exists but `isTypeSupported('video/mp4')` returns false. |
| Web Audio | Partial | `AudioContext` (a clock, no output), `OfflineAudioContext` with buffer source, oscillator, constant source and gain nodes, `decodeAudioData` (MP3, AAC, Vorbis, Opus, FLAC, WAV). No other nodes. |
| `MediaRecorder`, WebRTC, speech | No | `MediaRecorder`, `RTCPeerConnection` and `speechSynthesis` are undefined. |

## Observers and timing {#observers}

| Area | Status | Notes |
| --- | --- | --- |
| `MutationObserver` | Yes | All options, microtask delivery. |
| `IntersectionObserver` | Partial | Thresholds, `root`, `rootMargin`, clipping and scrolling; some options throw NotSupportedError. Records describe this as computed over an approximate layout. |
| `ResizeObserver` | Partial | One size for all boxes (the `box` option is ignored, as in upstream Lightpanda). |
| `PerformanceObserver`, timeline | Yes | User Timing, Paint Timing (`first-paint`/`first-contentful-paint` at the first rendering opportunity), Resource Timing, Navigation Timing, `performance.timing`. |

## Navigation, frames and windows {#navigation}

| Area | Status | Notes |
| --- | --- | --- |
| Location and History | Yes | Every `Location` setter, `pushState`/`replaceState`, `popstate` and `hashchange` in Chrome's order, `<meta http-equiv=refresh>` and the `Refresh` header. |
| Navigation API | Yes | `window.navigation`, `navigate` events with `intercept()`, entries, traversal promises. |
| Page lifecycle | Yes | `pagehide`, `visibilitychange`, `unload` when a document is left; `beforeunload` on `Page.close`. |
| iframes | Partial | Same-process child frames for `src`, `srcdoc` and `about:blank`, sandbox flags, per-frame realms, cross-origin `WindowProxy` restrictions, `postMessage` between frames. At most 128 child frames per page and 32 levels deep. No out-of-process frames, no joint session history, no `loading=lazy`. |
| `window.open`, popups | Yes | Opens a page in the opener's context with `window.opener`, named reuse and `window.close()`; Playwright's `popup` event fires. |
| Dialogs (`alert`, `confirm`, `prompt`) | Yes | Block for `Page.handleJavaScriptDialog` while a CDP session has Page enabled; otherwise dismissed and reported. |
| `<dialog>`, popovers, invokers, `<details>` | Yes | Modal dialogs and inertness, popover stacks with light dismiss, `command`/`commandfor`, exclusive accordions. |

## Identity and environment {#identity}

| Area | Default | Notes |
| --- | --- | --- |
| User agent | `Mozilla/5.0 (compatible; pyro/<version>)` | Override per page through CDP `Emulation.setUserAgentOverride` (Playwright `userAgent`); `serve` has no user-agent flag. `navigator.userAgentData` and low-entropy `Sec-CH-UA` headers follow the override's metadata. Some sites serve different assets to this UA than to Chrome's. |
| Viewport | 1280x720 at DPR 1 over CDP (observed with the SDK on 0.0.33) | Set per page through CDP `Emulation.setDeviceMetricsOverride` or the SDK/Playwright `viewport`. A size change fires `resize`. |
| Locale | Process locale | `navigator.language` and `Accept-Language` follow the per-page locale; `Intl` and `Date` keep the process locale. |
| Timezone | Process timezone | One timezone per connection: a different zone in a second context of the same connection fails. |
| Permissions | Prompt | Per context through `Browser.grantPermissions`; `Notification` shows nothing (fires `show` when granted). `navigator.geolocation` exists; geolocation emulation is not recorded. |
| Secure context | Computed | `https:`, `localhost`, loopback, `file:`, `about:blank`, `data:` are potentially trustworthy. |

## Not available {#not-available}

Checked as `undefined` on 0.0.33: `SharedWorker`, `WebGLRenderingContext`/`WebGL2RenderingContext`, `RTCPeerConnection`, `MediaRecorder`, `speechSynthesis`, `XSLTProcessor`, `URLPattern`, `CookieStore`, `WebTransport`, `FontFace`, `FontFaceSet`, `AbstractRange`, `TrustedHTML`, `Sanitizer`, `TextTrack`, `Gamepad`, `EyeDropper`, `navigator.locks`. Also absent: WebDriver BiDi, Push and Background Sync, disk persistence of any storage.

If a page needs one of these, check [Troubleshooting](/guide/troubleshooting) and [Platforms and limits](/guide/platforms).
