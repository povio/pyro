# @povio/lightning-panda

The Lightningpanda browser CLI and typed testing API in one package.

E2E tests for `bun test` on Lightningpanda. A test sends its whole scenario as
one `LP.run` command: the browser runs every step itself (locating, acting,
waiting for the app to settle, checking) and replies once.

```ts
import { Lightningpanda, button, check, dialog, text } from '@povio/lightning-panda';

const lp = await Lightningpanda.connect('ws://127.0.0.1:9222/'); // or Lightningpanda.launch()
await lp.profile('employee', { url: 'http://localhost:3000/platform', localStorage: { token } });

test.concurrent('closes a position', async () => {
  const position = await api.createPosition();
  await using page = await lp.page({ as: 'employee', route: `/positions/${position.id}` });
  const r = await page.run([
    check(text(position.number)).visible(),
    button('Settings').press('Enter'),
    dialog().hasText('will be closed').button('Close Position', { exact: true }).click(),
    check.request('POST', `/positions/${position.id}/close`).status(200),
    check(button('Reopen Position')).count(1),
  ]);
});
```

- **Pages.** `lp.profile(name, …)` keeps signed-in pages booted ahead
  (`Lightningpanda.registerPreparedPage`); `lp.page({ as, route })` takes one
  and attaches in one call. `route` is a same-document navigation
  (`pushState` + `popstate`) run as the first step of the first `run`, so the
  app does not boot again. `url` navigates instead. With no page waiting, one
  is booted for the test.
- **Connections.** `connect(ws, { connections: 4 })` opens one browser thread
  per connection, each with its own pools. A test takes a page from the
  connection with the fewest open pages, and a connection boots replacements
  as soon as it has no open page, in parallel with tests on the others.
- **Fork mode.** Against `lightningpanda serve --fork` each `lp.page()` opens
  its own connection, which the server forks from a process that booted the
  profile once: the page starts booted in about 20 ms, shares nothing with
  other tests, and its process exits when the page closes. `connections` and
  `pool` then make no difference. `setup` steps in a profile run once on the
  booted page (sign-in flows, opening a view, filling filters) and every page
  starts from their result. `base: 'otherProfile'` builds a snapshot on top of
  another one: only its own `setup` runs, on a fork of that snapshot (boot on
  the login page once, sign in once, then one snapshot per test file).
- **Locators** are data: `css`, `testId`, `text`, `label`, `placeholder`,
  `role` (and `button`, `dialog`, `link`, …), chained inside one another
  (`dialog().button('OK')`), filtered (`.visible()`, `.hasText()`,
  `.hasNotText()`, `.has()`, `.hasNot()`) and picked (`.first()`, `.last()`,
  `.nth(i)`). Strings match case-insensitive substrings unless `{ exact: true }`;
  a `RegExp` (flags `i`, `m`, `s`) tests the whitespace-normalized text.
- **Steps**: `.click()`, `.fill(v)`, `.press('Enter' | 'Shift+Tab' | …)`,
  `.check()`, `.uncheck()`, `.focus()`, `.read(as)`; `check(loc).visible()`,
  `.hidden()`, `.text()`, `.containsText()`, `.value()`, `.count()`,
  `.checked()`, `.not.…`; `check.request(method, path).status(n)` /
  `.count(n)`; `goto(url)`, `route(url)`, `evaluate(expr, as)`.
- **Waiting** happens in the browser. A step that does not hold yet is tried
  again when the DOM changes; an action settles before the next step (the
  page goes quiet: no queued task, no request the action started, no timeout
  due within 50 ms, no pending animation frame or navigation); a request
  check waits for the request log. Each step has its own timeout (5 s default,
  `page.run(steps, { timeout })` or per step).
- **The reply** has `values` (reads), `results` (per step), `requests` (every
  request the page made during the run), `screenshots` and `ms`. A failure
  throws `RunError` naming the step and why it was still waiting.
