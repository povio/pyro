# CDP coverage inventory

Sources:
- **Playwright side:** every `send("Domain.method")` in `playwright-core@1.59.1/lib/server/chromium/*.js`, the version TOS pins. This is the static superset Playwright *may* send. The subset TOS actually drives will be recorded through `bench/scripts/cdp-record-proxy.mjs` and added here.
- **Upstream side:** the method enums of `src/server/cdp/domains/*.zig` at lightpanda-io/browser `2ad38773fd674d3c9cea26a263b814ee46f6ba84` (200 methods across 22 domains). "yes" means upstream dispatches the method; it does not imply full Chrome semantics.

Rewrite status:
- **engine:** implemented over the real JavaScriptCore engine (`crates/lp-cdp/src/engine`). Covered by `crates/lp-cdp/tests/engine_cdp.rs` and `tests/e2e/about-blank.mjs` (real Playwright 1.59.1).
- **accepted, recorded only:** the command succeeds, but nothing in the engine can observe its effect yet. The note says why the success is truthful today and what will make it effective.
- **conditional:** succeeds only when the request is already true of the engine; otherwise it returns an explicit error.
- **pending:** returns an explicit CDP error (`-32000 … is not supported by lightningpanda`) and never a fabricated success.

HTTP and `data:` document loading now use the real loader. `Network` emits request/redirect/response/data/completion/failure events to enabled page sessions, with response-body capture. Browser contexts own distinct network clients. DOM/Input protocol wiring remains pending; this is not TOS compatibility. Fetch request-stage interception supports continue overrides, fulfill, abort, disable/detach cleanup and server Basic-auth challenges. Response-stage interception, custom status phrases, binary headers, proxy credentials and non-Basic credential schemes are explicitly unsupported. Only one session per page can own interception; different pages in a context may have separate owners.

Storage cookie commands use the network cookie jar, including HTTP response cookies, expiry and context isolation. Negative CDP expiry values mean session cookies. Invalid batches are validated before modifying the real jar. Partitioned/SameParty cookies, non-default priority and source overrides return explicit errors. This does not install document.cookie or localStorage into browser realms.

Capture uses a shared process payload budget (default 64 MiB, 8 MiB/body), at most 4096 response records per page, and bounded network subscriptions (1024 events/8 MiB logical bytes per context). Observer overflow closes the CDP connection instead of silently losing events. Disabling the last Network session, navigation, or page teardown releases retained bodies. Per-session `Network.enable` buffer options are explicitly unsupported. These are logical retention limits, not an RSS measurement; shared chunk allocations and metadata contribute additional memory.

Playwright 1.59.1 methods: 101. Upstream implements 81 of them. Rewrite: 32 engine, 10 accepted/recorded only, 3 conditional, 56 pending.

| Method | Upstream 2ad38773f | Rewrite | Notes |
| --- | --- | --- | --- |
| `Browser.cancelDownload` | **no** | pending |  |
| `Browser.getVersion` | yes | engine |  |
| `Browser.getWindowBounds` | **no** | pending |  |
| `Browser.getWindowForTarget` | yes | accepted, recorded only | one headless window per page; reports bounds set by setWindowBounds, else the emulated viewport (default 1280x720) |
| `Browser.grantPermissions` | yes | pending |  |
| `Browser.resetPermissions` | yes | pending |  |
| `Browser.setDockTile` | **no** | pending |  |
| `Browser.setDownloadBehavior` | yes | accepted, recorded only | accepted; the loader does not implement download delivery yet |
| `Browser.setWindowBounds` | yes | accepted, recorded only | stores the bounds, which getWindowForTarget reports back; no page API (outerWidth, screen) observes them yet (upstream: no-op) |
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
| `Emulation.setUserAgentOverride` | yes | pending | refused until per-page HTTP and navigator overrides can be applied consistently |
| `Fetch.continueRequest` | yes | engine | request stage; see supported scope above |
| `Fetch.disable` | yes | engine | request stage; see supported scope above |
| `Fetch.enable` | yes | engine | request stage; see supported scope above |
| `Fetch.failRequest` | yes | engine | request stage; see supported scope above |
| `Fetch.fulfillRequest` | yes | engine | request stage; see supported scope above |
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
| `Network.enable` | yes | engine | subscribes to bounded per-context events; filters by page/session; buffer options unsupported |
| `Network.getResponseBody` | yes | engine | text/base64 completed captures; explicit errors for unavailable or evicted bodies |
| `Network.loadNetworkResource` | **no** | pending |  |
| `Network.setCacheDisabled` | yes | conditional | no response cache exists; disabling is already true, allowing caching does not require entries |
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
| `Page.navigate` | yes | engine | about:blank, data and HTTP documents; unsupported schemes/network errors return explicit errorText |
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
| `Storage.clearCookies` | yes | engine | per-context native cookie jar; omitted context uses a distinct default context |
| `Storage.getCookies` | yes | engine | per-context native cookie jar; omitted context uses a distinct default context |
| `Storage.setCookies` | yes | engine | per-context native cookie jar; omitted context uses a distinct default context |
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
