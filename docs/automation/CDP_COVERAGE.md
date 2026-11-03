# CDP coverage inventory

Sources:
- **Playwright side:** every `send("Domain.method")` in `playwright-core@1.59.1/lib/server/chromium/*.js`, the version TOS pins. This is the static superset Playwright *may* send. The subset TOS actually drives will be recorded through `bench/scripts/cdp-record-proxy.mjs` and added here.
- **Input schema:** [Chrome DevTools Protocol Input domain](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/pdl/domains/Input.pdl).
- **Upstream side:** the method enums of `src/server/cdp/domains/*.zig` at lightpanda-io/browser `2ad38773fd674d3c9cea26a263b814ee46f6ba84` (200 methods across 22 domains). "yes" means upstream dispatches the method; it does not imply full Chrome semantics.

Rewrite status:
- **engine:** implemented over the real JavaScriptCore engine (`crates/lp-cdp/src/engine`). Covered by `crates/lp-cdp/tests/engine_cdp.rs` and `tests/e2e/about-blank.mjs` (real Playwright 1.59.1).
- **accepted, recorded only:** the command succeeds, but nothing in the engine can observe its effect yet. The note says why the success is truthful today and what will make it effective.
- **conditional:** succeeds only when the request is already true of the engine; otherwise it returns an explicit error.
- **pending:** returns an explicit CDP error (`-32000 … is not supported by lightningpanda`) and never a fabricated success.

HTTP and `data:` document loading now use the real loader. `Network` emits request/redirect/response/data/completion/failure events to enabled page sessions, with response-body capture. Browser contexts own distinct network clients. DOM automation node/geometry and file-input commands are implemented; frontend DOM mirrors remain pending; native child-frame discovery, worlds, navigation and frame-owner identity are implemented ([frame scope](FRAMES.md)); basic native Input dispatch is implemented; this is not TOS compatibility. Fetch request-stage interception supports continue overrides, fulfill, abort, disable/detach cleanup and server Basic-auth challenges. Response-stage interception pauses matched responses (lp-net buffers only those) for `getResponseBody`, `continueResponse`, `fulfillRequest` and `failRequest`. Custom status phrases, proxy credentials and non-Basic credential schemes are explicitly unsupported. Fulfillment accepts text headers or base64 NUL-separated binary headers, preserving native value bytes and duplicate entries; the existing Network JSON header display uses UTF-8 replacement for invalid text. Redirect hops have distinct linked Fetch IDs; auth retries retain the hop ID. Only one session per page can own interception; different pages in a context may have separate owners.

Storage cookie commands use the network cookie jar, including HTTP response cookies, expiry and context isolation. Negative CDP expiry values mean session cookies. Invalid batches are validated before modifying the real jar. Partitioned/SameParty cookies, non-default priority and source overrides return explicit errors. Browser WebAPI installation separately supplies the DOM and Web Storage in main and isolated realms.

Capture uses a shared process payload budget (default 64 MiB, 8 MiB/body), at most 4096 response records per page, and bounded network subscriptions (1024 events/8 MiB logical bytes per context). Observer overflow closes the CDP connection instead of silently losing events. Disabling the last Network session, main navigation, or page teardown releases retained bodies; child navigation/removal releases only that frame's captures. Per-session `Network.enable` buffer options are explicitly unsupported. These are logical retention limits, not an RSS measurement; shared chunk allocations and metadata contribute additional memory.

The browser integration smoke also exercises DOM access before init scripts, shared DOM across isolated worlds, page fetch with exact Network frame/loader correlation, route fulfillment/abort after headers, and Web Storage context/page/navigation isolation. These checks use local HTTP and JavaScript evaluation; The full DOM protocol and actual TOS workflows remain unvalidated. A separate Input check covers keyboard typing/text insertion and mouse checkbox activation; editing commands are advisory and pen/touch remain unsupported; native drag-and-drop works under Playwright's drag interception; wheel default scrolling follows the synthetic layout. Mouse coordinates follow the native synthetic layout, not full CSS positioning; the smoke derives its click point from the reported rectangle and verifies the exact hit target.

