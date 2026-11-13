# Behaviour differences

Most Playwright tests pass unchanged, but a few behaviours differ, either because Pyro is not Chromium or because the API is implemented differently. This page lists every known difference with its consequence and the fix. Read it before debugging the first failures of a migrated suite.

## Actions settle before they return

In Playwright, `click()` returns once the click has been dispatched. In Pyro, an action returns once the page has **settled**: no queued task, no request the action started still running, no timer due within 50 ms, no pending animation frame or navigation. The browser waits for this itself, in the same command as the action. See [Settling and auto-waiting](/guide/settling).

Things that do not hold up settling: an open WebSocket, timers on a paused [fake clock](/playwright/advanced#clock), and timers due more than 50 ms later.

Consequences:

- **Transient states are gone when the action returns.** A test that checks a spinner or a "Saving..." label between the click and the result can fail, because by the time `click()` returns, the save has finished:

  ```ts
  await page.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByText('Saving...')).toBeVisible();   // may time out: already "Saved"
  ```

  Assert the final state instead, or check the request:

  ```ts
  const saved = page.waitForResponse('**/api/save');
  await page.getByRole('button', { name: 'Save' }).click();
  expect((await saved).status()).toBe(200);
  await expect(page.getByText('Saved')).toBeVisible();
  ```

  If the transient state itself must be tested, slow the request down with a route (`route.fulfill` after a delay), so the state lasts.

- **Explicit waits become unnecessary.** `waitForLoadState('networkidle')` or `waitForTimeout()` after an action mostly wait for nothing. They still work.
- **A `waitForResponse` started before the action still resolves.** The response arrived during the action, and the listener saw it.

`focus()` and `blur()` do not wait for the page to settle.

## Dialogs

With a `page.on('dialog')` listener registered, a dialog that an action opens waits for the listener's answer, as in Chrome. Without a listener, the dialog is **dismissed**: `confirm()` returns `false`, `prompt()` returns `null`, `alert()` returns at once. This is Playwright's default too.

Register the listener **before** the action. If a listener answers a dialog that was already dismissed, a warning is printed once: `a dialog opened by an action or locator call was dismissed before the listener answered it`.

## The browser is not Chromium

- **User agent.** `navigator.userAgent` is `Mozilla/5.0 (compatible; pyro/<version>)`. Apps that sniff the user agent may show an "unsupported browser" screen or take a different code path. Set `userAgent` in `use` if needed (note that this disables [booted pages](/playwright/boot-pools)).
- **Every project runs on Pyro.** `browserName: 'firefox'` or `'webkit'` changes the `browserName` fixture, not the browser. A file runs in each project that matches it, so a Chromium, Firefox and WebKit matrix runs every test three times on the same browser.
- **Web APIs.** Coverage is broad (DOM, fetch, XHR, WebSocket, workers, IndexedDB, Web Crypto, Canvas 2D, observers, ...) but partial. See [Web platform support](/reference/web-platform). A missing API shows up as `undefined` in app code or a page error.
- **JavaScript engine.** Pyro runs JavaScriptCore, not V8. Error messages and stack traces are worded the way JavaScriptCore words them. Code that parses V8 stack traces sees a different format.
- **Fonts and rendering.** Pages are rendered by Pyro's CPU renderer with the operating system's fonts. Screenshots differ from Chromium's, and between macOS and Linux. See [Screenshots and snapshots](/playwright/snapshots).
- **Layout.** Layout is computed by Pyro's engine. Element sizes and positions (`boundingBox`, `toBeInViewport`, scroll positions) can differ slightly from Chromium's.

## Shadow DOM

Measured on version 0.0.33 and again on the current code: CSS selectors, `getByRole`, `getByLabel` and `getByPlaceholder` do not find elements inside open shadow roots; `getByText` does. Playwright pierces open shadow roots with all of them. Locate shadow DOM content by text.

## Context options

`use` and `browser.newContext()` take Playwright's context options with Playwright's meaning: `viewport`, `screen`, `deviceScaleFactor`, `isMobile`, `hasTouch`, `locale`, `timezoneId`, `geolocation`, `permissions`, `proxy`, `httpCredentials`, `offline`, `javaScriptEnabled`, `ignoreHTTPSErrors`, `bypassCSP`, `serviceWorkers`, `acceptDownloads`, `recordVideo` and the rest. Options you do not set have Playwright's defaults as fixtures (`isMobile`, `hasTouch` and `offline` are `false`; `javaScriptEnabled` and `acceptDownloads` are `true`). [Config and the test runner](/playwright/runner#use-options) describes each option.

The differences that remain:

| Option | Difference |
| --- | --- |
| `forcedColors`, `strictSelectors` | Accepted, no effect. |
| `proxy` | HTTP proxies only. A SOCKS or HTTPS `server` is an error. The credentials are sent with the first request (Chromium waits for a 407). `context.request` and the `request` fixture do not go through the proxy. |
| `timezoneId` | An error on the Intel macOS browser, which runs the system's JavaScriptCore. Each time zone gets a CDP connection of its own, opened in a few milliseconds. |
| `locale` | Sets `navigator.language(s)`, `Accept-Language` and the default locale of `Intl` and the `toLocale*String` methods in documents. `Intl` inside a worker keeps the host's locale. |
| `bypassCSP` | Pyro enforces no Content-Security-Policy directive in the first place: a policy blocks no script, style, image or connection. The option only lifts the hiding of the `nonce` attribute that a header-delivered policy causes. |
| `serviceWorkers: 'block'` | `navigator.serviceWorker.register()` registers nothing and resolves with `undefined`, after the console warning `Service Worker registration blocked by Playwright`. |
| `isMobile` | Pages lay out by `<meta name="viewport">`, as in Chrome's mobile emulation, and a page without one lays out 980 px wide. There is no pinch zoom, and touch input is not emulated beyond `navigator.maxTouchPoints` and the pointer media queries (`tap()` is a click). |
| `recordVideo` | Needs an ffmpeg (see [Video](/playwright/runner#video)). `recordVideo.showActions` and the runner's `video.show` draw no annotations. CSS animations show at their end state, as in screenshots. |

A context that sets any option changing how a page loads gets a fresh page instead of a [booted page](/playwright/boot-pools).

## Selector-first page methods and deprecated APIs

Playwright's selector-first methods on `Page` and `Frame` (`page.click(selector)`, `page.fill(selector, value)`, `page.check`, `page.hover`, `page.press`, `page.textContent(selector)`, `page.getAttribute(selector, name)`, `page.isVisible(selector)`, `page.dispatchEvent`, `page.dragAndDrop`, `page.selectOption`, ...) are not implemented. Calling one is a type error and a `TypeError` at run time. Use locators: `page.locator(selector).click()`.

Deprecated APIs are left out, so using one is a type error: `locator.type`, `elementHandle.type`, the `noWaitAfter` option, `page.accessibility`, `page.waitForNavigation`, and `FrameLocator.first()` / `last()` / `nth()`.

See [API coverage](/playwright/api-coverage) for the full list.

## Test runner

- **Plain `bun test` reports a run-time skip as a pass.** `test.skip()` inside a running test is reported as skipped by `pyro run`, but Bun cannot mark a running test as skipped, so plain `bun test` counts it as passed and prints `↳ skipped: <title>`.
- **A timed-out test's pending call is cancelled.** When a test times out, the call its test function waits on fails at once with `locator.click: Test timeout of 30000ms exceeded.` and its call log, and the pages stay as they were. Playwright leaves the call waiting until the page closes. `afterEach` hooks and fixture teardown then run in a time of their own, so a hook that takes a screenshot of the page works for a timed-out test too. If the browser does not answer the cancel within 2 s (a page script that never returns), the test's pages are closed instead.
- **The `screenshot` option does not capture a context that closes while the test still passes.** Playwright captures it in case the test fails later. Pyro captures the open pages right after the test function, and a context that closes once the outcome asks for a capture.
- **Parallel tests inside one worker run one after another.** `mode: 'parallel'` and `fullyParallel` spread tests over workers; within a worker they are sequential unless `PYRO_CONCURRENT=1`.
- **Serial groups retry the failed test only.** Playwright reruns the whole group.
- **A retry runs in the same worker.** Playwright retries in a new worker process, so module state and worker fixtures start over there; here they carry on.
- **Steps are not reported.** `test.step()` runs its body, `step.skip()` ends that step and `test.step.skip()` skips one; none of them appear in the output.
- **Unsupported flags fail.** `--trace`, `--repeat-each`, `--last-failed`, `--forbid-only`, `--output` and other flags not listed in [Config and the test runner](/playwright/runner#flags) exit with status 2.
- **The config's `reporter` is ignored**, and there is no HTML report or trace. The `trace` option is accepted and records nothing.

## Timeouts

- An action or navigation without a timeout waits up to **30 s**. In Playwright the default action timeout is 0 (no limit except the test timeout). Set `actionTimeout` / `navigationTimeout`, or `page.setDefaultTimeout()`, to change it.
- A single action's timeout is capped at 600 000 ms.

## Network

- **`page.request` and `context.request` do not share cookies with the browser context.** Each API request context keeps its own cookie jar. In Playwright they share one.
- **`route.fetch()` and API requests run in the test process**, with `fetch`, not in the browser.
- **The `Response` from `page.goto()` and `page.reload()`** is the document's response: status, status text, headers, the request's headers, the redirects on the way (`request().redirectedFrom()`) and `body()` / `text()` / `json()`. Without a `request` or `response` listener on the page, the headers come from the navigation's own record, which leaves out the cookie and credential headers (`set-cookie`, `authorization`, ...). Register a listener before navigating to get them all. `goBack()` and `goForward()` return `null`. A `goto` on a [booted page](/playwright/boot-pools) returns `null`.
- **`response.body()`** reads the body from the browser, which keeps a page's recent bodies only: read it soon after the response. A redirect's response and a response a route fulfilled have no body to read.
- **`request.frame()`** is always the main frame, and `request.timing()` fields are all -1.
- **`context.storageState()`** collects `localStorage` only from origins with an open page.
- **`storageState` seeding** leaves a `__pyroStorageState` key in `localStorage`.
- **`page.routeWebSocket()`** is not implemented.

## Assertions and locators

- **`toHaveRole`** reads the `role` attribute only. A `<button>` without `role="button"` fails `toHaveRole('button')`.
- **`toHaveAccessibleDescription`** reads `aria-description`, else `title`. **`toHaveAccessibleName`** with a RegExp or `ignoreCase` reads `aria-label`, else the text content; with a plain string it uses the browser's full name computation.
- **`toHaveScreenshot` threshold** is a per-channel tolerance with default 0 (exact). Playwright's is a perceptual distance with default 0.2. Options other than `threshold`, `maxDiffPixels`, `maxDiffPixelRatio`, `clip`, `fullPage`, `omitBackground` and `timeout` are rejected. The image is rendered at the context's `deviceScaleFactor`; Playwright's default there is `scale: 'css'`.
- **Strict mode violations** are worded differently: `strict mode violation: {"name":"Buy","role":"button"} matched 2 elements: <button> "Buy", <button> "Buy"`.
- **`getByTestId`, `getByAltText`, `getByTitle`** take strings only.
- **`click({ clickCount })`** supports 1 and 2. `delay` options are ignored. `tap()` is a click.
- **`selectors.setTestIdAttribute()`** does nothing; use the `testIdAttribute` option.

## Marker attributes in the DOM

The Playwright layer adds attributes to find elements across calls:

- `data-pyro-frame="<frame id>"` on an `<iframe>` once its frame has been listed (`page.frames()`, `frame.childFrames()`).
- `data-pyro-handle="<id>"` on an element for the length of an element handle action.

An app or a snapshot that serializes the DOM can see them.

## Other API details

- `ConsoleMessage.args()` returns an empty list. `text()` joins the arguments' values.
- `page.pdf()` passes its options to CDP `Page.printToPDF` unchanged: use `paperWidth`, `marginTop` and the other CDP names, in inches. Playwright's `format` and `margin` have no effect.
- `page.screenshot()` ignores `mask`, `animations`, `scale`, `caret` and `style`. The image is in device pixels (Playwright's default, `scale: 'device'`). `omitBackground` works for PNG. Pyro's own `raw` option returns raw pixels: see [Screenshots and snapshots](/playwright/snapshots#raw-frames).
- `page.pause()`, `page.bringToFront()` and `locator.highlight()` do nothing. `page.opener()` returns `null`, also for a popup. `page.video()` returns `null` unless the context has `recordVideo`.
- `download.cancel()` does nothing.
- `context.tracing.start()` / `stop()` do nothing.
- `browser.version()` returns `'pyro'`.
- `devices` is Playwright 1.59.1's table of descriptors, unchanged. Each descriptor's `userAgent`, `viewport`, `deviceScaleFactor`, `isMobile` and `hasTouch` take effect; its `defaultBrowserType` does not, since every project runs on Pyro.

## Next

- See these differences as failures, with fixes: [Triage the first red run](/recipes/playwright-triage-first-failures).
- Check a test against Chromium while you fix it: [Run the same suite on Chromium and Pyro](/recipes/playwright-side-by-side).
- Method by method: [API coverage](/playwright/api-coverage).
