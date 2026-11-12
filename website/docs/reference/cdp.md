# CDP coverage

Pyro speaks the Chrome DevTools Protocol over WebSocket. It implements the subset that Playwright's Chromium driver and Puppeteer send, over its own engine (JavaScriptCore, a Rust DOM and Blitz layout). This page lists every method the dispatcher handles, by domain, and the events it emits.

Every method not listed here returns an explicit error. Nothing is faked as a success (see [Unknown and unsupported methods](#unknown-and-unsupported-methods)). For Pyro's own domains, see [Pyro CDP domain](/reference/cdp-domain) (prepared pages, browser level) and [LP.run wire format](/reference/lp-run) (`LP.*`, page level).

**Status key**

| Status | Meaning |
| --- | --- |
| implemented | Backed by the engine. Notes list the limits that differ from Chrome. |
| partial | Works for the common case. Some parameters or modes return errors or are ignored (see notes). |
| accepted | Returns `{}`, but nothing in the engine observes it. The success is still truthful, because the thing it would control does not exist (for example, there are no breakpoints to skip). |

## Connecting

Start a server with [`lightningpanda serve`](/reference/serve) (default `127.0.0.1:9222`). It serves:

| Endpoint | Response |
| --- | --- |
| `GET /json/version` | `{"Browser": "Chrome/124.0.6367.29", "Protocol-Version": "1.3", "User-Agent": "…", "V8-Version": "", "WebKit-Version": "", "webSocketDebuggerUrl": "ws://HOST:PORT/"}`. `HOST` is [`--advertise-host`](/reference/serve#advertise-host), else `--host`. |
| `GET /json`, `GET /json/list` | `[]`. A fresh connection has no pages to list. |
| WebSocket upgrade on any path | A CDP connection. In fork mode, the query `?profile=N` asks for a fork of profile N's snapshot ([details](/reference/cdp-domain#connecting-to-a-profile-s-fork)). |

`PUT` is accepted like `GET`. Other paths return 404. Other methods return 405.

Security checks on every request:

- The `Host` header must be an IP literal or `localhost` (with an optional port). Anything else gets `403`, a guard against DNS rebinding. Connect by IP when the server is remote.
- WebSocket upgrades that carry an `Origin` header get `403`, so web pages in a normal browser cannot drive the server.

Each WebSocket connection is a separate browser: its own engine thread, browser contexts and targets. Every context is disposed when the connection closes. The number of connections and the message sizes are capped by [`--cdp-max-connections`](/reference/serve#cdp-max-connections) and [`--cdp-max-message-size`](/reference/serve#cdp-max-message-size).

```ts
// Playwright (stock): connect over CDP
import { chromium } from 'playwright';
const browser = await chromium.connectOverCDP('http://127.0.0.1:9222');

// Puppeteer
import puppeteer from 'puppeteer-core';
const browser2 = await puppeteer.connect({ browserWSEndpoint: 'ws://127.0.0.1:9222/' });
```

The Playwright drop-in (`@povio/lightning-panda/playwright`, see [Coming from Playwright](/playwright/overview)) and the [SDK](/reference/sdk) connect for you. For Puppeteer details, see [Puppeteer and raw CDP](/scraping/cdp).

Sessions use flat mode only. `Target.attachToTarget` and `Target.setAutoAttach` without `flatten: true` fail with `Only flatten mode is supported`. Per-connection budgets: 256 browser contexts, 256 targets, 512 sessions.

## Unknown and unsupported methods

| Case | Error |
| --- | --- |
| Domain or method the engine does not implement | `{"code": -32000, "message": "<Method> is not supported by lightningpanda"}` |
| Page method sent on the browser session (no `sessionId`), for example `Page.navigate` | same `-32000 … is not supported by lightningpanda`. Attach to the target first. |
| Unknown `Lightningpanda.*` method, or a method name without a `.` | `{"code": -32601, "message": "'<Method>' wasn't found"}` |
| Unknown `sessionId` | `{"code": -32001, "message": "Session with given id not found."}` |
| Bad params | `-32602` with the parse or validation message (Chrome's text where Chrome has one) |

Known unsupported methods that Playwright 1.59 may send: `Browser.cancelDownload`, `Browser.getWindowBounds`, `Browser.setDockTile`, `Emulation.setScriptExecutionDisabled`, `HeapProfiler.collectGarbage`, `Network.loadNetworkResource`, `Page.setFontFamilies`, `Page.startScreencast`, `Tracing.start`, `Tracing.end`. `Runtime.queryObjects` also fails explicitly, because JavaScriptCore's API has no heap iteration.

## Browser-level methods

These are sent without a `sessionId`, or on a browser session from `Target.attachToBrowserTarget`.

### Target

| Method | Status | Notes |
| --- | --- | --- |
| `Target.setDiscoverTargets` | implemented | Page and `tab` filters honored. |
| `Target.setAutoAttach` | implemented | Flat mode only. Attaches pages (Playwright) or `tab` targets (Puppeteer) as the filter asks. Popups start paused (`waitingForDebugger`) until `Runtime.runIfWaitingForDebugger`. Not available on browser sessions from `attachToBrowserTarget`. |
| `Target.getTargets` | implemented | |
| `Target.getTargetInfo` | implemented | |
| `Target.getBrowserContexts` | implemented | The default context is not listed, as in Chrome. |
| `Target.createBrowserContext` | implemented | Several contexts per connection, each with its own network client, cookies and storage. `proxyServer` takes one HTTP proxy (CONNECT for https, absolute-form for http; `direct://` for none), and `proxyBypassList` takes Chrome's syntax (`<local>`, `<-loopback>`, CIDR, wildcards). SOCKS, `https://` proxies and per-scheme lists are refused. |
| `Target.disposeBrowserContext` | implemented | The default context cannot be disposed. |
| `Target.createTarget` | implemented | Page targets only. Page-opened popups (`target=_blank`, named targets, `window.open`) become targets in the opener's context with `openerId`. There is no cross-page WindowProxy: `window.open` returns a minimal object (`closed`, `close()`, `focus()`, `blur()`, `location`). |
| `Target.closeTarget` | implemented | |
| `Target.attachToTarget` | implemented | Flat mode only. |
| `Target.detachFromTarget` | implemented | By `sessionId` or `targetId`. |
| `Target.activateTarget` | implemented | Validates the target. There is nothing to bring forward. |
| `Target.attachToBrowserTarget` | implemented | Flat browser sessions with explicit page attachment. Nested browser sessions and per-session auto-attach or discovery are unsupported. |

### Browser, Storage, Security

| Method | Status | Notes |
| --- | --- | --- |
| `Browser.getVersion` | implemented | `product` `Chrome/124.0.6367.29`, `protocolVersion` `1.3`, Chrome-like `userAgent`. `revision` and `jsVersion` are empty. |
| `Browser.setDownloadBehavior` | implemented | Per browser context. `deny`, `default`, `allow`, `allowAndName`. The allow modes need `downloadPath`. `<a download>` to blob:, data: or same-origin URLs, and navigations answered with `Content-Disposition: attachment`, are written to disk. `Browser.downloadWillBegin` and `Browser.downloadProgress` are emitted when `eventsEnabled` is set. |
| `Browser.grantPermissions` | implemented | Per context (the default context without `browserContextId`). Drives `navigator.permissions.query`, live `PermissionStatus` change events and the context clipboard. Chrome's messages for unknown types. |
| `Browser.resetPermissions` | implemented | Back to Chrome's defaults (`clipboard-write` granted, the rest `prompt`). |
| `Browser.setPermission` | implemented | One permission for one origin, or for every origin. |
| `Storage.getCookies` / `setCookies` / `clearCookies` | implemented | The context's real cookie jar (HTTP cookies included). Negative `expires` means a session cookie. Invalid batches are rejected before anything changes. Partitioned and SameParty cookies, non-default priority and source overrides are refused. |
| `Security.enable` / `disable` | accepted | No security state events. |
| `Security.setIgnoreCertificateErrors` | implemented | On the browser session (Puppeteer `acceptInsecureCerts`), it covers every context, later ones included. On a page session, it covers that page's context. Handshake signatures are still verified. Otherwise an untrusted certificate fails with `net::ERR_CERT_AUTHORITY_INVALID`. |
| `Runtime.runIfWaitingForDebugger` | accepted | On a browser session. |

### Pyro

`Lightningpanda.capabilities`, `registerPreparedPage`, `acquirePreparedPage`, `preparedPages`, `unregisterPreparedPage`, `adoptPreparedPage`, `preparedProfiles`. See [Pyro CDP domain](/reference/cdp-domain).

## Page-session methods

These are sent with the `sessionId` of an attached page.

### Page

| Method | Status | Notes |
| --- | --- | --- |
| `Page.enable` / `disable` | implemented | |
| `Page.navigate` | implemented | about:blank, data: and HTTP(S) documents. `frameId` can target child frames. A download response fails the navigation with `net::ERR_ABORTED`. |
| `Page.reload` | implemented | Same limits as `navigate`. |
| `Page.getFrameTree` | implemented | Nested frames with loader ids and security origins. |
| `Page.setLifecycleEventsEnabled` | implemented | Per-frame lifecycle events. Milestones already reached are replayed. |
| `Page.addScriptToEvaluateOnNewDocument` / `removeScriptToEvaluateOnNewDocument` | implemented | |
| `Page.createIsolatedWorld` | implemented | Named isolated worlds per frame. |
| `Page.getNavigationHistory` | implemented | Main-frame history. Only the current entry has a `title`. `transitionType` is always `other`. |
| `Page.navigateToHistoryEntry` | implemented | Same-document entries fire `popstate`. Others re-fetch (no bfcache). |
| `Page.getLayoutMetrics` | implemented | CSS px from Pyro's layout. No pinch zoom: the visual viewport equals the layout viewport. |
| `Page.captureScreenshot` | implemented | Rendered on the CPU by the Blitz renderer. `png` and `jpeg` (`quality`), `clip` with `scale`, `captureBeyondViewport`. `webp`, `fromSurface: false` and `optimizeForSpeed` are ignored or refused. Pixels differ from Chromium's. See [Rendering](/guide/rendering). |
| `Page.printToPDF` | implemented | Paper size, `landscape`, margins, `scale` (0.1–2), `printBackground`, `pageRanges`, `generateTaggedPDF`, links, `transferMode: ReturnAsStream`. Paged by `.pz-page` boxes (Paprize) or by cutting the document at Chrome-like break points. `displayHeaderFooter` and its templates, `preferCSSPageSize` and `generateDocumentOutline` are accepted and ignored. The page prints with its emulated media (screen unless set to `print`), not Chrome's print media. See [Rendering](/guide/rendering). |
| `Page.handleJavaScriptDialog` | implemented | `alert`, `confirm`, `prompt` and `beforeunload` block while a session has Page enabled. During `LP.*` driver commands dialogs are dismissed instead, unless `LP.dialogs {block: true}` is set. |
| `Page.setInterceptFileChooserDialog` | implemented | Emits `Page.fileChooserOpened`. |
| `Page.setDocumentContent` | implemented | `document.open`, `write`, `close` on the current document. URL and contexts stay. No `frameNavigated`. |
| `Page.close` | implemented | Fires `beforeunload`. A cancelling handler opens a dialog. |
| `Page.setBypassCSP` | accepted | There is no CSP enforcement. |
| `Page.bringToFront` | accepted | Each page is always in the foreground. |

### Runtime

| Method | Status | Notes |
| --- | --- | --- |
| `Runtime.enable` / `disable` | implemented | Execution context, console and exception events. |
| `Runtime.evaluate` | implemented | `returnByValue`, `awaitPromise`, `generatePreview` (V8's `ObjectPreview` shape), `objectGroup`. A JavaScriptCore Proxy is previewed as its target. |
| `Runtime.callFunctionOn` | implemented | |
| `Runtime.getProperties` | implemented | `ownProperties`, `accessorPropertiesOnly`, `nonIndexedPropertiesOnly`, `generatePreview`, and the internal properties JavaScriptCore exposes. No `[[Target]]`/`[[Handler]]` for a Proxy, no `[[Scopes]]`, no `[[FunctionLocation]]`. |
| `Runtime.releaseObject` / `releaseObjectGroup` | implemented | |
| `Runtime.addBinding` / `removeBinding` | implemented | Survives navigation. Emits `Runtime.bindingCalled` (Playwright `exposeFunction`). |
| `Runtime.runIfWaitingForDebugger` | implemented | Starts a paused popup's navigation. |
| `Runtime.terminateExecution` | implemented | Stops a running script. It answers "Execution was terminated". |
| `Runtime.setAsyncCallStackDepth` | accepted | No async stacks are recorded. |
| `Runtime.queryObjects` | unsupported | Explicit error. |

### DOM

| Method | Status | Notes |
| --- | --- | --- |
| `DOM.enable` / `disable` | implemented | Only `DOM.documentUpdated`. No frontend mirror or mutation events. |
| `DOM.describeNode` | partial | Backend node ids, depth 0–32, at most 1024 nodes. No frontend `nodeId`s, no depth `-1`, no `pierce`. |
| `DOM.resolveNode` | implemented | Into any world of the node's frame. |
| `DOM.getBoxModel` / `getContentQuads` | implemented | From Blitz layout, with transforms. Inline boxes report one quad per line. |
| `DOM.scrollIntoViewIfNeeded` | implemented | Reveals through iframes. Honors `rect`. |
| `DOM.focus` | implemented | |
| `DOM.getFrameOwner` | implemented | |
| `DOM.setFileInputFiles` | implemented | Paths read from disk (64 MiB each). Fires trusted `input` and `change`. |

`DOM.getDocument`, `DOM.querySelector*`, `DOM.getOuterHTML` and the rest of the frontend DOM domain are unsupported. Use `Runtime.evaluate` or `LP.*` locators.

### Input

| Method | Status | Notes |
| --- | --- | --- |
| `Input.dispatchMouseEvent` | implemented | Hit testing through iframes. Trusted mouse and pointer events, boundary events, wheel scrolling. No pen and no custom pressure. |
| `Input.dispatchKeyEvent` | implemented | Trusted keyboard events with default actions. The macOS editing `commands` are advisory: the key's own default action runs (arrows, Home/End, Backspace, Delete, Meta+A). Clipboard, undo and word movement do nothing. |
| `Input.insertText` | implemented | Cancelable `beforeinput` and `input`. |
| `Input.dispatchTouchEvent` | implemented | Chrome's pointer, touch and compat-mouse sequence. A tap clicks. Touch never scrolls the page. |
| `Input.dispatchDragEvent` | partial | Native drag sessions as in Blink. Drags carrying `files` and child-frame targets are unsupported. |
| `Input.setInterceptDrags` | implemented | Emits `Input.dragIntercepted` (Playwright drag-and-drop). |

### Emulation

| Method | Status | Notes |
| --- | --- | --- |
| `Emulation.setDeviceMetricsOverride` / `clearDeviceMetricsOverride` | partial | Width, height, DPR, screen size and resize events. `mobile: true` is accepted, but `<meta name=viewport>` has no effect. Transforms and non-trivial `screenOrientation` are refused. |
| `Emulation.setEmulatedMedia` | implemented | `print`/`screen`, `prefers-color-scheme` and `prefers-reduced-motion` reach matchMedia, CSS, layout and rendering. Other features are accepted and not evaluated. |
| `Emulation.setUserAgentOverride` | implemented | Per page. `userAgentMetadata` drives `navigator.userAgentData` and the low-entropy `Sec-CH-UA*` headers. |
| `Emulation.setLocaleOverride` | partial | `navigator.language(s)` and `Accept-Language` only. `Intl` and `Date` keep the process locale. |
| `Emulation.setTimezoneOverride` | partial | Per connection (`Date`, `Intl`, `Temporal`). Two pages on one connection cannot hold different zones ("Cannot set timezone"). |
| `Emulation.setGeolocationOverride` / `clearGeolocationOverride` | implemented | Per browser context. Needs the geolocation permission. |
| `Emulation.setTouchEmulationEnabled` | implemented | `navigator.maxTouchPoints` (1–16), `(pointer: coarse)`, `(hover: none)`. |
| `Emulation.setDefaultBackgroundColorOverride` | implemented | Screenshot background. `a: 0` gives transparent PNGs. |
| `Emulation.setVirtualTimePolicy` | partial | Freezes `Date` and `performance.now()` and holds timers. `advance` and `pauseIfNetworkFetchesPending` with `budget` (emits `Emulation.virtualTimeBudgetExpired`). Thread-wide and owned by one page. Cannot be turned off. |
| `Emulation.setFocusEmulationEnabled` | accepted | Headless pages always behave as focused. |

### Network

| Method | Status | Notes |
| --- | --- | --- |
| `Network.enable` / `disable` | implemented | Request, response, data, finish and failure events, plus WebSocket events. Bounded buffers: on overflow the connection closes instead of dropping events. Per-session buffer options are refused. |
| `Network.getResponseBody` | implemented | Text or base64. 64 MiB process budget, 8 MiB per body. Evicted bodies get an explicit error. |
| `Network.getRequestPostData` | implemented | Up to 1 MiB per body, 8 MiB per page. |
| `Network.setExtraHTTPHeaders` | implemented | Per page. Applied while Network is enabled. |
| `Network.setUserAgentOverride` | implemented | Same as the Emulation method. |
| `Network.setCacheDisabled` | implemented | Per page, while Network is enabled. |
| `Network.clearBrowserCache` | implemented | The context's HTTP cache, and the shared asset cache when it is on. |
| `Network.setBypassServiceWorker` | implemented | Per session. |
| `Network.emulateNetworkConditions` | partial | `offline` only (per context). Latency and throughput are ignored. |
| `Network.getCookies` / `setCookie` / `setCookies` / `deleteCookies` / `clearBrowserCookies` | implemented | The page's context jar. |

### Fetch

| Method | Status | Notes |
| --- | --- | --- |
| `Fetch.enable` / `disable` | implemented | Request and response stages. Only one session per page can intercept. Synchronous XHR is not offered for interception. |
| `Fetch.continueRequest` | implemented | Request overrides. `interceptResponse: true` pauses the request again at the response stage. |
| `Fetch.fulfillRequest` | implemented | Text or binary headers. Custom status phrases are refused. |
| `Fetch.failRequest` | implemented | |
| `Fetch.continueResponse` | implemented | Response stage. |
| `Fetch.getResponseBody` | implemented | Response stage only. |
| `Fetch.continueWithAuth` | implemented | Server and proxy Basic challenges (`Fetch.authRequired`). Other schemes are unsupported. |

### Accessibility

| Method | Status | Notes |
| --- | --- | --- |
| `Accessibility.enable` / `disable` | accepted | Trees are computed per call. |
| `Accessibility.getFullAXTree` | implemented | `depth`, `frameId`. Chrome's node shape, compared 1:1 on fixtures. No `InlineTextBox` nodes, no synthesized nodes, shadow and frame contents not inlined. At most 16384 nodes. |
| `Accessibility.getPartialAXTree` | partial | `backendNodeId`/`objectId` and `fetchRelatives`. No frontend `nodeId`. |
| `Accessibility.queryAXTree` | implemented | Exact `accessibleName` and `role` match. |

### Coverage: Profiler, Debugger, CSS

| Method | Status | Notes |
| --- | --- | --- |
| `Profiler.enable` | accepted | Coverage starts with `startPreciseCoverage`. |
| `Profiler.disable` | implemented | Stops precise coverage. |
| `Profiler.startPreciseCoverage` / `stopPreciseCoverage` / `takePreciseCoverage` | implemented | JavaScriptCore's control-flow profiler in V8's block-coverage shape (for c8 and v8-to-istanbul). Playwright `page.coverage` JS. |
| `Debugger.enable` / `disable` | implemented | Emits `Debugger.scriptParsed` while precise coverage is on. No breakpoints. |
| `Debugger.getScriptSource` | implemented | |
| `Debugger.resume`, `setSkipAllPauses`, `setAsyncCallStackDepth`, `setBreakpointsActive`, `setPauseOnExceptions`, `setBlackboxPatterns` | accepted | The engine never pauses. |
| `CSS.enable` / `disable` | implemented | `CSS.styleSheetAdded` for `<link>` and `<style>` sheets. |
| `CSS.getStyleSheetText` | implemented | |
| `CSS.startRuleUsageTracking` / `stopRuleUsageTracking` / `takeCoverageDelta` | implemented | Rule usage for `page.coverage` CSS. |

### Other domains

| Method | Status | Notes |
| --- | --- | --- |
| `Browser.getWindowForTarget` | partial | One headless window per page. Reports the bounds set with `setWindowBounds`, else the viewport (default 1280x720). |
| `Browser.setWindowBounds` | accepted | Stored and reported back. No page API observes it. |
| `Performance.enable` / `disable` | accepted | No timeline events. |
| `Performance.getMetrics` | implemented | `Timestamp`, `Documents`, `Frames`, `Nodes`, `LayoutCount`, `RecalcStyleCount`, `LayoutDuration`, `RecalcStyleDuration`. No JS heap metrics. |
| `IO.read` / `IO.close` | implemented | Streams from `printToPDF` with `ReturnAsStream`. |
| `Log.enable` / `disable` | accepted | Console output goes through `Runtime.consoleAPICalled`. |
| `Audits.enable` / `disable` | accepted | No issues are reported. |
| `Security.*` | see [Browser, Storage, Security](#browser-storage-security) | |
| `Target.getTargets`, `Target.getTargetInfo`, `Target.setAutoAttach` | implemented | On a page session, `setAutoAttach` attaches the page's dedicated workers. Iframes are frames of the page, not separate targets. |
| `LP.run`, `LP.idle`, `LP.act`, `LP.expect`, `LP.query`, `LP.dialogs` | implemented | Pyro's native driver. See [LP.run wire format](/reference/lp-run). |

### Worker sessions

Dedicated workers attached through `Target.setAutoAttach` on a page session accept `Runtime.enable`, `disable`, `evaluate`, `callFunctionOn`, `releaseObject`, `releaseObjectGroup`, plus `setAsyncCallStackDepth`, `discardConsoleEntries` and `terminateExecution` (accepted), `Target.getTargetInfo`, `Target.setAutoAttach` and `Runtime.runIfWaitingForDebugger`. Workers never wait for the debugger. Anything else returns the unsupported error.

### Tab sessions (Puppeteer)

A `tab` target session (Puppeteer's auto-attach filter) accepts `Target.setAutoAttach` (attaches its page), `Target.getTargetInfo`, `Target.detachFromTarget` and `Runtime.runIfWaitingForDebugger`.

## Events

| Domain | Events |
| --- | --- |
| Target | `targetCreated`, `targetDestroyed`, `attachedToTarget`, `detachedFromTarget` (no `targetInfoChanged`) |
| Page | `frameAttached`, `frameDetached`, `frameNavigated`, `frameStartedNavigating`, `frameStartedLoading`, `frameStoppedLoading`, `frameRequestedNavigation`, `frameScheduledNavigation`, `frameClearedScheduledNavigation`, `navigatedWithinDocument`, `lifecycleEvent`, `domContentEventFired`, `loadEventFired`, `javascriptDialogOpening`, `javascriptDialogClosed`, `fileChooserOpened` |
| Runtime | `executionContextCreated`, `executionContextDestroyed`, `executionContextsCleared`, `consoleAPICalled` (with object previews and the caller's stack trace), `exceptionThrown`, `bindingCalled` |
| Network | `requestWillBeSent`, `responseReceived`, `dataReceived`, `loadingFinished`, `loadingFailed`, `webSocketCreated`, `webSocketWillSendHandshakeRequest`, `webSocketHandshakeResponseReceived`, `webSocketFrameSent`, `webSocketFrameReceived`, `webSocketFrameError`, `webSocketClosed` (no `*ExtraInfo` events: `hasExtraInfo` is `false`) |
| Fetch | `requestPaused`, `authRequired` |
| Browser | `downloadWillBegin`, `downloadProgress` |
| DOM | `documentUpdated` |
| CSS | `styleSheetAdded` |
| Debugger | `scriptParsed` |
| Input | `dragIntercepted` |
| Emulation | `virtualTimeBudgetExpired` |

## Differences from Chrome worth knowing

- **Contexts:** many browser contexts can coexist on one connection. All of them are disposed when the connection closes.
- **No placeholder page:** `Target.setAutoAttach` attaches no startup page.
- **`about:blank` has a realm:** scripts can run in a blank page.
- **Iframes are frames, not targets:** there are no out-of-process iframes. Child frames appear through `Page.frame*` events and `frameId` parameters.
- **Rendering is CPU-only:** screenshots and PDFs come from Blitz (Stylo, Taffy, Parley, vello_cpu), so pixels differ from Chromium's.

For the web APIs inside the page, see [Web platform support](/reference/web-platform). For Playwright API coverage, see [API coverage](/playwright/api-coverage).