Methods: 122 (Playwright 1.59.1's static list plus the session methods it sends that the list misses). Upstream implements 95 of them. Rewrite: 84 engine, 10 accepted/recorded only, 1 conditional, 27 pending (counts from the table below).

| Method | Upstream 2ad38773f | Rewrite | Notes |
| --- | --- | --- | --- |
| `Audits.enable` | **no** | accepted, recorded only | no issues are reported |
| `Browser.cancelDownload` | **no** | pending |  |
| `Browser.getVersion` | yes | engine |  |
| `Browser.getWindowBounds` | **no** | pending |  |
| `Browser.getWindowForTarget` | yes | accepted, recorded only | one headless window per page; reports bounds set by setWindowBounds, else the emulated viewport (default 1280x720) |
| `Browser.grantPermissions` | yes | pending |  |
| `Browser.resetPermissions` | yes | pending |  |
| `Browser.setDockTile` | **no** | pending |  |
| `Browser.setDownloadBehavior` | yes | engine | per browser context; under allow/allowAndName, `<a download>` links to blob:, data: or same-origin URLs (blob bytes captured at click, so an immediate revokeObjectURL is fine; HTTP fetched) are written to downloadPath (guid-named for allowAndName) with Chrome's Browser.downloadWillBegin and downloadProgress inProgress/completed (filePath) events on the browser session when eventsEnabled; failures report canceled; deny/default deliver nothing. A navigation answered with Content-Disposition: attachment becomes a download as in Chrome: nothing commits (Page.navigate gets net::ERR_ABORTED; the document request reports Network.loadingFailed net::ERR_ABORTED, canceled; a requested navigation is cleared). Name: Content-Disposition filename* then filename, else the download attribute, else the URL's last segment, else "download" |
| `Browser.setWindowBounds` | yes | accepted, recorded only | stores the bounds, which getWindowForTarget reports back; no page API (outerWidth, screen) observes them yet (upstream: no-op) |
| `CSS.disable` | **no** | pending |  |
| `CSS.enable` | yes | pending |  |
| `CSS.startRuleUsageTracking` | **no** | pending |  |
| `CSS.stopRuleUsageTracking` | **no** | pending |  |
| `DOM.describeNode` | yes | engine | native Node wrappers/backend IDs; bounded depth and node count; no frontend mirror IDs or pierced traversal |
| `DOM.disable` | **no** | engine | stops documentUpdated notifications |
| `DOM.enable` | yes | engine | documentUpdated notifications; frontend mirror/mutation tracking unsupported |
| `DOM.focus` | yes | engine | native focusability check and focus steps |
| `DOM.getBoxModel` | yes | engine | separate content, padding, border and margin quads from the laid-out border/padding/margin widths; the transforms of the element and its containing blocks map them (a rotated box reports its rotated points, as Chrome); `width`/`height` are the untransformed border box; main-frame viewport coordinates, untransformed boxes in layout units. An inline box over several lines reports the bounding box of its line fragments (Chrome uses the line boxes, so its top/bottom can differ by the line gap) and only inline margins |
| `DOM.getContentQuads` | yes | engine | the border-box quad after transforms (Playwright `boundingBox` is its bounding box); an inline box over several lines reports one quad per line, the first starting after its start border/padding and the last ending after its end ones |
| `DOM.getFrameOwner` | yes | engine | native child owner backend ID in the same page session; missing/main frame rejected |
| `DOM.resolveNode` | yes | engine | wraps the node in the requested execution context of its frame (or its main world) |
| `DOM.scrollIntoViewIfNeeded` | yes | engine | in a child frame, also reveals each owning iframe in its parent; connected visible elements over native synthetic layout; `rect` (node-relative area, sent by Playwright for positional clicks) scrolls the window to the nearest edge after revealing the node |
| `DOM.setFileInputFiles` | yes | engine | `files` are paths read from disk (64 MiB each; name from the last path component, type from the extension, `lastModified` from the file's modification time), then trusted `input` and `change` as when a user picks them; a node that is not an `<input type=file>` fails with Chrome's "Node is not a file input element"; `[]` clears the selection |
| `Debugger.disable` | yes | pending |  |
| `Debugger.enable` | yes | pending |  |
| `Debugger.resume` | yes | pending |  |
| `Debugger.setSkipAllPauses` | yes | pending |  |
| `Emulation.setDefaultBackgroundColorOverride` | yes | engine (`render` feature) | screenshots paint `color` (RGBA, `a` defaults to 1) instead of white under the document; `a: 0` gives transparent PNG pixels (straight alpha; JPEG drops alpha); absent color resets |
| `Emulation.setDeviceMetricsOverride` | yes | engine | persistent native width/height/DPR/screen and resize; mobile, transforms and non-native orientation explicitly unsupported |
| `Emulation.setEmulatedMedia` | yes | engine | each call replaces the emulation: `print` media type and `prefers-color-scheme`/`prefers-reduced-motion` drive matchMedia, @media in the style engine, Blitz layout and screenshots/PDFs; other features are accepted and not evaluated |
| `Emulation.setFocusEmulationEnabled` | yes | accepted, recorded only | headless pages always behave as focused |
| `Emulation.setGeolocationOverride` | yes | pending |  |
| `Emulation.setLocaleOverride` | yes | accepted, recorded only | stored per page; navigator.language(s) and Accept-Language follow the locale unless an explicit acceptLanguage override is active; Intl and Date keep the process locale (JavaScriptCore is process-wide); empty or absent resets; a malformed tag is invalid params |
| `Emulation.setScriptExecutionDisabled` | yes | pending |  |
| `Emulation.setTimezoneOverride` | yes | accepted, recorded only | stored per page; Date and Intl keep the process time zone (JavaScriptCore is process-wide); empty resets; an obviously malformed id fails with "Invalid timezone id" (no tz database check) |
| `Emulation.setTouchEmulationEnabled` | yes | conditional | `enabled: false` succeeds (no touch is emulated); `enabled: true` is an explicit error |
| `Emulation.setUserAgentOverride` | yes | engine | page-local navigator and request defaults, persistent across navigation; client hints unsupported |
| `Fetch.continueRequest` | yes | engine | request stage; `interceptResponse: true` pauses the same request again at the response stage (same interception id); see supported scope above |
| `Fetch.continueResponse` | **no** | engine | response stage: the real response, or a new `responseCode`/`responseHeaders` (text or binary) with the real body |
| `Fetch.continueWithAuth` | yes | engine | server Basic challenges; proxy credentials and other schemes unsupported |
| `Fetch.disable` | yes | engine | request and response stages; see supported scope above |
| `Fetch.enable` | yes | engine | request and response stages; see supported scope above |
| `Fetch.failRequest` | yes | engine | request and response stages; see supported scope above |
| `Fetch.fulfillRequest` | yes | engine | request and response stages; see supported scope above |
| `Fetch.getResponseBody` | **no** | engine | a response paused at the response stage (text, else base64); requests paused at the request stage get Chrome's error |
| `HeapProfiler.collectGarbage` | **no** | pending |  |
| `IO.close` | yes | engine (`render` feature) | closes a `printToPDF` stream; reads of a closed handle are invalid params |
| `IO.read` | yes | engine (`render` feature) | reads a `printToPDF` `ReturnAsStream` stream as base64 chunks (`offset`, `size`, default 1 MiB) until `eof` |
| `Input.dispatchDragEvent` | **no** | engine | drives the page's native drag session as Blink does: dragEnter/dragOver fire `drag` on the source, then `dragenter`/`dragleave` on a changed target or `dragover` on the same one; drop fires `drop` (read-only data) when the last dragenter/dragover was canceled, else `drag` + `dragleave`, then `dragend` with the accepted operation; dragCancel fires `dragend` only. Target data follows the CDP items and operations mask (protected mode in dragenter/dragover/dragleave). A dragEnter without a session is a drag entering from outside. Drags carrying `files` and child-frame targets are unsupported |
| `Input.dispatchKeyEvent` | yes | engine | native focused-frame keyboard routing, trusted events and control default actions; raw-key cross-document char suppression. The macOS editing `commands` Playwright derives are advisory: the key's own default action runs as on Linux (arrows with or without Shift, Home/End, Backspace, Delete, Meta+A; Escape delivers events only; clipboard, undo, word movement and Emacs-style aliases do nothing). Timestamps, system keys and alternate metadata are unsupported |
| `Input.dispatchMouseEvent` | yes | engine | hit testing descends into iframes (the point is translated into the innermost frame document, which alone receives the events), trusted mouse/pointer events and activation; `mouseWheel` fires a trusted cancelable `wheel` and, unless canceled, scrolls the nearest container that can still move per axis, else the viewport (wheel/touch listeners on window, document, html and body default to passive); a wheel or move over empty viewport space targets the root element (as Chrome hit-tests it; presses there and points outside the viewport stay errors); moves fire Chrome's boundary events (out, leave inner to outer, over, enter outer to inner, for pointer then mouse); pen and custom pressure unsupported |
| `Input.dispatchTouchEvent` | yes | pending |  |
| `Input.insertText` | yes | engine | native focus/selection and cancellable beforeinput/input |
| `Input.setInterceptDrags` | **no** | engine | while enabled, a left-button `mouseMoved` at least 3px (Chrome's macOS threshold) from a press on a draggable source (`draggable=true`, or a link with href) fires `dragstart` and, unless canceled, `pointercancel` and `Input.dragIntercepted` with the drag data (links prefill uri-list/plain/html as Chrome) and mask. Pointer-driven drags keep plain mouse events. Without interception no native drag starts (no OS drag loop); images are not draggable (no image decoding) |
| `Log.enable` | yes | accepted, recorded only | console goes through Runtime.consoleAPICalled; no Log entries are produced |
| `Network.clearBrowserCache` | yes | engine | empties the HTTP cache of the page's browser context (shared by its pages, as in Chrome 147) via `NetClient::clear_http_cache()`, and, when the opt-in shared public-asset cache is on (`LIGHTNINGPANDA_SHARED_ASSET_CACHE_MB`), its process-wide store via `clear_shared_asset_cache()`; cache hits, including shared ones, report `fromDiskCache: true` with `encodedDataLength: 0` |
| `Network.clearBrowserCookies` | yes | engine | empties the page's browser context's jar |
| `Network.deleteCookies` | yes | engine | by `name` and `url` or `domain`, optionally `path` |
| `Network.emulateNetworkConditions` | yes | engine | `offline` for the page's browser context (Chrome scopes it to the target): requests fail with net::ERR_INTERNET_DISCONNECTED and navigator.onLine is false; latency and throughput are accepted and ignored |
| `Network.enable` | yes | engine | subscribes to bounded per-context events; filters by page/session; buffer options unsupported |
| `Network.getCookies` | yes | engine | `urls` (default: the page's frame URLs) through the page's browser context's jar, as HTTP sees them |
| `Network.getRequestPostData` | yes | engine | request bodies of the page's requests while a session has Network enabled, up to 1 MiB each and 8 MiB per page (oldest dropped); `hasPostData` stays true past 64 KiB |
| `Network.getResponseBody` | yes | engine | text/base64 completed captures; explicit errors for unavailable or evicted bodies |
| `Network.loadNetworkResource` | **no** | pending |  |
| `Network.setCacheDisabled` | yes | engine | per page, as in Chrome 147: the page's requests bypass the per-context HTTP cache (no lookup or store, `Pragma`/`Cache-Control: no-cache` added) while its session has Network enabled; sibling pages of the same context still use the cache |
| `Network.setCookie` | yes | engine | the page's browser context's jar; `{success}` |
| `Network.setCookies` | yes | engine | the page's browser context's jar; all or none, as Storage.setCookies |
| `Network.setExtraHTTPHeaders` | yes | engine | per page (sibling pages, even in the same browser context, are unaffected); each call replaces the map and {} clears it; applied to navigations, subresources, fetch and XHR only while the session has Network enabled (kept while disabled), as in Chrome 147; page headers override same-name request headers including User-Agent; invalid names/values rejected with Chrome's messages |
| `Network.setUserAgentOverride` | yes | engine | same as `Emulation.setUserAgentOverride` |
| `Page.addScriptToEvaluateOnNewDocument` | yes | engine |  |
| `Page.bringToFront` | yes | accepted, recorded only | single foreground page per target |
| `Page.captureScreenshot` | yes | engine (`render` feature) | rendered by lp-render (Blitz: Stylo, Taffy, Parley, vello_cpu) from a snapshot of the live DOM, off the browser thread; `png` and `jpeg` (`quality`), `clip` (document CSS px, `scale`; the viewport keeps its scroll, so fixed elements appear where the viewport shows them, unless `captureBeyondViewport`), `captureBeyondViewport` (whole document); hover and focus styles of the page's pointer and focus; `Emulation.setDefaultBackgroundColorOverride`; `webp`, `fromSurface: false` and `optimizeForSpeed` are ignored or refused; pixels differ from Chromium's (no hinting, Blitz layout); see [SCREENSHOTS.md](SCREENSHOTS.md). Without the feature: an explicit error |
| `Page.close` | yes | engine | answers `{}` at once, then fires `beforeunload` at the main window; a handler that cancels it (`preventDefault()` or a `returnValue`) opens a `beforeunload` dialog answered by `Page.handleJavaScriptDialog` (accept closes, dismiss keeps the page) while a session has Page enabled, and the page closes when none has. Chrome additionally requires a user gesture before it asks; the engine tracks no user activation. `onbeforeunload` returning a string is not honoured (handler return values are not implemented) |
| `Page.createIsolatedWorld` | yes | engine | named isolated worlds per frame, with init scripts for that world |
| `Page.enable` | yes | engine |  |
| `Page.getFrameTree` | yes | engine | nested native frames with per-document loader and actual security origin |
| `Page.getLayoutMetrics` | yes | engine | main frame, CSS px: emulated viewport, window scroll offset and document scroll size (at least the viewport); no pinch zoom, so the visual viewport equals the layout viewport with `scale`/`zoom` 1; the deprecated fields repeat the CSS values, as headless Chromium on macOS reports at DPR 1 and 2. The document size comes from lightningpanda's approximate layout, not the renderer's, so a full-page `clip` built from it can differ from `captureBeyondViewport` |
| `Page.getNavigationHistory` | yes | engine | main-frame session history with stable entry ids; the current entry's `title` is its document's `<title>` (others are empty); `transitionType` is `other`; the first navigation replaces the initial about:blank entry, as in Chrome |
| `Page.handleJavaScriptDialog` | yes | engine | while a session has Page enabled on the page, `alert`/`confirm`/`prompt` suspend the script (Page.javascriptDialogOpening with `hasBrowserHandler: true`) and the connection is served by a nested read loop until this command answers: `confirm` returns `accept`, `prompt` returns `promptText` (empty when omitted) or null on dismiss, then Page.javascriptDialogClosed {frameId, result, userInput}. No page task or timer runs meanwhile; every other command waits in order until the dialog closes (Chrome likewise answers `Runtime.evaluate` on the page only afterwards). The session detaching or disabling Page, `Page.close`, its target closing, its browser context being disposed or the connection ending dismisses it (those commands then run). Without such a session, and during `LP.*` driver commands, dialogs do not block: they return dismissed results (undefined, false, null) and are reported, and this answers such a dialog ({}) or fails with -32602 "No dialog is showing" |
| `Page.navigate` | yes | engine | about:blank, data and HTTP documents; frameId supports native children; explicit failures, no joint child history |
| `Page.navigateToHistoryEntry` | yes | engine | answers once the traversal is queued, like Chrome; same-document entries commit as `navigatedWithinDocument` with `popstate`, others re-fetch the document (no bfcache); unknown ids are invalid params |
| `Page.printToPDF` | yes | engine (`render` feature) | rendered by lp-render from the page's Blitz document after the fonts and images its layout fetches arrive (up to 10 s; the browser thread keeps serving commands, and the PDF is written on the render thread): one PDF page per `.pz-page` box (Paprize), else the document cut into the paper's content box (`paperWidth`/`paperHeight`, `landscape`, margins default 0.4 in, `scale` 0.1-2); `printBackground` (default false) leaves out background colors and images; `pageRanges` ("1-3, 5, 7-") keeps those pages ("Page range syntax error", "Page range exceeds page count"); `displayHeaderFooter`/templates, `preferCSSPageSize`, `generateTaggedPDF` and `generateDocumentOutline` are accepted and not produced; `transferMode: ReturnAsStream` returns an `IO` stream; invalid sizes, margins and scales fail with Chrome's messages. Deviation: the document prints with the page's emulated media (screen unless `Emulation.setEmulatedMedia` set print), not Chrome's print media, because ToS emulates screen. Without the feature: an explicit error |
| `Page.reload` | yes | engine | same limits as Page.navigate |
| `Page.removeScriptToEvaluateOnNewDocument` | yes | engine |  |
| `Page.setBypassCSP` | yes | accepted, recorded only | no CSP enforcement exists |
| `Page.setFontFamilies` | **no** | pending |  |
| `Page.setInterceptFileChooserDialog` | yes | engine | per session; activating an `<input type=file>` (click, label, script `click()`) emits `Page.fileChooserOpened` {frameId, mode selectSingle/selectMultiple, backendNodeId} to intercepting sessions; no native picker exists |
| `Page.setLifecycleEventsEnabled` | yes | engine | per-frame lifecycle events and replay of reached milestones |
| `Page.startScreencast` | **no** | pending |  |
| `Performance.enable` | **no** | accepted, recorded only | no performance timeline events; see `Performance.getMetrics` |
| `Performance.getMetrics` | yes | engine | Timestamp, Documents, Frames, Nodes, LayoutCount and RecalcStyleCount (both the process's Blitz layout syncs, which restyle and lay out what changed), LayoutDuration and RecalcStyleDuration (Blitz layout and style phases, seconds); no JS heap metrics; does not reset LP.geometryStats (`render` feature) |
| `Profiler.disable` | **no** | pending |  |
| `Profiler.enable` | **no** | pending |  |
| `Profiler.startPreciseCoverage` | **no** | pending |  |
| `Profiler.stopPreciseCoverage` | **no** | pending |  |
| `Profiler.takePreciseCoverage` | **no** | pending |  |
| `Runtime.addBinding` | yes | engine | defines a non-enumerable global taking one string in the page's realms (every world, or `executionContextName`'s; `executionContextId` covers that context only), now and in every later realm before init scripts run, so it survives navigation; calls emit `Runtime.bindingCalled` {name, payload, executionContextId} to the sessions that added it (Playwright `exposeFunction`/`exposeBinding`) |
| `Runtime.callFunctionOn` | yes | engine | `generatePreview` adds `Runtime.ObjectPreview` (see `Runtime.evaluate`) |
| `Runtime.enable` | yes | engine |  |
| `Runtime.evaluate` | yes | engine | `generatePreview`: V8's `ObjectPreview` shape (5 own enumerable data properties of an object, 100 elements of an array, Map/Set `size` and 5 `entries`, `[[PrimitiveValue]]`, `[[PromiseState]]`/`[[PromiseResult]]`, `overflow`, `subtype`); DOM nodes preview no prototype accessors and a Proxy is previewed as its target (JavaScriptCore cannot tell a Proxy apart). A script stopped by `Runtime.terminateExecution` answers "Execution was terminated" |
| `Runtime.getProperties` | yes | engine | `ownProperties`, `accessorPropertiesOnly` and `generatePreview`; symbol-keyed properties carry `symbol`; accessors report both `get` and `set` (undefined when missing); `nonIndexedPropertiesOnly`; `internalProperties` unless `accessorPropertiesOnly`: `[[Prototype]]` (null when none), `[[PrimitiveValue]]`, `[[Entries]]` of a Map/Set and a promise's `[[PromiseState]]`/`[[PromiseResult]]` (read through a reaction, which marks a rejected promise handled; inside a running microtask it reads "pending"). Not available from JavaScriptCore's API: a Proxy's `[[Target]]`/`[[Handler]]`, bound functions' `[[TargetFunction]]`, `[[FunctionLocation]]` and `[[Scopes]]` |
| `Runtime.releaseObject` | yes | engine |  |
| `Runtime.removeBinding` | **no** | engine | removes the binding from the page's future realms and deletes the global from live ones once no session of the page uses it |
| `Runtime.runIfWaitingForDebugger` | yes | engine | starts a paused popup's navigation; other targets never start paused |
| `Security.disable` | **no** | engine | no security state is reported |
| `Security.enable` | **no** | engine | no security state events are emitted |
| `Security.setIgnoreCertificateErrors` | yes | engine | on a page session: that page's browser context; on the browser session (Puppeteer `acceptInsecureCerts`): every context, including later ones. HTTPS then accepts any server certificate (handshake signatures still verified); otherwise an untrusted certificate fails with net::ERR_CERT_AUTHORITY_INVALID |
| `Storage.clearCookies` | yes | engine | per-context native cookie jar; omitted context uses a distinct default context |
| `Storage.getCookies` | yes | engine | per-context native cookie jar; omitted context uses a distinct default context |
| `Storage.setCookies` | yes | engine | per-context native cookie jar; omitted context uses a distinct default context |
| `Target.attachToBrowserTarget` | yes | engine | flattened browser sessions support explicit page attachment and scoped replies/events; per-browser-session auto-attach/discovery and nested browser sessions are unsupported |
| `Target.attachToTarget` | yes | engine |  |
| `Target.closeTarget` | yes | engine |  |
| `Target.createBrowserContext` | yes | engine | `proxyServer` is refused |
| `Target.createTarget` | yes | engine | engine-opened popups (`target=_blank`/named links, `window.open` with `noopener`) become targets in the opener's context with `openerId`, paused (`waitingForDebugger`) under auto-attach; `window.open` with opener access throws (no cross-page WindowProxy); named targets always open a new page |
| `Target.detachFromTarget` | yes | engine |  |
| `Target.disposeBrowserContext` | yes | engine |  |
| `Target.getTargetInfo` | yes | engine |  |
| `Target.setAutoAttach` | yes | engine |  |
| `Tracing.end` | **no** | pending |  |
| `Tracing.start` | **no** | pending |  |

Session methods not in Playwright's list but implemented: `Page.setDocumentContent` (document.open/write/close on the existing document in the frame's main world: URL, loader, window and contexts stay; `Page.lifecycleEvent` init with the same loaderId, then DOMContentLoaded/load; no frameNavigated; init scripts do not run again), `Runtime.setAsyncCallStackDepth` (accepted; no async stacks are recorded), `Runtime.terminateExecution` (the connection's watchdog check stops a running script when it arrives). `Runtime.queryObjects` fails explicitly: JavaScriptCore's API has no heap iteration. `Runtime.consoleAPICalled` arguments that are objects carry a preview and every call carries the caller's synchronous `stackTrace` (captured only while a session has Runtime enabled), as in Chrome (Playwright's `ConsoleMessage.location()`); frames of code run through `Runtime.evaluate` have no URL or line in JavaScriptCore.

Browser-level methods not in Playwright's list but implemented: `Target.getTargets`, `Target.getBrowserContexts`, `Target.setDiscoverTargets` (page filters honored), `Target.activateTarget` (validates the target), `Runtime.releaseObjectGroup`, `Emulation.clearDeviceMetricsOverride`, `Page.disable`, `Runtime.disable`, `Network.disable`. Session methods outside that static list include `Fetch.continueWithAuth` (server Basic) and the Accessibility methods below.

## Deliberate differences from upstream

- **Browser contexts:** several contexts may coexist in one process. Upstream allows only one, which is why TOS starts one process per context.
- **No `STARTUP` placeholder page:** none is attached on `Target.setAutoAttach`. Upstream fakes one for Puppeteer.
- **Context teardown:** all contexts are disposed when the connection closes.
- **`about:blank` realm:** it has an executable realm. The pinned upstream build does not, and TOS works around that.

## Upstream-only surface not yet tracked for the rewrite

Upstream also implements domains Playwright does not send:
- `LP.*`: markdown, semantic tree, agent helpers.
- `WebMCP.*`.
- `Debugger.*` beyond enable/disable, backed by v8_inspector.
- `DOM.getDocument`, `DOM.querySelector*` and `DOM.getOuterHTML`.

These are out of scope for the first TOS milestone. They stay listed as uncovered until they are implemented.

## Accessibility

Playwright 1.59.1 sends no Accessibility commands (`getByRole` and `ariaSnapshot` run in the injected script); Puppeteer's `page.accessibility` and `aria/` selectors and other CDP clients do.

| Method | Upstream 2ad38773f | Rewrite | Notes |
| --- | --- | --- | --- |
| `Accessibility.enable` / `disable` | **no** | accepted, recorded only | the trees are computed per command, so there is nothing to start or stop |
| `Accessibility.getFullAXTree` | yes | engine | `depth` and `frameId` as in Chrome (depth counts unignored levels; the root is always expanded) |
| `Accessibility.getPartialAXTree` | **no** | engine | `nodeId` (frontend) unsupported; `backendNodeId`/`objectId`; `fetchRelatives` adds children and ancestors; nodes outside the tree come back ignored (`notRendered`/`uninteresting`) |
| `Accessibility.queryAXTree` | yes | engine | `accessibleName`/`role` exact match in the DOM subtree, document order; like Chrome, aria-hidden matches are returned ignored |

Each call walks the document once and builds Chrome's node shape from the driver's ARIA role and accname code (the one `getByRole` uses): `RootWebArea` root with `frameId`, `<html>`/`<body>` and layout-only blocks ignored (`uninteresting`), unsemantic inline elements flattened, `StaticText` per rendered text run, `display:none` left out, `aria-hidden`/`inert` subtrees ignored, and the properties Chrome reports (focusable, focused, disabled, invalid, editable, settable, hasPopup, level, multiselectable, orientation, multiline, readonly, required, value range, checked, expanded, modal, pressed, selected, describedby, labelledby, url, live regions). `nodeId` is the backend node id as a string. Nothing is tracked between calls. Compared against Chrome 1:1 on fixture pages; intentional differences: no `InlineTextBox` nodes (Chrome's StaticText `childIds` list them; ours are empty), no synthesized nodes (list markers, the `<select>` popup, text-field inner editors, `<input type=submit>` label text), no name `sources` or `chromeRole`, shadow trees and child-frame contents are not inlined (an `<iframe>` is an `Iframe` leaf, as in Chrome's per-frame trees), `<q>` quote pseudo-content and `<meter>` internals are not modelled, `term` is unnamed (Playwright's accname), ARIA roles are not demoted to `generic` outside their required context (a lone `treeitem` stays `treeitem`), `aria-hidden` content is always kept as ignored nodes (Chrome drops some of it), and `<pre>` whitespace is collapsed. Backend node ids are bounded to 65536 live entries per page, so a larger tree fails explicitly.

## Native DOM automation scope

`DOM.describeNode`, `resolveNode`, `getBoxModel`, `getContentQuads`, `scrollIntoViewIfNeeded` and
`focus` use native nodes and binding worlds. `resolveNode` can adopt a backend node into another
world of the same page. IDs are connection-unique, bounded to 65536 per page, generation-checked,
and do not retain nodes or old documents. Descriptions allow depths 0–32 with at most 1024 nodes.
Frontend node IDs, full-subtree depth -1, pierced traversal, frontend mirrors and mutation events
are unsupported. Enabling DOM supplies documentUpdated notifications on navigation.

Geometry is Blitz's layout (lp-render): box-model quads come from its border, padding and margin
widths and the transforms of the element's containing-block chain. Scroll rectangle overrides are explicitly
unsupported. Detached/hidden elements reject geometry/scroll commands. Raw handle owner-frame
and bounding-box checks pass through real Playwright. With the native MutationObserver dependency,
the retained locator fill/click regression also passes unchanged. This focused local fixture does
not establish general locator or TOS compatibility.

Root-frame `DOM.scrollIntoViewIfNeeded` uses the native changed-target list to queue trusted
scroll events for moved overflow ancestors inside-out, followed by the document when its
viewport moved. Container events do not bubble; the document event does. Repeating an unchanged
scroll emits nothing. The layout scope is vertical start reveal and horizontal nearest-edge
reveal, not full CSS alignment or child-frame geometry. The nested-scroll protocol regression
also checks that subsequent focus needs no further scrolling.

Protocol commands invoke `Browser::checkpoint()` after releasing command borrows and before event
flush. This starts resources inserted by synchronous Runtime calls while idle and drains native
events from Input/DOM actions. The Playwright stylesheet check awaits a real local HTTP response
without a page timer or follow-up CDP command to trigger loading.

`Emulation.setUserAgentOverride` validates CDP field types and delegates atomic HTTP header
validation to the browser/native identity API. An empty userAgent alone resets the override;
with acceptLanguage/platform it keeps the default UA while applying those fields. Current and
future worlds and navigation/fetch/XHR use the page override without changing sibling pages.
Already-started requests retain their snapshot. userAgentMetadata is explicitly unsupported
before mutation. This is navigator/request identity, not Intl locale or timezone emulation.
The [CDP Emulation schema](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/pdl/domains/Emulation.pdl)
is the parameter reference; browser persistence and transport limits are documented in
[USER_AGENT.md](../browser/USER_AGENT.md).

Device metrics validate integer dimensions in 0–10,000,000 and finite, nonnegative DPR before
changing the browser. Zero width/height/DPR restores that field's native default; clear resets
metrics while preserving unrelated media/input preferences. Existing and new worlds, init scripts,
navigation, screen dimensions and current media-query matches observe the page's stored viewport.
CSS dimension changes produce a native coalesced resize event; sibling pages stay independent.
There is no separate visible OS window, so dontSetVisibleSize does not alter native metrics.

Mobile layout and additional transform/visual-viewport options fail explicitly before mutation.
Until native orientation overrides exist, an explicit screenOrientation is accepted only when
its angle is zero and its primary type already matches the resulting native screen dimensions.
In particular, Playwright's desktop portrait packet that requests landscapePrimary is unsupported;
this limitation must not be mistaken for complete page.setViewportSize support. MediaQueryList
change notifications and full CSS/mobile layout remain native binding gaps. The browser viewport
contract is documented in [VIEWPORT.md](../browser/VIEWPORT.md).
