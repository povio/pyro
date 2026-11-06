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

Existing Playwright tests run on Lightningpanda by swapping one import and
keeping Playwright's runner, config, reporters and `expect`:

```ts
import { test, expect } from '@povio/lightning-panda/test'; // was '@playwright/test'

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

`browser`, `context` and `page` become Lightningpanda's (one browser per
worker: `$LIGHTNINGPANDA_WS`, else launched with `$LIGHTNINGPANDA_BIN` or the
bundled one; `use: { lightningpanda: { wsEndpoint, bin, args } }` in the
config). Each awaited call is one `LP.run` command, so locating, the
actionability checks, auto-waiting and settling run natively in the browser.
`expect` stays Playwright's: its locator matchers call the locators'
`_expect`, which maps them onto native checks (or retries a value read in the
page), so `expect` imported from `@playwright/test` in helper files works
too. Config options honoured: `baseURL`, `viewport`, `storageState`,
`extraHTTPHeaders`, `userAgent`, `colorScheme`, `actionTimeout`,
`navigationTimeout`, `testIdAttribute`. The `request` fixture is Playwright's.
`page.route` and `context.route` (`fulfill`, `continue`, `abort`,
`fallback`, `fetch`, `times`; globs, RegExps and predicates) run on the
browser's request interception. `@povio/lightning-panda/playwright` has the
same classes for scripts (`chromium.launch()`, `chromium.connectOverCDP(ws)`).

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

The TOS close flow (`test/playwright/tos-flow.ts`, Playwright code as TOS
writes it) takes 23 ms per test on booted pages, 29-33 ms on fresh pages and
55-67 ms for stock Playwright driving the same browser over CDP (Playwright
runner, one worker, `REPEAT=40`, medians of two rounds on a loaded machine).
Actions settle before they return (the next call starts once the page is
quiet), so a test that asserts a transient state between an action and its
result (a spinner) can miss it.

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
