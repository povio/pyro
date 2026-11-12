# API coverage

This page lists Playwright's library API class by class and marks what `@povio/lightning-panda/playwright` implements. Use it to check whether a suite depends on something missing before you migrate.

**How this was derived.** The tables come from reading the classes in `packages/lightning-panda/src/playwright/` (`page.ts`, `browser.ts`, `handles.ts`, `request.ts`, `clock.ts`, `coverage.ts`, `cdp-session.ts`, `index.ts`) at version 0.0.33 and comparing their methods with Playwright's documented API. Many entries were also checked by running scripts against the published 0.0.33 package. The test runner and `expect` are covered in [Fixtures, hooks and steps](/playwright/fixtures), [Config and the test runner](/playwright/runner) and [Assertions](/playwright/assertions).

**Legend.** Supported: works as in Playwright. Partial: works with the limits noted. No-op: exists and does nothing. Not supported: missing (a type error, and `undefined` at run time) or throws.

## Module exports

| Export | Status | Notes |
| --- | --- | --- |
| `test`, `expect`, `defineConfig` | Supported | See the runner pages. |
| `chromium` | Partial | `launch`, `connectOverCDP`, `connect`, `name`. |
| `firefox`, `webkit`, `_android`, `_electron` | Not supported | |
| `request` | Supported | `request.newContext()`. |
| `devices` | Partial | Five descriptors: `Desktop Chrome`, `Desktop Chrome HiDPI`, `Desktop Edge`, `Pixel 7`, `iPhone 15`. |
| `errors` | Partial | `errors.TimeoutError` only. |
| `selectors` | Partial | `setTestIdAttribute()` is a no-op (use the `testIdAttribute` option); `register()` throws. |
| Types (`Page`, `Locator`, `BrowserContext`, `PlaywrightTestConfig`, ...) | Supported | Playwright's shapes. |

## BrowserType (`chromium`)

| Method | Status | Notes |
| --- | --- | --- |
| `launch(options)` | Partial | Launches the bundled Pyro. Takes `bin`, `args` (serve flags) and `boot`. Playwright's launch options (`headless`, `executablePath`, `proxy`, ...) are ignored. |
| `connectOverCDP(endpoint)` | Supported | A `ws://` URL or `http://host:port` of a running `lightningpanda serve`. `timeout` and `headers` are ignored. |
| `connect(wsEndpoint)` | Partial | Same as `connectOverCDP`; there is no Playwright server protocol. |
| `name()` | Supported | `'chromium'`. |
| `launchPersistentContext`, `launchServer`, `executablePath` | Not supported | |

## Browser

| Method | Status | Notes |
| --- | --- | --- |
| `newContext(options)` | Supported | In a test, starts from the test's `use` options. See [Context options](#context-options). |
| `newPage(options)` | Supported | A page in its own context, closed with the page. |
| `contexts()`, `isConnected()`, `close()` | Supported | |
| `version()` | Partial | Returns `'lightningpanda'`. |
| `browserType()` | Partial | `{ name: () => 'chromium' }`. |
| `on/once/off('disconnected')` | Supported | |
| `newBrowserCDPSession()`, `startTracing()`, `stopTracing()`, `removeAllListeners()` | Not supported | |

## BrowserContext

