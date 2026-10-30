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

HTTP and `data:` document loading now use the real loader. `Network` emits request/redirect/response/data/completion/failure events to enabled page sessions, with response-body capture. Browser contexts own distinct network clients. DOM automation node/geometry commands are implemented; frontend DOM mirrors and file-input commands remain pending; native child-frame discovery, worlds, navigation and frame-owner identity are implemented ([frame scope](FRAMES.md)); basic native Input dispatch is implemented; this is not TOS compatibility. Fetch request-stage interception supports continue overrides, fulfill, abort, disable/detach cleanup and server Basic-auth challenges. Response-stage interception, custom status phrases, proxy credentials and non-Basic credential schemes are explicitly unsupported. Fulfillment accepts text headers or base64 NUL-separated binary headers, preserving native value bytes and duplicate entries; the existing Network JSON header display uses UTF-8 replacement for invalid text. Redirect hops have distinct linked Fetch IDs; auth retries retain the hop ID. Only one session per page can own interception; different pages in a context may have separate owners.

Storage cookie commands use the network cookie jar, including HTTP response cookies, expiry and context isolation. Negative CDP expiry values mean session cookies. Invalid batches are validated before modifying the real jar. Partitioned/SameParty cookies, non-default priority and source overrides return explicit errors. Browser WebAPI installation separately supplies the DOM and Web Storage in main and isolated realms.

Capture uses a shared process payload budget (default 64 MiB, 8 MiB/body), at most 4096 response records per page, and bounded network subscriptions (1024 events/8 MiB logical bytes per context). Observer overflow closes the CDP connection instead of silently losing events. Disabling the last Network session, main navigation, or page teardown releases retained bodies; child navigation/removal releases only that frame's captures. Per-session `Network.enable` buffer options are explicitly unsupported. These are logical retention limits, not an RSS measurement; shared chunk allocations and metadata contribute additional memory.

The browser integration smoke also exercises DOM access before init scripts, shared DOM across isolated worlds, page fetch with exact Network frame/loader correlation, route fulfillment/abort after headers, and Web Storage context/page/navigation isolation. These checks use local HTTP and JavaScript evaluation; The full DOM protocol and actual TOS workflows remain unvalidated. A separate Input check covers keyboard typing/text insertion and mouse checkbox activation; editing commands and pen/touch remain unsupported; native drag-and-drop works under Playwright's drag interception; wheel default scrolling follows the synthetic layout. Mouse coordinates follow the native synthetic layout, not full CSS positioning; the smoke derives its click point from the reported rectangle and verifies the exact hit target.

Playwright 1.59.1 methods: 101. Upstream implements 81 of them. Rewrite: 53 engine, 7 accepted/recorded only, 2 conditional, 39 pending (counts from the table below).