- **Screenshots** (a browser built with the `render` feature):
  `matchScreenshot('home')`, `css('#card').matchScreenshot('card')` or
  `await page.matchScreenshot('home', { fullPage: true })` render the page in
  the browser (Blitz, CPU) and compare it there with
  `__screenshots__/home.png` beside the test file. A baseline is sent once
  per connection; a render whose display list matches the one recorded in
  `home.json` passes without rasterizing. A missing baseline is written and
  fails the test; `LP_UPDATE_SNAPSHOTS=1` (or `bun test -u`) rewrites
  baselines; a mismatch throws `ScreenshotMismatchError` and writes
  `home.actual.png` and `home.diff.png`. Options: `threshold` (0-1 per
  channel), `maxDiffPixels`, `maxDiffPixelRatio`, `clip`, `fullPage`, `dir`.
  `expect.extend(screenshotMatchers)` adds `await expect(page).toMatchScreenshot(name)`.
  Baselines come from lightningpanda's renderer: Chromium screenshots do not
  match them pixel for pixel.

## Playwright tests, unchanged

Playwright tests run on Lightningpanda by replacing the imports of
`playwright` and `@playwright/test` with `@povio/lightning-panda/playwright`.
Nothing else in a test changes, and Playwright itself is not needed:

```ts
import { test, expect, type Page } from '@povio/lightning-panda/playwright'; // was '@playwright/test'

test('closes a position', async ({ page }) => {
  await page.goto(`/positions/${id}`);
  await page.getByRole('button', { name: 'Settings', exact: true }).press('Enter');
  const done = page.waitForResponse((r) => r.url().endsWith('/close') && r.status() === 200);
  await page.getByRole('dialog').filter({ hasText: 'will be closed' })
    .getByRole('button', { name: 'Close Position', exact: true }).click();
  await done;
  await expect(page.getByRole('button', { name: 'Reopen Position' })).toHaveCount(1);
});
```

```sh
npx lightningpanda test                      # as `npx playwright test`
npx lightningpanda test -c playwright.e2e.config.ts positions-close -j 4
```

