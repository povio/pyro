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

- **User agent.** `navigator.userAgent` is `Mozilla/5.0 (compatible; lightningpanda/<version>)`. Apps that sniff the user agent may show an "unsupported browser" screen or take a different code path. Set `userAgent` in `use` if needed (note that this disables [booted pages](/playwright/boot-pools)).
- **Every project runs on Pyro.** `browserName: 'firefox'` or `'webkit'` changes the `browserName` fixture, not the browser. Each file runs once, under its first project.
- **Web APIs.** Coverage is broad (DOM, fetch, XHR, WebSocket, workers, IndexedDB, Web Crypto, Canvas 2D, observers, ...) but partial. See [Web platform support](/reference/web-platform). A missing API shows up as `undefined` in app code or a page error.
- **JavaScript engine.** Pyro runs JavaScriptCore, not V8. Error messages and stack traces are worded the way JavaScriptCore words them. Code that parses V8 stack traces sees a different format.
- **Fonts and rendering.** Pages are rendered by Pyro's CPU renderer with the operating system's fonts. Screenshots differ from Chromium's, and between macOS and Linux. See [Screenshots and snapshots](/playwright/snapshots).
- **Layout.** Layout is computed by Pyro's engine. Element sizes and positions (`boundingBox`, `toBeInViewport`, scroll positions) can differ slightly from Chromium's.

## Shadow DOM

Measured on version 0.0.33: CSS selectors, `getByRole`, `getByLabel` and `getByPlaceholder` do not find elements inside open shadow roots; `getByText` does. Playwright pierces open shadow roots with all of them. Locate shadow DOM content by text.

## Context options without effect

These `use` / `newContext` options are accepted and do nothing to pages:

| Option | Alternative |
| --- | --- |
| `locale`, `timezoneId` | None in the Playwright layer. |
| `geolocation`, `permissions`, `context.grantPermissions()`, `setGeolocation()` | None. |
| `offline`, `context.setOffline()` | A route that aborts every request. |
| `proxy` | `use.lightningpanda.args: ['--proxy', url]`. |
| `httpCredentials` | Works for `request` only. For pages, send an `authorization` header with `extraHTTPHeaders`. |
| `ignoreHTTPSErrors` | Works for `request` only. |
| `deviceScaleFactor`, `isMobile`, `hasTouch` | None. Pages render at scale 1. |
| `javaScriptEnabled: false`, `bypassCSP`, `serviceWorkers` | None. |
| `recordVideo` | None. |

Options you do not set are `undefined`, not `false` as in Playwright. `test.skip(isMobile)` therefore skips every test unless `isMobile` is set.

## Selector-first page methods and deprecated APIs

Playwright's selector-first methods on `Page` and `Frame` (`page.click(selector)`, `page.fill(selector, value)`, `page.check`, `page.hover`, `page.press`, `page.textContent(selector)`, `page.getAttribute(selector, name)`, `page.isVisible(selector)`, `page.dispatchEvent`, `page.dragAndDrop`, `page.selectOption`, ...) are not implemented. Calling one is a type error and a `TypeError` at run time. Use locators: `page.locator(selector).click()`.

Deprecated APIs are left out, so using one is a type error: `locator.type`, `elementHandle.type`, the `noWaitAfter` option, `page.accessibility`, `page.waitForNavigation`, and `FrameLocator.first()` / `last()` / `nth()`.

See [API coverage](/playwright/api-coverage) for the full list.

## Test runner

- **Plain `bun test` reports a run-time skip as a pass.** `test.skip()` inside a running test is reported as skipped by `lightningpanda test`, but Bun cannot mark a running test as skipped, so plain `bun test` counts it as passed and prints `↳ skipped: <title>`.
- **`testInfo.status` in hooks.** `testInfo.status`, `error`, `errors` and `duration` are not updated while a test runs. In `afterEach`, `status` is `'passed'` even after a failure. A hook like this never takes its screenshot:

  ```ts
  test.afterEach(async ({ page }, testInfo) => {
    if (testInfo.status !== testInfo.expectedStatus) await page.screenshot({ path: `failed-${testInfo.title}.png` });
  });
  ```

  Take such screenshots unconditionally into `testInfo.outputPath()`, or catch the error in the test body and record it yourself.
