# CDP inventory (idle/smoke-proxy-1.cdp.jsonl)

records=218 ws_connections=1 commands=91 error_responses=0 events=33

HTTP: GET /json/version -> 200

## Client -> server commands

| method | count | w/ sessionId | w/o sessionId | ok | error | error messages |
|---|---:|---:|---:|---:|---:|---|
| Runtime.callFunctionOn | 26 | 26 | 0 | 26 | 0 |  |
| Runtime.releaseObject | 8 | 8 | 0 | 8 | 0 |  |
| Input.dispatchMouseEvent | 6 | 6 | 0 | 6 | 0 |  |
| Page.enable | 4 | 4 | 0 | 4 | 0 |  |
| Runtime.evaluate | 3 | 3 | 0 | 3 | 0 |  |
| Target.setAutoAttach | 3 | 2 | 1 | 3 | 0 |  |
| Browser.setDownloadBehavior | 2 | 0 | 2 | 2 | 0 |  |
| DOM.getContentQuads | 2 | 2 | 0 | 2 | 0 |  |
| DOM.scrollIntoViewIfNeeded | 2 | 2 | 0 | 2 | 0 |  |
| Emulation.setEmulatedMedia | 2 | 2 | 0 | 2 | 0 |  |
| Emulation.setFocusEmulationEnabled | 2 | 2 | 0 | 2 | 0 |  |
| Log.enable | 2 | 2 | 0 | 2 | 0 |  |
| Network.enable | 2 | 2 | 0 | 2 | 0 |  |
| Page.addScriptToEvaluateOnNewDocument | 2 | 2 | 0 | 2 | 0 |  |
| Page.createIsolatedWorld | 2 | 2 | 0 | 2 | 0 |  |
| Page.getFrameTree | 2 | 2 | 0 | 2 | 0 |  |
| Page.setLifecycleEventsEnabled | 2 | 2 | 0 | 2 | 0 |  |
| Runtime.enable | 2 | 2 | 0 | 2 | 0 |  |
| Runtime.runIfWaitingForDebugger | 2 | 2 | 0 | 2 | 0 |  |
| Browser.getVersion | 1 | 0 | 1 | 1 | 0 |  |
| Browser.getWindowForTarget | 1 | 1 | 0 | 1 | 0 |  |
| Browser.setWindowBounds | 1 | 1 | 0 | 1 | 0 |  |
| Emulation.setDeviceMetricsOverride | 1 | 1 | 0 | 1 | 0 |  |
| Fetch.enable | 1 | 1 | 0 | 1 | 0 |  |
| Fetch.fulfillRequest | 1 | 1 | 0 | 1 | 0 |  |
| Input.insertText | 1 | 1 | 0 | 1 | 0 |  |
| Network.getResponseBody | 1 | 1 | 0 | 1 | 0 |  |
| Network.setCacheDisabled | 1 | 1 | 0 | 1 | 0 |  |
| Page.navigate | 1 | 1 | 0 | 1 | 0 |  |
| Target.closeTarget | 1 | 0 | 1 | 1 | 0 |  |
| Target.createBrowserContext | 1 | 0 | 1 | 1 | 0 |  |
| Target.createTarget | 1 | 0 | 1 | 1 | 0 |  |
| Target.disposeBrowserContext | 1 | 0 | 1 | 1 | 0 |  |
| Target.getTargetInfo | 1 | 0 | 1 | 1 | 0 |  |

## Server -> client events

| event | count | w/ sessionId | w/o sessionId |
|---|---:|---:|---:|
| Page.lifecycleEvent | 5 | 5 | 0 |
| Runtime.executionContextCreated | 4 | 4 | 0 |
| Network.loadingFinished | 3 | 3 | 0 |
| Network.requestWillBeSent | 3 | 3 | 0 |
| Network.responseReceived | 3 | 3 | 0 |
| Target.attachedToTarget | 2 | 0 | 2 |
| DOM.documentUpdated | 1 | 1 | 0 |
| Fetch.requestPaused | 1 | 1 | 0 |
| Inspector.detached | 1 | 1 | 0 |
| Page.domContentEventFired | 1 | 1 | 0 |
| Page.frameNavigated | 1 | 1 | 0 |
| Page.frameStartedLoading | 1 | 1 | 0 |
| Page.frameStartedNavigating | 1 | 1 | 0 |
| Page.frameStoppedLoading | 1 | 1 | 0 |
| Page.loadEventFired | 1 | 1 | 0 |
| Runtime.executionContextsCleared | 1 | 1 | 0 |
| Target.detachedFromTarget | 1 | 0 | 1 |
| Target.targetCreated | 1 | 0 | 1 |
| Target.targetDestroyed | 1 | 0 | 1 |