| Method | Status | Notes |
| --- | --- | --- |
| `newPage()`, `pages()`, `browser()`, `close()` | Supported | |
| `addInitScript(script, arg)` | Supported | Function, string, `{ path }` or `{ content }`. |
| `addCookies()`, `cookies(urls)` | Supported | A cookie needs `url` or `domain`. |
| `clearCookies(options)` | Partial | Clears all cookies; filter options are ignored. |
| `storageState({ path })` | Partial | `localStorage` only of origins with an open page; `indexedDB` is not saved. |
| `route()`, `unroute()`, `unrouteAll()` | Supported | `unrouteAll`'s `behavior` is ignored. |
| `routeFromHAR(path, options)` | Supported | Replay, and recording with `update: true`. |
| `routeWebSocket()` | Not supported | |
| `exposeFunction()`, `exposeBinding()` | Supported | Binding source `frame` is the main frame. |
| `setExtraHTTPHeaders()` | Supported | |
| `setDefaultTimeout()`, `setDefaultNavigationTimeout()` | Supported | |
| `newCDPSession(pageOrFrame)` | Supported | Pyro's CDP. |
| `request` | Partial | Does not share cookies with the context. |
| `clock` | Supported | |
| `tracing` | No-op | `start`, `stop`, `startChunk`, `stopChunk`, `group`, `groupEnd`. |
| `waitForEvent(event, options)` | Supported | Default timeout 30 s. |
| `on/once/off` | Partial | Events `page`, `close`, `console`, `request`, `response`, `requestfinished`, `requestfailed`. No `dialog`, `weberror`, `serviceworker`, `backgroundpage`. |
| `grantPermissions()`, `clearPermissions()`, `setGeolocation()`, `setOffline()` | No-op | |
| `setHTTPCredentials()`, `serviceWorkers()`, `backgroundPages()`, `removeAllListeners()` | Not supported | |

### Context options

| Option | Status |
| --- | --- |
| `baseURL`, `viewport`, `storageState`, `extraHTTPHeaders`, `userAgent`, `colorScheme`, `reducedMotion`, `acceptDownloads`, `recordHar` | Supported |
| `httpCredentials`, `ignoreHTTPSErrors` | Partial: applied to `context.request` only |
| `screen`, `forcedColors`, `bypassCSP`, `deviceScaleFactor`, `geolocation`, `hasTouch`, `isMobile`, `javaScriptEnabled`, `locale`, `offline`, `permissions`, `proxy`, `serviceWorkers`, `strictSelectors`, `timezoneId`, `recordVideo` | Accepted, no effect |

## Page

| Method | Status | Notes |
| --- | --- | --- |
| `goto(url, options)` | Partial | `waitUntil` (`load`, `domcontentloaded`, `commit` as `domcontentloaded`, `networkidle`), `timeout`. The returned `Response` has URL and status only; `null` on a [booted page](/playwright/boot-pools). `referer` is ignored. |
| `reload()` | Partial | Same response limits as `goto`. |
| `goBack()`, `goForward()` | Partial | Return `null`. |
| `url()`, `title()`, `content()`, `setContent()` | Supported | |
| `locator()`, `getByRole()`, `getByText()`, `getByLabel()`, `getByPlaceholder()`, `getByAltText()`, `getByTitle()`, `getByTestId()`, `frameLocator()` | Supported | `getByAltText`, `getByTitle`, `getByTestId`: strings only. |
| `evaluate()`, `evaluateHandle()` | Supported | Handles in arguments work. |
| `$()`, `$$()`, `$eval()`, `$$eval()` | Supported | |
| `waitForSelector()` | Supported | States `attached`, `detached`, `visible`, `hidden`. |
| `waitForFunction(fn, arg, options)` | Partial | `polling` in ms; `'raf'` polls every 16 ms. |
| `waitForLoadState()`, `waitForURL()`, `waitForTimeout()` | Supported | |
| `waitForRequest()`, `waitForResponse()`, `waitForEvent()` | Supported | |
| `route()`, `unroute()`, `unrouteAll()`, `routeFromHAR()` | Supported | |
| `routeWebSocket()` | Not supported | |
| `on/once/off/addListener/removeListener/removeAllListeners` | Supported | Events below. |
| `mainFrame()`, `frames()`, `frame(nameOrOptions)` | Supported | |
| `context()`, `isClosed()`, `close()` | Supported | `runBeforeUnload` is ignored. |
| `keyboard`, `mouse` | Supported | See below. |
| `touchscreen` | Not supported | |
| `clock`, `coverage` | Supported | See [Clock, coverage, ARIA](/playwright/advanced). |
| `request` | Partial | The context's `request`; separate cookies. |
| `addInitScript()` | Supported | |
| `addScriptTag()`, `addStyleTag()` | Partial | Return `null` instead of an element handle. |
| `exposeFunction()`, `exposeBinding()` | Supported | |
| `setExtraHTTPHeaders()` | Supported | |
| `setViewportSize()`, `viewportSize()` | Supported | |
| `emulateMedia()` | Partial | `colorScheme`, `reducedMotion`, `media`; `forcedColors` is ignored. |
| `screenshot()` | Partial | `path`, `fullPage`, `clip`, `type`, `quality`. Rendered by Pyro. |
| `pdf()` | Partial | Options go to CDP `Page.printToPDF` unchanged (`paperWidth`, `marginTop`, ... in inches). |
| `ariaSnapshot()` | Supported | Includes `mode: 'ai'`. |
| `workers()` | Supported | |
| `setDefaultTimeout()`, `setDefaultNavigationTimeout()` | Supported | |
| `opener()` | Partial | Always `null`. |
| `video()` | Partial | Always `null`. |
| `bringToFront()`, `pause()` | No-op | |
| `click()`, `dblclick()`, `fill()`, `type()`, `press()`, `check()`, `uncheck()`, `setChecked()`, `hover()`, `tap()`, `focus()`, `selectOption()`, `setInputFiles()`, `dispatchEvent()`, `dragAndDrop()`, `textContent()`, `innerText()`, `innerHTML()`, `inputValue()`, `getAttribute()`, `isVisible()`, `isHidden()`, `isEnabled()`, `isDisabled()`, `isChecked()`, `isEditable()` (selector-first forms) | Not supported | Use locators. |
| `waitForNavigation()`, `accessibility` | Not supported | Deprecated in Playwright. |
| `addLocatorHandler()`, `removeLocatorHandler()`, `requestGC()`, `consoleMessages()`, `pageErrors()`, `requests()` | Not supported | |

