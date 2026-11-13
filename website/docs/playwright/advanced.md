# Clock, coverage, ARIA and more

This page covers the parts of the API beyond locators and assertions: the fake clock, JS and CSS coverage, ARIA snapshots (including AI mode), raw CDP sessions, downloads, popups, dialogs, file choosers, workers and handles. Each section says what matches Playwright and where it differs.

## Clock

```ts
test('session expires after 30 minutes', async ({ page }) => {
  await page.clock.install({ time: new Date('2026-01-01T09:00:00Z') });
  await page.goto('/dashboard');
  await page.clock.fastForward('30:00');
  await expect(page.getByText('Session expired')).toBeVisible();
});
```

`page.clock` and `context.clock` are the same clock, shared by a context's pages. Methods: `install({ time })`, `fastForward(ticks)`, `runFor(ticks)`, `pauseAt(time)`, `resume()`, `setFixedTime(time)`, `setSystemTime(time)`. Times take a number (ms since the epoch), a date string or a `Date`; ticks take milliseconds or `'mm:ss'` / `'hh:mm:ss'` strings.

On the first call, every document of the context gets a fake clock that replaces `Date`, `Intl.DateTimeFormat`, `setTimeout` / `setInterval`, `requestAnimationFrame`, `requestIdleCallback`, `performance.now`, `AbortSignal.timeout` and `Event.timeStamp`. As in Playwright, there are two times: monotonic ticks drive timers and `performance.now`, wall time drives `Date`. `runFor`, `fastForward`, `pauseAt` and real time move both; `setSystemTime` and `setFixedTime` move or freeze only the wall time.

Every call runs in every frame of every open page, and is also recorded as an init script, so a document loaded later lands on the same fake time.

Fake timers are not the browser's timers, so **a paused clock does not hold up an action's settling**. With a paused clock, `click()` returns once the real page is quiet; timers the app scheduled on the fake clock fire only when you advance it.

## Coverage

```ts
test('collects coverage', async ({ page }) => {
  await page.coverage.startJSCoverage();
  await page.coverage.startCSSCoverage();
  await page.goto('/');
  await page.getByRole('button', { name: 'Open menu' }).click();
  const js = await page.coverage.stopJSCoverage();
  const css = await page.coverage.stopCSSCoverage();
  // js: [{ url, scriptId, source, functions: [{ functionName, isBlockCoverage, ranges }] }]
  // css: [{ url, text, ranges: [{ start, end }] }]
});
```