| Method | Upstream 2ad38773f | Rewrite | Notes |
| --- | --- | --- | --- |
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
| `DOM.getFrameOwner` | yes | engine | native child owner backend ID in the same page session; missing/main frame rejected |
| `DOM.scrollIntoViewIfNeeded` | yes | engine | in a child frame, also reveals each owning iframe in its parent; connected visible elements over native synthetic layout; `rect` (node-relative area, sent by Playwright for positional clicks) scrolls the window to the nearest edge after revealing the node |
| `DOM.setFileInputFiles` | yes | pending |  |
| `Debugger.disable` | yes | pending |  |
| `Debugger.enable` | yes | pending |  |
| `Debugger.resume` | yes | pending |  |
| `Debugger.setSkipAllPauses` | yes | pending |  |
| `Emulation.setDefaultBackgroundColorOverride` | yes | pending |  |
| `Emulation.setDeviceMetricsOverride` | yes | engine | persistent native width/height/DPR/screen and resize; mobile, transforms and non-native orientation explicitly unsupported |
| `Emulation.setEmulatedMedia` | yes | accepted, recorded only | stored; mapping to native media preferences remains pending |
| `Emulation.setFocusEmulationEnabled` | yes | accepted, recorded only | headless pages always behave as focused |
| `Emulation.setGeolocationOverride` | yes | pending |  |
| `Emulation.setLocaleOverride` | yes | conditional | succeeds only when the requested locale equals the engine (process) locale; otherwise an explicit error |
| `Emulation.setScriptExecutionDisabled` | yes | pending |  |
| `Emulation.setTimezoneOverride` | yes | conditional | succeeds only when it equals the process time zone; otherwise an explicit error |
| `Emulation.setTouchEmulationEnabled` | yes | pending |  |
| `Emulation.setUserAgentOverride` | yes | engine | page-local navigator and request defaults, persistent across navigation; client hints unsupported |
| `Fetch.continueRequest` | yes | engine | request stage; see supported scope above |
| `Fetch.disable` | yes | engine | request stage; see supported scope above |
| `Fetch.enable` | yes | engine | request stage; see supported scope above |
| `Fetch.failRequest` | yes | engine | request stage; see supported scope above |
| `Fetch.fulfillRequest` | yes | engine | request stage; see supported scope above |
| `HeapProfiler.collectGarbage` | **no** | pending |  |
| `IO.close` | yes | pending |  |
| `IO.read` | yes | pending |  |
| `Input.dispatchDragEvent` | **no** | engine | drives the page's native drag session as Blink does: dragEnter/dragOver fire `drag` on the source, then `dragenter`/`dragleave` on a changed target or `dragover` on the same one; drop fires `drop` (read-only data) when the last dragenter/dragover was canceled, else `drag` + `dragleave`, then `dragend` with the accepted operation; dragCancel fires `dragend` only. Target data follows the CDP items and operations mask (protected mode in dragenter/dragover/dragleave). A dragEnter without a session is a drag entering from outside. Drags carrying `files` and child-frame targets are unsupported |
| `Input.dispatchKeyEvent` | yes | engine | native focused-frame keyboard routing, trusted events and control default actions; raw-key cross-document char suppression. The macOS editing commands Playwright derives are accepted only where the engine's default action matches: ArrowLeft/Right/Up/Down with or without Shift (Up/Down in single-line text inputs and textareas), Backspace, Delete, Meta+A, and Escape (`cancelOperation`, events only). Other commands and alternate metadata are unsupported |
| `Input.dispatchMouseEvent` | yes | engine | hit testing descends into iframes (the point is translated into the innermost frame document, which alone receives the events), trusted mouse/pointer events and activation; `mouseWheel` fires a trusted cancelable `wheel` and, unless canceled, scrolls the nearest container that can still move per axis, else the viewport (wheel/touch listeners on window, document, html and body default to passive); a wheel or move over empty viewport space targets the root element (as Chrome hit-tests it; presses there and points outside the viewport stay errors); moves fire Chrome's boundary events (out, leave inner to outer, over, enter outer to inner, for pointer then mouse); pen and custom pressure unsupported |
| `Input.dispatchTouchEvent` | yes | pending |  |
| `Input.insertText` | yes | engine | native focus/selection and cancellable beforeinput/input |
| `Input.setInterceptDrags` | **no** | engine | while enabled, a left-button `mouseMoved` at least 3px (Chrome's macOS threshold) from a press on a draggable source (`draggable=true`, or a link with href) fires `dragstart` and, unless canceled, `pointercancel` and `Input.dragIntercepted` with the drag data (links prefill uri-list/plain/html as Chrome) and mask. Pointer-driven drags keep plain mouse events. Without interception no native drag starts (no OS drag loop); images are not draggable (no image decoding) |
| `Log.enable` | yes | accepted, recorded only | console goes through Runtime.consoleAPICalled; no Log entries are produced |
| `Network.clearBrowserCache` | yes | engine | empties the HTTP cache of the page's browser context (shared by its pages, as in Chrome 147) via `NetClient::clear_http_cache()`, and, when the opt-in shared public-asset cache is on (`LIGHTNINGPANDA_SHARED_ASSET_CACHE_MB`), its process-wide store via `clear_shared_asset_cache()`; cache hits, including shared ones, report `fromDiskCache: true` with `encodedDataLength: 0` |
| `Network.emulateNetworkConditions` | yes | pending |  |
| `Network.enable` | yes | engine | subscribes to bounded per-context events; filters by page/session; buffer options unsupported |
| `Network.getResponseBody` | yes | engine | text/base64 completed captures; explicit errors for unavailable or evicted bodies |
| `Network.loadNetworkResource` | **no** | pending |  |
| `Network.setCacheDisabled` | yes | engine | per page, as in Chrome 147: the page's requests bypass the per-context HTTP cache (no lookup or store, `Pragma`/`Cache-Control: no-cache` added) while its session has Network enabled; sibling pages of the same context still use the cache |
| `Network.setExtraHTTPHeaders` | yes | engine | per page (sibling pages, even in the same browser context, are unaffected); each call replaces the map and {} clears it; applied to navigations, subresources, fetch and XHR only while the session has Network enabled (kept while disabled), as in Chrome 147; page headers override same-name request headers including User-Agent; invalid names/values rejected with Chrome's messages |
| `Page.addScriptToEvaluateOnNewDocument` | yes | engine |  |
| `Page.bringToFront` | yes | accepted, recorded only | single foreground page per target |
| `Page.captureScreenshot` | yes | engine (`render` feature) | rendered by lp-render (Blitz: Stylo, Taffy, Parley, vello_cpu) from a snapshot of the live DOM, off the browser thread; `png` and `jpeg` (`quality`), `clip` (document CSS px, `scale`), `captureBeyondViewport` (whole document); `webp`, `fromSurface: false` and `optimizeForSpeed` are ignored or refused; pixels differ from Chromium's (no hinting, Blitz layout); see [SCREENSHOTS.md](SCREENSHOTS.md). Without the feature: an explicit error |
| `Page.close` | yes | pending |  |
| `Page.enable` | yes | engine |  |
| `Page.getFrameTree` | yes | engine | nested native frames with per-document loader and actual security origin |
| `Page.getLayoutMetrics` | yes | engine | main frame, CSS px: emulated viewport, window scroll offset and document scroll size (at least the viewport); no pinch zoom, so the visual viewport equals the layout viewport with `scale`/`zoom` 1; the deprecated fields repeat the CSS values, as headless Chromium on macOS reports at DPR 1 and 2. The document size comes from lightningpanda's approximate layout, not the renderer's, so a full-page `clip` built from it can differ from `captureBeyondViewport` |
| `Page.getNavigationHistory` | yes | engine | main-frame session history with stable entry ids; `title` is empty and `transitionType` is `other`; the initial about:blank entry is kept by the first navigation (Chrome replaces it) |
| `Page.handleJavaScriptDialog` | yes | pending |  |
| `Page.navigate` | yes | engine | about:blank, data and HTTP documents; frameId supports native children; explicit failures, no joint child history |
| `Page.navigateToHistoryEntry` | yes | engine | answers once the traversal is queued, like Chrome; same-document entries commit as `navigatedWithinDocument` with `popstate`, others re-fetch the document (no bfcache); unknown ids are invalid params |
| `Page.printToPDF` | yes | pending |  |
| `Page.reload` | yes | engine | same limits as Page.navigate |
| `Page.removeScriptToEvaluateOnNewDocument` | yes | engine |  |
| `Page.setBypassCSP` | yes | accepted, recorded only | no CSP enforcement exists |
| `Page.setFontFamilies` | **no** | pending |  |
| `Page.setInterceptFileChooserDialog` | yes | engine | per session; activating an `<input type=file>` (click, label, script `click()`) emits `Page.fileChooserOpened` {frameId, mode selectSingle/selectMultiple, backendNodeId} to intercepting sessions; no native picker exists |
| `Page.setLifecycleEventsEnabled` | yes | engine | per-frame lifecycle events and replay of reached milestones |
| `Page.startScreencast` | **no** | pending |  |
| `Profiler.disable` | **no** | pending |  |
| `Profiler.enable` | **no** | pending |  |
| `Profiler.startPreciseCoverage` | **no** | pending |  |
| `Profiler.stopPreciseCoverage` | **no** | pending |  |
| `Profiler.takePreciseCoverage` | **no** | pending |  |
| `Runtime.addBinding` | yes | pending |  |
| `Runtime.callFunctionOn` | yes | engine |  |
| `Runtime.enable` | yes | engine |  |
| `Runtime.evaluate` | yes | engine |  |
| `Runtime.getProperties` | yes | engine |  |
| `Runtime.releaseObject` | yes | engine |  |
| `Runtime.runIfWaitingForDebugger` | yes | engine | starts a paused popup's navigation; other targets never start paused |
| `Security.setIgnoreCertificateErrors` | yes | pending |  |
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

Browser-level methods not in Playwright's list but implemented: `Target.getTargets`, `Target.getBrowserContexts`, `Target.setDiscoverTargets` (page filters honored), `Target.activateTarget` (validates the target), `Runtime.releaseObjectGroup`, `Emulation.clearDeviceMetricsOverride`, `Page.disable`, `Runtime.disable`, `Network.disable`. Session methods outside that static list include `Fetch.continueWithAuth` (server Basic).

## Deliberate differences from upstream

- **Browser contexts:** several contexts may coexist in one process. Upstream allows only one, which is why TOS starts one process per context.
- **No `STARTUP` placeholder page:** none is attached on `Target.setAutoAttach`. Upstream fakes one for Puppeteer.
- **Context teardown:** all contexts are disposed when the connection closes.
- **`about:blank` realm:** it has an executable realm. The pinned upstream build does not, and TOS works around that.

## Upstream-only surface not yet tracked for the rewrite

Upstream also implements domains Playwright does not send:
- `LP.*`: markdown, semantic tree, agent helpers.
- `WebMCP.*`.
- `Accessibility.*`: `getFullAXTree`, `queryAXTree`.
- `Debugger.*` beyond enable/disable, backed by v8_inspector.
- `Network` cookie helpers.
- `Fetch.continueResponse` and `Fetch.getResponseBody`.
- `DOM.getDocument`, `DOM.querySelector*` and `DOM.getOuterHTML`.

These are out of scope for the first TOS milestone. They stay listed as uncovered until they are implemented.

## Native DOM automation scope

`DOM.describeNode`, `resolveNode`, `getBoxModel`, `getContentQuads`, `scrollIntoViewIfNeeded` and
`focus` use native nodes and binding worlds. `resolveNode` can adopt a backend node into another
world of the same page. IDs are connection-unique, bounded to 4096 per page, generation-checked,
and do not retain nodes or old documents. Descriptions allow depths 0–32 with at most 1024 nodes.
Frontend node IDs, full-subtree depth -1, pierced traversal, frontend mirrors and mutation events
are unsupported. Enabling DOM supplies documentUpdated notifications on navigation.

Geometry is the existing synthetic native layout. Its single rectangle supplies every box-model
quad; this is not full CSS margin/border/padding layout. Scroll rectangle overrides are explicitly
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
