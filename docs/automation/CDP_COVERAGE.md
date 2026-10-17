# CDP coverage inventory

Sources:
- **Playwright side:** every `send("Domain.method")` in `playwright-core@1.59.1/lib/server/chromium/*.js`, the version TOS pins. This is the static superset Playwright *may* send. The subset TOS actually drives will be recorded through `bench/scripts/cdp-record-proxy.mjs` and added here.
- **Upstream side:** the method enums of `src/server/cdp/domains/*.zig` at lightpanda-io/browser `2ad38773fd674d3c9cea26a263b814ee46f6ba84` (200 methods across 22 domains). "yes" means upstream dispatches the method; it does not imply full Chrome semantics.

Rewrite status:
- **engine:** implemented over the real JavaScriptCore engine (`crates/lp-cdp/src/engine`). Covered by `crates/lp-cdp/tests/engine_cdp.rs` and `tests/e2e/about-blank.mjs` (real Playwright 1.59.1).
- **accepted, recorded only:** the command succeeds, but nothing in the engine can observe its effect yet. The note says why the success is truthful today and what will make it effective.
- **conditional:** succeeds only when the request is already true of the engine; otherwise it returns an explicit error.
- **pending:** returns an explicit CDP error (`-32000 … is not supported by lightningpanda`) and never a fabricated success.

Only `about:blank` can load until document loading (W2 `lp-dom`/`lp-net`) is integrated, so the DOM, Input, Network events and Fetch domains are pending.

Playwright 1.59.1 methods: 101. Upstream implements 81 of them. Rewrite: 22 engine, 12 accepted/recorded only, 2 conditional, 65 pending.