The entries are in the format Playwright returns on Chromium (V8's), so code that post-processes Chromium coverage receives the same shape.

- **JS**: block coverage per script, with its source. Options: `resetOnNavigation` (default `true`) and `reportAnonymousScripts` (scripts from `eval` and `new Function`; default `false`). Anonymous functions carry V8's inferred names (`ns.a = function () {}` is `ns.a`).
- **CSS**: the used rule ranges of each style sheet that has a URL, with its text. Constructed style sheets are not reported.

Caveats:

- **Start before `goto` for whole scripts.** Started after the page ran, coverage reports what Chrome does: counts start at zero, and a function is listed only when it ran since. A script's top level and functions that do not run again are left out.
- **One gap**: an async function or generator that was suspended when coverage started and resumes after it is not reported.
- **`source` can be missing** when `stopJSCoverage()` is called right after `goto`: script sources are fetched in the background and may not have arrived. Another action, an assertion or a short wait before stopping avoids it.
- Turning JS coverage on or off makes the engine recompile the page's code once. Expect the first actions after `startJSCoverage()` to be slower.

## ARIA snapshots

```ts
test('navigation structure', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('navigation')).toMatchAriaSnapshot(`
    - navigation "Main":
      - link "Home"
      - link /About/
  `);
});
```

`locator.ariaSnapshot()` and `page.ariaSnapshot()` return the accessibility tree as Playwright's YAML. `expect(locator).toMatchAriaSnapshot(template)` polls until the tree matches the template. The template language is Playwright's: partial matching, regular expressions (`/About/`), `/children: contain | equal | deep-equal`, property lines such as `/placeholder`, and the state attributes `[checked]`, `[disabled]`, `[expanded]`, `[level=N]`, `[pressed]`, `[selected]`.

Without a template, the snapshot is read from a file:

```ts
await expect(page.getByRole('main')).toMatchAriaSnapshot();                    // anonymous <title> <n>.aria.yml
await expect(page.getByRole('main')).toMatchAriaSnapshot({ name: 'main.aria.yml' });
```

Files live at `{snapshotDir}/{testFileDir}/{testFileName}-snapshots/{arg}{ext}` unless `expect.toMatchAriaSnapshot.pathTemplate` or `snapshotPathTemplate` says otherwise. A missing file is written in `missing` mode; `-u` rewrites a failing one and `-u all` rewrites all, as regex templates (the codegen form). An empty file counts as missing. The default `/children` mode comes from `expect.toMatchAriaSnapshot.children` in the config.

The tree is built in the browser from the same role and name code that `getByRole` uses. CSS `::before` and `::after` text is included (strings, `attr()`, and alt text after `/`); counters, quotes and images add nothing, as in Playwright.

```ts
console.log(await page.ariaSnapshot());
```

```yaml
- heading "Todos" [level=1]
- text: New todo
- textbox "New todo":
  - /placeholder: What needs doing?
- button "Add"
- list
```

### AI mode and aria-ref

`ariaSnapshot({ mode: 'ai' })` produces Playwright's AI-mode snapshot: each element that can be targeted gets a ref, plus `[active]` for the focused element, `[cursor=pointer]`, generic containers, and the snapshots of iframes nested under their `iframe` node (refs inside them look like `f1e5`).

```yaml
- generic [active] [ref=e1]:
  - heading "Todos" [level=1] [ref=e2]
  - generic [ref=e3]:
    - text: New todo
    - textbox "New todo" [ref=e4]:
      - /placeholder: What needs doing?
  - button "Add" [ref=e5]
```

`page.locator('aria-ref=e5')` finds the element of a ref from the last AI snapshot of its frame. This is how an agent can act on what it read:

```ts
const snapshot = await page.ariaSnapshot({ mode: 'ai' });
// ... an agent picks "e5" from the snapshot ...
await page.locator('aria-ref=e5').click();
```

In AI mode, `locator.ariaSnapshot()` does not wait: it fails at once when the locator matches nothing, as Playwright does. The `depth` option limits the tree in both modes. See [Writing tests with AI agents](/e2e/ai-agents).

## CDP sessions

```ts
const cdp = await page.context().newCDPSession(page);
const { result } = await cdp.send('Runtime.evaluate', { expression: '1 + 1', returnByValue: true });
cdp.on('Network.responseReceived', (e) => console.log(e.response.url));
await cdp.detach();
```

`context.newCDPSession(pageOrFrame)` returns a session on the page's target with `send`, `on`, `once`, `off` and `detach`. It speaks Pyro's CDP: its own domains and a subset of Chrome's. See [CDP coverage](/reference/cdp) and the [Pyro CDP domain](/reference/cdp-domain). `browser.newBrowserCDPSession()` is not implemented.

## Downloads

```ts
const [download] = await Promise.all([
  page.waitForEvent('download'),
  page.getByRole('link', { name: 'Export CSV' }).click(),
]);
await download.saveAs('out/export.csv');
```

`Download` has `page`, `url`, `suggestedFilename`, `path`, `saveAs`, `failure`, `createReadStream` and `delete`. `cancel()` does nothing. Files land in a temporary directory that is removed when the browser closes, and `saveAs` makes missing directories. `acceptDownloads` defaults to `true`. With `acceptDownloads: false` nothing is saved: the event still fires, `download.failure()` returns Playwright's `Pass { acceptDownloads: true } when you are creating your browser context.`, and `path()`, `saveAs()` and `createReadStream()` throw it. Both an HTTP response with `Content-Disposition: attachment` and a click on an `<a download>` link to a `data:` URL raise the event.

## Popups

```ts
const [popup] = await Promise.all([
  page.waitForEvent('popup'),
  page.getByRole('link', { name: 'Open in new tab' }).click(),
]);
await expect(popup).toHaveURL(/\/report/);
```

A target opened by a page (`window.open`, `target="_blank"`) becomes a page of the opener's context. It is announced by `page.on('popup')` and `context.on('page')` once the popup's document has committed, so `popup.url()` is its own URL. A popup has the context's options, init scripts, exposed functions, routes and extra headers from its first document on. `page.opener()` always returns `null`.

A popup whose navigation is answered with an attachment becomes a download on the opener, and the popup closes, as in Chrome. A popup whose navigation fails shows Chromium's error document, `chrome-error://chromewebdata/`.

## Dialogs

```ts
page.on('dialog', (dialog) => dialog.accept());
await page.getByRole('button', { name: 'Delete all' }).click();
```

With a `dialog` listener, a dialog an action opens waits for the listener's answer, as in Chrome. `Dialog` has `type`, `message`, `defaultValue`, `page`, `accept(promptText?)` and `dismiss()`.

**Without a listener, a dialog is dismissed**: `confirm()` returns `false`, `prompt()` returns `null`. This matches Playwright's default, but check suites that relied on a listener registered after the action. See [Behaviour differences](/playwright/differences#dialogs).

## File choosers

```ts
const [chooser] = await Promise.all([
  page.waitForEvent('filechooser'),
  page.getByRole('button', { name: 'Upload' }).click(),
]);
await chooser.setFiles({ name: 'a.txt', mimeType: 'text/plain', buffer: Buffer.from('hello') });
```

`FileChooser` has `page`, `element`, `isMultiple` and `setFiles` (paths or payloads). Simpler: `locator.setInputFiles()` on the `<input type="file">`.

## Workers

```ts
const [worker] = await Promise.all([
  page.waitForEvent('worker'),
  page.goto('/with-worker'),
]);
console.log(worker.url(), await worker.evaluate(() => self.name));
```

`page.on('worker')`, `page.workers()`, and `Worker` with `url`, `evaluate`, `evaluateHandle`, `isClosed`, the `close` event and `waitForEvent`. Dedicated workers only; service workers are not exposed.

## Element and JS handles

```ts
const button = await page.$('button.primary');
await button?.click();
const body = await page.evaluateHandle(() => document.body);
const tag = await page.evaluate((el) => el.tagName, body);
await body.dispose();
```

`page.$`, `page.$$`, `page.$eval`, `page.$$eval`, `evaluateHandle`, `locator.elementHandle()` / `elementHandles()`, and handles inside `evaluate` arguments all work, in the main frame and in child frames.

- `JSHandle`: `evaluate`, `evaluateHandle`, `jsonValue`, `getProperty`, `getProperties`, `asElement`, `dispose` (also `await using`).
- `ElementHandle` actions (`click`, `fill`, `check`, `press`, `selectOption`, `setInputFiles`, `hover`, `tap`, `focus`, `dispatchEvent`, `screenshot`, ...) run as native actions on the element. For the length of the action the element carries a `data-pyro-handle` attribute.
- `ElementHandle.$` and `$$` take CSS only.
- `waitForElementState('stable')` returns at once.
- `ElementHandle.waitForSelector` is not implemented.

Prefer locators: handles are kept for code that already uses them.

## Keyboard and mouse

`page.keyboard`: `press`, `type`, `insertText`, `down`, `up`. `press` and `type` act on the focused element (the body when nothing has focus). Keys held with `down()` (Shift, Control, Alt, Meta) apply to later clicks and presses until `up()`. `delay` is ignored.

`page.mouse`: `click`, `dblclick`, `move`, `down`, `up`, `wheel`. `page.touchscreen` is not implemented.

## Next

- Every known difference: [Behaviour differences](/playwright/differences).
- Method-by-method status: [API coverage](/playwright/api-coverage).