**Page events.** Supported: `close`, `console`, `dialog`, `domcontentloaded`, `download`, `filechooser`, `frameattached`, `framedetached`, `framenavigated`, `load`, `pageerror`, `popup`, `request`, `requestfailed`, `requestfinished`, `response`, `websocket`, `worker`. Not supported: `crash`.

## Frame

| Method | Status | Notes |
| --- | --- | --- |
| `locator()`, `getBy*()`, `frameLocator()` | Supported | |
| `url()`, `name()`, `page()`, `parentFrame()`, `childFrames()`, `isDetached()`, `frameElement()` | Supported | |
| `goto()` | Partial | On a child frame, sets `location.href` and returns `null`. |
| `evaluate()`, `evaluateHandle()`, `content()`, `title()` | Partial | Child frames run through the `<iframe>` element's window: cross-origin frames fail. |
| `waitForLoadState()`, `waitForURL()`, `waitForSelector()`, `waitForFunction()`, `waitForTimeout()` | Supported | |
| `$()`, `$$()`, `$eval()`, `$$eval()` | Supported | |
| `addScriptTag()`, `addStyleTag()`, `setContent()` | Not supported | |
| Selector-first action and read methods (`click()`, `fill()`, ...) | Not supported | Use locators. |
| `waitForNavigation()` | Not supported | Deprecated. |

## Locator