- **Worker fixtures are set up per file**, not per worker process. The built-in `browser` lives for the whole worker.
- **Parallel tests inside one worker run one after another.** `mode: 'parallel'` and `fullyParallel` spread tests over workers; within a worker they are sequential unless `LP_CONCURRENT=1`.
- **Serial groups retry the failed test only.** Playwright reruns the whole group.
- **`step.skip()` skips the whole test.** Steps are not reported.
- **Unsupported flags fail.** `--shard`, `--trace`, `--repeat-each`, `--last-failed` and other flags not listed in [Config and the test runner](/playwright/runner#flags) exit with status 2.
- **The config's `reporter` is ignored**, and there is no HTML report, trace or video.
- **Project `dependencies` do not run.** Move setup projects into `globalSetup`.

## Timeouts

- An action or navigation without a timeout waits up to **30 s**. In Playwright the default action timeout is 0 (no limit except the test timeout). Set `actionTimeout` / `navigationTimeout`, or `page.setDefaultTimeout()`, to change it.
- A single action's timeout is capped at 600 000 ms.

## Network

- **`page.request` and `context.request` do not share cookies with the browser context.** Each API request context keeps its own cookie jar. In Playwright they share one.
- **`route.fetch()` and API requests run in the test process**, with `fetch`, not in the browser.
- **The `Response` from `page.goto()`** has the URL and status only: `headers()` is empty and `body()` throws. `page.reload()` is the same. `goBack()` and `goForward()` return `null`. A `goto` on a [booted page](/playwright/boot-pools) returns `null`.
- **`request.frame()`** is always the main frame, and `request.timing()` fields are all -1.
- **`context.storageState()`** collects `localStorage` only from origins with an open page.
- **`storageState` seeding** leaves a `__lpStorageState` key in `localStorage`.
- **`page.routeWebSocket()`** is not implemented.

## Assertions and locators

- **`toHaveRole`** reads the `role` attribute only. A `<button>` without `role="button"` fails `toHaveRole('button')`.
- **`toHaveAccessibleDescription`** reads `aria-description`, else `title`. **`toHaveAccessibleName`** with a RegExp or `ignoreCase` reads `aria-label`, else the text content; with a plain string it uses the browser's full name computation.
- **`toHaveScreenshot` threshold** is a per-channel tolerance with default 0 (exact). Playwright's is a perceptual distance with default 0.2. Options other than `threshold`, `maxDiffPixels`, `maxDiffPixelRatio`, `clip`, `fullPage` and `timeout` are rejected.
- **Strict mode violations** are worded differently: `strict mode violation: {"name":"Buy","role":"button"} matched 2 elements: <button> "Buy", <button> "Buy"`.
- **`getByTestId`, `getByAltText`, `getByTitle`** take strings only.
- **`click({ clickCount })`** supports 1 and 2. `delay` options are ignored. `tap()` is a click.
- **`selectors.setTestIdAttribute()`** does nothing; use the `testIdAttribute` option.

## Marker attributes in the DOM

The Playwright layer adds attributes to find elements across calls:

- `data-lp-frame="<frame id>"` on an `<iframe>` once its frame has been listed (`page.frames()`, `frame.childFrames()`).
- `data-lp-handle="<id>"` on an element for the length of an element handle action.

An app or a snapshot that serializes the DOM can see them.

## Other API details

- `ConsoleMessage.args()` returns an empty list. `text()` joins the arguments' values.
- `page.pdf()` passes its options to CDP `Page.printToPDF` unchanged: use `paperWidth`, `marginTop` and the other CDP names, in inches. Playwright's `format` and `margin` have no effect.
- `page.screenshot()` ignores `mask`, `animations`, `omitBackground`, `scale`, `caret` and `style`.
- `page.pause()`, `page.bringToFront()` and `locator.highlight()` do nothing. `page.video()` and `page.opener()` return `null`.
- `context.tracing.start()` / `stop()` do nothing.
- `browser.version()` returns `'lightningpanda'`.
- `devices` has five entries: `Desktop Chrome`, `Desktop Chrome HiDPI`, `Desktop Edge`, `Pixel 7`, `iPhone 15`.