`lightningpanda test` takes Playwright's usual flags (`-c`, `--project`,
`-j/--workers`, `-g/--grep`, `--grep-invert`, `--retries`, `--timeout`,
`-u`, `-x`, `--max-failures`, `--list`, `--reporter list|line|dot`) and file
filters. It needs [Bun](https://bun.sh) on PATH: the tests run in `bun test`
worker processes.

The module is the whole of Playwright's API, implemented here:

- **The library:** `chromium` (`launch`, `connectOverCDP`), Browser,
  BrowserContext, Page, Frame, Locator, FrameLocator, handles, Request,
  Response, Route, Download, Dialog, Worker, FileChooser, WebSocket,
  `request` (APIRequestContext), `devices`, `errors`, `selectors`.
- **The runner, on `bun test`:** `test`, `test.describe` (`only`, `skip`,
  `serial`, `parallel`, `configure`), `beforeEach`/`afterEach`/
  `beforeAll`/`afterAll` with fixtures, `test.extend` (test and worker
  fixtures, options, `auto`, overrides that use the fixture they replace),
  `test.use`, `test.step`, `test.info()`, `skip`/`fixme`/`fail`/`slow`,
  retries and timeouts. Built-in fixtures: `browser`, `context`, `page`,
  `request`, `playwright`, `browserName` and every context option.
- **`expect`:** every web-first matcher on locators, pages (`toHaveURL`,
  `toHaveTitle`, `toHaveScreenshot`) and API responses (`toBeOK`), plus
  `.not`, `.soft`, `.poll`, `.configure`, `.extend` and `toPass`. Other
  matchers and the asymmetric ones (`expect.any`, `objectContaining`, ...)
  are Bun's, which follow Jest as Playwright's do.
- **The config file:** `defineConfig` with `use` (the config's, then the
  project's), `projects` (`$LP_PROJECT`, else the first), `timeout`,
  `retries`, `expect.timeout`, `outputDir`, `webServer` (started, waited for
  and stopped; `reuseExistingServer`), `globalSetup` and `globalTeardown`. The
  file is `$LP_CONFIG`, else `lightningpanda.config.*` or `playwright.config.*`
  in the working directory.

The browser is `$LIGHTNINGPANDA_WS`, else one launched per run
(`$LIGHTNINGPANDA_BIN` or the bundled one), configurable with `use: {
lightningpanda: { wsEndpoint, bin, args, boot } }`. Each awaited call is one
`LP.run` command, so locating, the actionability checks, auto-waiting and
settling run in the browser.

How a run goes, as with `playwright test`:

- **Files:** each project's `testDir`, `testMatch` (default
  `**/*.@(spec|test).?(c|m)[jt]s?(x)`; a pattern without `/` matches file
  names, so TOS's `*.e2e.ts` lists work) and `testIgnore`. Each file gets its
  own project's `use`.
- **Workers:** `workers` processes (the config's, `-j`, else half the CPUs),
  each with its own browser and module state. A file's tests run in order in
  one worker. Tests that are parallel (`fullyParallel`, or a describe with
  `mode: 'parallel'`) spread over the workers, and a describe's `beforeAll`
  and `afterAll` run in each worker that runs some of its tests. Web servers
  and global setup run once, in the parent.
- **Reporting:** passes, failures (with their errors), skips (declared or
  `test.skip()` at run time), flaky tests (passed on a retry) and expected
  failures, then a summary. The tests' own output is shown per worker.

Plain `bun test ./e2e/foo.e2e.ts` (with `$LP_CONFIG` naming the config) also
runs the tests, in one process: Bun then reports a run-time `test.skip()` as
a pass, since it cannot mark a running test as skipped. Not there either way:
Playwright's HTML report, trace viewer, video and sharding.

The TOS close flow (`test/playwright/tos-flow.ts`, Playwright code as TOS
writes it) takes 14.7 ms per test this way (median of 40, fresh page),
against 41–44 ms for stock Playwright driving the same browser over CDP.
Actions settle before they return (the next call starts once the page is
quiet), so a test that asserts a transient state between an action and its
result (a spinner) can miss it.

**Pages booted ahead.** A page gets its target on its first command. With

```ts
// playwright.config.ts
use: { lightningpanda: { boot: [{ url: '/platform', routes: '/platform/**' }] } }
```

a context's first page whose first call is `goto` to a `routes` match (or to
`url` itself) is taken from a pool of pages that already loaded `url` with
the context's init scripts, `storageState` local storage and viewport, and
the `goto` becomes a same-document route change (`pushState` + `popstate`,
settled), returning `null`. The app's shell was booted before the test
seeded anything, so use it for routes that fetch their data when they
mount. Contexts with cookies, `extraHTTPHeaders`, `userAgent` or media
emulation, and any other first call, get a fresh page.

Also covered:

- element and JS handles (`page.$`, `$$`, `evaluateHandle`,
  `locator.elementHandle()`, handles as `evaluate` arguments), in the main
  frame and in child frames; element handle actions run natively on the
  element, which carries a `data-lp-handle` attribute for the length of the
  action
- `locator.and()`/`or()`, filters and `nth()` after `first()/last()/nth()`,
  every `getByRole` option, XPath
- selector strings: `css=`, `text=`, `role=`, `id=`, `xpath=`, `data-testid=`,
  `nth=`, `visible=`, `>>` chains, the `internal:` engines Playwright's
  locators serialize to (`internal:role`, `:text`, `:label`, `:attr`,
  `:testid`, `:has`, `:has-not`, `:has-text`, `:has-not-text`, `:and`, `:or`,
  `:chain`, `:control=enter-frame`), and CSS with `:has-text()`, `:text()`,
  `:text-is()`, `:text-matches()`, `:visible`, `:nth-match()` and `:light()`
- popups (`page.on('popup')`, `context.on('page')`), downloads
  (`page.on('download')`, `saveAs`, `path`), file choosers
  (`page.on('filechooser')`, `setFiles`), workers (`page.on('worker')`,
  `worker.evaluate`)
- `keyboard.down/up`, click modifiers
- `page.frames()` and child frames (`goto`, `waitForURL`, `waitForFunction`,
  locators, handles; their `<iframe>` gets a `data-lp-frame` attribute once
  listed)
- `exposeFunction`/`exposeBinding`, `routeFromHAR` replay and recording
  (`update: true`, written when the context closes; bodies beside the file
  unless `updateContent: 'embed'`)
- with a `page.on('dialog')` listener, a dialog an action opens waits for its
  answer as in Chrome; without one it is dismissed
- WebSockets (`page.on('websocket')`, `url()`, `isClosed()`, `framesent`,
  `framereceived`, `socketerror`, `close`, `waitForEvent`); an open socket
  does not hold up an action's settling

- `context.newCDPSession(page)`, `page.request`/`context.request`, context
  events (`request`, `response`, `console`, `page`, `close`) and
  `recordHar` in the context options

Types are Playwright's own shapes, checked by `bun run test:types`:
`expect()` offers Locator, Page or APIResponse matchers by what it is given
(`toPass` only on functions), `expect.extend` matchers are typed from their
arguments, `getByRole` takes the ARIA role union, and `evaluate`/`$eval`
infer argument and return types. Deprecated APIs (`locator.type`,
`elementHandle.type`, `noWaitAfter`, `page.accessibility`, FrameLocator
`first`/`last`/`nth`) are left out, so using one is a type error. Not
implemented yet: `page.clock`, `page.coverage`, `toMatchSnapshot` and ARIA
snapshots.

Not supported (a clear error): the deprecated layout pseudo-classes
(`:left-of()`, `:right-of()`, `:above()`, `:below()`, `:near()`); a `+` or `~`
combinator right after a Playwright pseudo-class.

## Install

```sh
npm install --save-dev @povio/lightning-panda
# Or: bun add --dev @povio/lightning-panda
npx lightningpanda serve --port 9222
```

For release candidates, install `@povio/lightning-panda@next`.

The package bundles browsers for **macOS 14+ on Apple Silicon and Intel**, with
layout, screenshots and PDF rendering enabled. No Rust toolchain, install
scripts or binary downloads are needed. The API supports Node.js 22+ and Bun;
Bun loads TypeScript sources, while Node loads compiled JavaScript and types.
On other operating systems, use `Lightningpanda.connect()` with a remote
browser. Installing/importing the client does not launch a local browser.

`Lightningpanda.launch()` selects `bin`, then `$LIGHTNINGPANDA_BIN`, then the
browser beside the package in a release archive, then the bundled browser for
the current architecture. The CLI also respects `$LIGHTNINGPANDA_BIN`.

On Apple silicon the bundled browser is the **fork-capable build** with its
patched JavaScriptCore included, so `args: ['--fork']` works without a separate
browser. The Intel browser uses macOS's system JavaScriptCore and has no fork
mode.

The package is licensed under AGPL-3.0-only; see the included `LICENSE` and
`NOTICE`. Maintainers: see
[publishing instructions](https://github.com/povio/lightningpanda/blob/main/docs/NPM_PUBLISHING.md).

Coding agents: read [LLM.md](LLM.md) before writing tests.

`examples/positions-close.test.ts` is TOS's `positions-close.e2e.ts` ported.
`bench/scripts/cdp-perf.mjs --filter '^(pw\.)?tos\.'` runs the same flow
against a TOS-shaped app in the bench site, as Playwright, step by step, as one
run, and as one run on a pre-booted page.