| Method | Upstream 2ad38773f | Rewrite | Notes |
| --- | --- | --- | --- |
| `Browser.cancelDownload` | **no** | pending |  |
| `Browser.getVersion` | yes | engine |  |
| `Browser.getWindowBounds` | **no** | pending |  |
| `Browser.getWindowForTarget` | yes | accepted, recorded only | one headless window per page; bounds from the emulated viewport (default 1280x720) |
| `Browser.grantPermissions` | yes | pending |  |
| `Browser.resetPermissions` | yes | pending |  |
| `Browser.setDockTile` | **no** | pending |  |
| `Browser.setDownloadBehavior` | yes | accepted, recorded only | accepted; no network, so no download can start |
| `Browser.setWindowBounds` | yes | accepted, recorded only | validated window id; nothing observes window bounds yet (upstream: no-op) |
| `CSS.disable` | **no** | pending |  |
| `CSS.enable` | yes | pending |  |
| `CSS.startRuleUsageTracking` | **no** | pending |  |
| `CSS.stopRuleUsageTracking` | **no** | pending |  |
| `DOM.describeNode` | yes | pending |  |
| `DOM.disable` | **no** | pending |  |
| `DOM.enable` | yes | pending |  |
| `DOM.getFrameOwner` | yes | pending |  |
| `DOM.scrollIntoViewIfNeeded` | yes | pending |  |
| `DOM.setFileInputFiles` | yes | pending |  |
| `Debugger.disable` | yes | pending |  |
| `Debugger.enable` | yes | pending |  |
| `Debugger.resume` | yes | pending |  |
| `Debugger.setSkipAllPauses` | yes | pending |  |
| `Emulation.setDefaultBackgroundColorOverride` | yes | pending |  |
| `Emulation.setDeviceMetricsOverride` | yes | accepted, recorded only | stored per page; no window metrics/layout API exists yet to apply it |
| `Emulation.setEmulatedMedia` | yes | accepted, recorded only | stored; no matchMedia/CSS media evaluation exists yet |
| `Emulation.setFocusEmulationEnabled` | yes | accepted, recorded only | headless pages always behave as focused |
| `Emulation.setGeolocationOverride` | yes | pending |  |
| `Emulation.setLocaleOverride` | yes | conditional | succeeds only when the requested locale equals the engine (process) locale; otherwise an explicit error |
| `Emulation.setScriptExecutionDisabled` | yes | pending |  |
| `Emulation.setTimezoneOverride` | yes | conditional | succeeds only when it equals the process time zone; otherwise an explicit error |
| `Emulation.setTouchEmulationEnabled` | yes | pending |  |
| `Emulation.setUserAgentOverride` | yes | accepted, recorded only | stored; no navigator or HTTP loader exists yet |
| `Fetch.continueRequest` | yes | pending |  |
| `Fetch.disable` | yes | pending |  |
| `Fetch.enable` | yes | pending |  |
| `Fetch.failRequest` | yes | pending |  |
| `Fetch.fulfillRequest` | yes | pending |  |
| `HeapProfiler.collectGarbage` | **no** | pending |  |
| `IO.close` | yes | pending |  |
| `IO.read` | yes | pending |  |
| `Input.dispatchDragEvent` | **no** | pending |  |
| `Input.dispatchKeyEvent` | yes | pending |  |
| `Input.dispatchMouseEvent` | yes | pending |  |
| `Input.dispatchTouchEvent` | yes | pending |  |
| `Input.insertText` | yes | pending |  |
| `Input.setInterceptDrags` | **no** | pending |  |
| `Log.enable` | yes | accepted, recorded only | console goes through Runtime.consoleAPICalled; no Log entries are produced |
| `Network.clearBrowserCache` | yes | pending |  |
| `Network.emulateNetworkConditions` | yes | pending |  |
| `Network.enable` | yes | accepted, recorded only | no network exists yet, so there are no events to report |
| `Network.getResponseBody` | yes | pending |  |
| `Network.loadNetworkResource` | **no** | pending |  |
| `Network.setCacheDisabled` | yes | pending |  |
| `Network.setExtraHTTPHeaders` | yes | pending |  |
| `Page.addScriptToEvaluateOnNewDocument` | yes | engine |  |
| `Page.bringToFront` | yes | accepted, recorded only | single foreground page per target |
| `Page.captureScreenshot` | yes | pending |  |
| `Page.close` | yes | pending |  |
| `Page.enable` | yes | engine |  |
| `Page.getFrameTree` | yes | engine |  |
| `Page.getLayoutMetrics` | yes | pending |  |
| `Page.getNavigationHistory` | yes | pending |  |
| `Page.handleJavaScriptDialog` | yes | pending |  |
| `Page.navigate` | yes | engine | about:blank commits; other URLs answer with `errorText` net::ERR_NOT_IMPLEMENTED until document loading lands |
| `Page.navigateToHistoryEntry` | yes | pending |  |
| `Page.printToPDF` | yes | pending |  |
| `Page.reload` | yes | engine | same limits as Page.navigate |
| `Page.removeScriptToEvaluateOnNewDocument` | yes | engine |  |
| `Page.setBypassCSP` | yes | accepted, recorded only | no CSP enforcement exists |
| `Page.setFontFamilies` | **no** | pending |  |
| `Page.setInterceptFileChooserDialog` | yes | accepted, recorded only | no file chooser exists |
| `Page.setLifecycleEventsEnabled` | yes | engine |  |
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
| `Runtime.runIfWaitingForDebugger` | yes | engine |  |
| `Security.setIgnoreCertificateErrors` | yes | pending |  |
| `Storage.clearCookies` | yes | pending |  |
| `Storage.getCookies` | yes | pending |  |
| `Storage.setCookies` | yes | pending |  |
| `Target.attachToBrowserTarget` | yes | pending |  |
| `Target.attachToTarget` | yes | engine |  |
| `Target.closeTarget` | yes | engine |  |
| `Target.createBrowserContext` | yes | engine | `proxyServer` is refused |
| `Target.createTarget` | yes | engine |  |
| `Target.detachFromTarget` | yes | engine |  |
| `Target.disposeBrowserContext` | yes | engine |  |
| `Target.getTargetInfo` | yes | engine |  |
| `Target.setAutoAttach` | yes | engine |  |
| `Tracing.end` | **no** | pending |  |
| `Tracing.start` | **no** | pending |  |

Browser-level methods not in Playwright's list but implemented: `Target.getTargets`, `Target.getBrowserContexts`, `Target.setDiscoverTargets` (page filters honored), `Target.activateTarget` (validates the target), `Runtime.releaseObjectGroup`, `Emulation.clearDeviceMetricsOverride`, `Page.disable`, `Runtime.disable`.

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
