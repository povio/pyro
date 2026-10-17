# CDP coverage inventory

Sources:
- Playwright side: every `send("Domain.method")` in `playwright-core@1.59.1/lib/server/chromium/*.js` (the version TOS pins). This is the static superset Playwright *may* send; the subset TOS actually drives will be recorded from a baseline run through `bench/cdp-record-proxy` and added here.
- Upstream side: the method enums of `src/server/cdp/domains/*.zig` at lightpanda-io/browser `2ad38773fd674d3c9cea26a263b814ee46f6ba84` (200 methods across 22 domains). "yes" means upstream dispatches the method; it does not imply full Chrome semantics.
- Rewrite status: `protocol only` = implemented in `crates/lp-cdp` and covered by protocol-sequencing tests, not yet wired to the JSC engine; `pending` = not implemented. No method returns fabricated success; unimplemented methods return an explicit CDP error.

Playwright 1.59.1 methods: 101. Upstream implements 81 of them.

| Method | Upstream 2ad38773f | Rewrite |
| --- | --- | --- |
| `Browser.cancelDownload` | **no** | pending |
| `Browser.getVersion` | yes | protocol only |
| `Browser.getWindowBounds` | **no** | pending |
| `Browser.getWindowForTarget` | yes | pending |
| `Browser.grantPermissions` | yes | pending |
| `Browser.resetPermissions` | yes | pending |
| `Browser.setDockTile` | **no** | pending |
| `Browser.setDownloadBehavior` | yes | pending |
| `Browser.setWindowBounds` | yes | pending |
| `CSS.disable` | **no** | pending |
| `CSS.enable` | yes | pending |
| `CSS.startRuleUsageTracking` | **no** | pending |
| `CSS.stopRuleUsageTracking` | **no** | pending |
| `DOM.describeNode` | yes | pending |
| `DOM.disable` | **no** | pending |
| `DOM.enable` | yes | pending |
| `DOM.getFrameOwner` | yes | pending |
| `DOM.scrollIntoViewIfNeeded` | yes | pending |
| `DOM.setFileInputFiles` | yes | pending |
| `Debugger.disable` | yes | pending |
| `Debugger.enable` | yes | pending |
| `Debugger.resume` | yes | pending |
| `Debugger.setSkipAllPauses` | yes | pending |
| `Emulation.setDefaultBackgroundColorOverride` | yes | pending |
| `Emulation.setDeviceMetricsOverride` | yes | pending |
| `Emulation.setEmulatedMedia` | yes | pending |
| `Emulation.setFocusEmulationEnabled` | yes | pending |
| `Emulation.setGeolocationOverride` | yes | pending |
| `Emulation.setLocaleOverride` | yes | pending |
| `Emulation.setScriptExecutionDisabled` | yes | pending |
| `Emulation.setTimezoneOverride` | yes | pending |
| `Emulation.setTouchEmulationEnabled` | yes | pending |
| `Emulation.setUserAgentOverride` | yes | pending |
| `Fetch.continueRequest` | yes | pending |
| `Fetch.disable` | yes | pending |
| `Fetch.enable` | yes | pending |
| `Fetch.failRequest` | yes | pending |
| `Fetch.fulfillRequest` | yes | pending |
| `HeapProfiler.collectGarbage` | **no** | pending |
| `IO.close` | yes | pending |
| `IO.read` | yes | pending |
| `Input.dispatchDragEvent` | **no** | pending |
| `Input.dispatchKeyEvent` | yes | pending |
| `Input.dispatchMouseEvent` | yes | pending |
| `Input.dispatchTouchEvent` | yes | pending |
| `Input.insertText` | yes | pending |
| `Input.setInterceptDrags` | **no** | pending |
| `Log.enable` | yes | pending |
| `Network.clearBrowserCache` | yes | pending |
| `Network.emulateNetworkConditions` | yes | pending |
| `Network.enable` | yes | pending |
| `Network.getResponseBody` | yes | pending |
| `Network.loadNetworkResource` | **no** | pending |
| `Network.setCacheDisabled` | yes | pending |
| `Network.setExtraHTTPHeaders` | yes | pending |
| `Page.addScriptToEvaluateOnNewDocument` | yes | pending |
| `Page.bringToFront` | yes | pending |
| `Page.captureScreenshot` | yes | pending |
| `Page.close` | yes | pending |
| `Page.enable` | yes | pending |
| `Page.getFrameTree` | yes | pending |
| `Page.getLayoutMetrics` | yes | pending |
| `Page.getNavigationHistory` | yes | pending |
| `Page.handleJavaScriptDialog` | yes | pending |
| `Page.navigate` | yes | pending |
| `Page.navigateToHistoryEntry` | yes | pending |
| `Page.printToPDF` | yes | pending |
| `Page.reload` | yes | pending |
| `Page.removeScriptToEvaluateOnNewDocument` | yes | pending |
| `Page.setBypassCSP` | yes | pending |
| `Page.setFontFamilies` | **no** | pending |
| `Page.setInterceptFileChooserDialog` | yes | pending |
| `Page.setLifecycleEventsEnabled` | yes | pending |
| `Page.startScreencast` | **no** | pending |
| `Profiler.disable` | **no** | pending |
| `Profiler.enable` | **no** | pending |
| `Profiler.startPreciseCoverage` | **no** | pending |
| `Profiler.stopPreciseCoverage` | **no** | pending |
| `Profiler.takePreciseCoverage` | **no** | pending |
| `Runtime.addBinding` | yes | pending |
| `Runtime.callFunctionOn` | yes | pending |
| `Runtime.enable` | yes | pending |
| `Runtime.evaluate` | yes | pending |
| `Runtime.getProperties` | yes | pending |
| `Runtime.releaseObject` | yes | pending |
| `Runtime.runIfWaitingForDebugger` | yes | protocol only |
| `Security.setIgnoreCertificateErrors` | yes | pending |
| `Storage.clearCookies` | yes | pending |
| `Storage.getCookies` | yes | pending |
| `Storage.setCookies` | yes | pending |
| `Target.attachToBrowserTarget` | yes | pending |
| `Target.attachToTarget` | yes | protocol only |
| `Target.closeTarget` | yes | protocol only |
| `Target.createBrowserContext` | yes | protocol only |
| `Target.createTarget` | yes | protocol only |
| `Target.detachFromTarget` | yes | protocol only |
| `Target.disposeBrowserContext` | yes | protocol only |
| `Target.getTargetInfo` | yes | protocol only |
| `Target.setAutoAttach` | yes | protocol only |
| `Tracing.end` | **no** | pending |
| `Tracing.start` | **no** | pending |

## Upstream-only surface not yet tracked for the rewrite

Upstream also implements domains Playwright does not send: `LP.*` (markdown/semantic tree/agent helpers), `WebMCP.*`, `Accessibility.*` (`getFullAXTree`, `queryAXTree`), `Debugger.*` beyond enable/disable (backed by v8_inspector), `Network` cookie helpers, `Fetch.continueResponse/getResponseBody`, `DOM.getDocument/querySelector*/getOuterHTML`. These are out of scope for the first TOS milestone and stay listed as uncovered until implemented.