| Method | Status | Notes |
| --- | --- | --- |
| `locator()`, `getBy*()`, `frameLocator()`, `contentFrame()`, `page()` | Supported | |
| `filter()` | Supported | `has`, `hasNot`, `hasText`, `hasNotText`, `visible`. |
| `and()`, `or()` | Supported | Both locators in the same frame. |
| `nth()`, `first()`, `last()`, `all()`, `count()` | Supported | |
| `click()`, `dblclick()` | Partial | `clickCount` 1 or 2; `delay` ignored. |
| `tap()` | Partial | A click. |
| `hover()` | Supported | |
| `fill()`, `clear()`, `press()`, `pressSequentially()` | Partial | `delay` and `force` ignored. |
| `check()`, `uncheck()`, `setChecked()` | Partial | `force` ignored. |
| `selectOption()`, `setInputFiles()`, `focus()`, `blur()`, `dispatchEvent()`, `scrollIntoViewIfNeeded()`, `selectText()` | Supported | |
| `dragTo(target)` | Partial | `timeout` only; position options ignored. |
| `textContent()`, `innerText()`, `innerHTML()`, `inputValue()`, `getAttribute()`, `allTextContents()`, `allInnerTexts()`, `boundingBox()` | Supported | |
| `isVisible()`, `isHidden()`, `isEnabled()`, `isDisabled()`, `isChecked()`, `isEditable()` | Supported | |
| `evaluate()`, `evaluateAll()`, `evaluateHandle()`, `elementHandle()`, `elementHandles()` | Supported | |
| `waitFor()` | Supported | |
| `ariaSnapshot()` | Supported | `mode: 'ai'`, `depth`. |
| `screenshot()` | Partial | Page screenshot clipped to the bounding box. |
| `highlight()` | No-op | |
| `describe()` | Not supported | |
| `type()` | Not supported | Deprecated in Playwright. |

## FrameLocator

| Method | Status | Notes |
| --- | --- | --- |
| `locator()`, `getBy*()`, `frameLocator()`, `owner()` | Supported | |
| `first()`, `last()`, `nth()` | Not supported | Deprecated in Playwright. |

## ElementHandle

| Method | Status | Notes |
| --- | --- | --- |
| `click()`, `dblclick()`, `tap()`, `hover()`, `fill()`, `press()`, `pressSequentially()`, `check()`, `uncheck()`, `setChecked()`, `focus()`, `selectOption()`, `selectText()`, `setInputFiles()`, `dispatchEvent()`, `screenshot()` | Supported | Native actions; the element carries `data-lp-handle` meanwhile. Same option limits as Locator. |
| `isVisible()`, `isHidden()`, `isEnabled()`, `isDisabled()`, `isEditable()`, `isChecked()`, `inputValue()` | Supported | |
| `getAttribute()`, `textContent()`, `innerText()`, `innerHTML()`, `boundingBox()` | Supported | |
| `ownerFrame()`, `contentFrame()`, `scrollIntoViewIfNeeded()` | Supported | |
| `waitForElementState(state)` | Partial | `stable` returns at once. |
| `$()`, `$$()`, `$eval()`, `$$eval()` | Partial | CSS selectors only. |
| `waitForSelector()` | Not supported | |
| `type()` | Not supported | Deprecated. |
| JSHandle methods | Supported | See below. |

## JSHandle

| Method | Status |
| --- | --- |
| `evaluate()`, `evaluateHandle()`, `jsonValue()`, `getProperty()`, `getProperties()`, `asElement()`, `dispose()`, `[Symbol.asyncDispose]` | Supported |

## Request

| Method | Status | Notes |
| --- | --- | --- |
| `url()`, `method()`, `headers()`, `allHeaders()`, `headerValue()`, `postData()`, `postDataBuffer()`, `postDataJSON()`, `resourceType()`, `isNavigationRequest()`, `response()`, `failure()`, `redirectedFrom()`, `redirectedTo()` | Supported | `allHeaders()` equals `headers()`. |
| `frame()` | Partial | Always the main frame. |
| `timing()` | Partial | Every field is -1. |
| `headersArray()`, `sizes()`, `serviceWorker()` | Not supported | |

## Response

| Method | Status | Notes |
| --- | --- | --- |
| `url()`, `status()`, `statusText()`, `ok()`, `headers()`, `allHeaders()`, `headerValue()`, `request()`, `frame()`, `finished()` | Supported | |
| `body()`, `text()`, `json()` | Partial | Not on the response `goto()` returns. |
| `headersArray()`, `headerValues()`, `securityDetails()`, `serverAddr()`, `fromServiceWorker()` | Not supported | |

## Route

| Method | Status | Notes |
| --- | --- | --- |
| `request()`, `continue()`, `abort()`, `fallback()` | Supported | |
| `fulfill()` | Supported | `status`, `headers`, `contentType`, `body`, `json`, `path`, `response`. |
| `fetch()` | Partial | Runs from the test process; `timeout` ignored. |

## Download

| Method | Status | Notes |
| --- | --- | --- |
| `page()`, `url()`, `suggestedFilename()`, `path()`, `saveAs()`, `failure()`, `createReadStream()`, `delete()` | Supported | |
| `cancel()` | No-op | |

## Dialog

| Method | Status | Notes |
| --- | --- | --- |
| `type()`, `message()`, `defaultValue()`, `page()`, `accept()`, `dismiss()` | Supported | Without a listener, dialogs are dismissed. |

## ConsoleMessage

| Method | Status | Notes |
| --- | --- | --- |
| `type()`, `text()`, `location()`, `page()` | Supported | |
| `args()` | Partial | Always an empty list. |

## Worker

| Method | Status |
| --- | --- |
| `url()`, `evaluate()`, `evaluateHandle()`, `isClosed()`, `on/once/off('close')`, `waitForEvent()` | Supported |

## FileChooser

| Method | Status |
| --- | --- |
| `page()`, `element()`, `isMultiple()`, `setFiles()` | Supported |

## WebSocket

| Method | Status | Notes |
| --- | --- | --- |
| `url()`, `isClosed()`, `waitForEvent()`, `on/once/off/addListener/removeListener/removeAllListeners` | Supported | Events `framesent`, `framereceived`, `socketerror`, `close`. |
| `waitForFrameSent()`, `waitForFrameReceived()` | Supported | |

## APIRequestContext and APIResponse

| Method | Status | Notes |
| --- | --- | --- |
| `get()`, `post()`, `put()`, `patch()`, `delete()`, `head()`, `fetch()` | Partial | `params`, `headers`, `data`, `form`, `multipart`, `timeout`, `failOnStatusCode`, `ignoreHTTPSErrors`, `maxRedirects` (0 or follow). `maxRetries` ignored. |
| `storageState()` | Partial | Cookies only. |
| `dispose()` | Supported | |
| `APIResponse`: `url()`, `status()`, `statusText()`, `ok()`, `headers()`, `headersArray()`, `body()`, `text()`, `json()`, `dispose()` | Supported | |

## Keyboard

| Method | Status | Notes |
| --- | --- | --- |
| `press()`, `type()`, `insertText()` | Partial | Act on the focused element; `delay` ignored. |
| `down()`, `up()` | Supported | Held modifiers apply to later clicks and presses. |

## Mouse

| Method | Status |
| --- | --- |
| `click()`, `dblclick()`, `move()`, `down()`, `up()`, `wheel()` | Supported |

## Touchscreen

| Method | Status |
| --- | --- |
| `tap()` | Not supported |

## Clock

| Method | Status |
| --- | --- |
| `install()`, `fastForward()`, `runFor()`, `pauseAt()`, `resume()`, `setFixedTime()`, `setSystemTime()` | Supported |

## Coverage

| Method | Status | Notes |
| --- | --- | --- |
| `startJSCoverage()`, `stopJSCoverage()` | Partial | See [caveats](/playwright/advanced#coverage). |
| `startCSSCoverage()`, `stopCSSCoverage()` | Supported | Sheets without a URL are not reported. |

## CDPSession

| Method | Status | Notes |
| --- | --- | --- |
| `send()`, `on()`, `once()`, `off()`, `addListener()`, `removeListener()`, `detach()` | Supported | Pyro's CDP domains. See [CDP coverage](/reference/cdp). |

## Not present at all

Tracing files, video recording, the HTML reporter, the trace viewer, codegen, the inspector, UI mode, component testing, Android and Electron automation, and Firefox and WebKit browsers.
