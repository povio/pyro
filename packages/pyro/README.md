# @povio/pyro

The Pyro browser CLI and typed testing API in one package.

E2E tests for `bun test` on Pyro. A test sends its whole scenario as
one `Pyro.run` command: the browser runs every step itself (locating, acting,
waiting for the app to settle, checking) and replies once.

```ts
import { Pyro, button, check, dialog, text } from '@povio/pyro';

const pyro = await Pyro.connect('ws://127.0.0.1:9222/'); // or Pyro.launch()
await pyro.profile('employee', { url: 'http://localhost:3000/platform', localStorage: { token } });

test.concurrent('closes a position', async () => {
  const position = await api.createPosition();
  await using page = await pyro.page({ as: 'employee', route: `/positions/${position.id}` });
  const r = await page.run([
    check(text(position.number)).visible(),
    button('Settings').press('Enter'),
    dialog().hasText('will be closed').button('Close Position', { exact: true }).click(),
    check.request('POST', `/positions/${position.id}/close`).status(200),
    check(button('Reopen Position')).count(1),
  ]);
});
```

- **Pages.** `pyro.profile(name, …)` keeps signed-in pages booted ahead
  (`Pyro.registerPreparedPage`); `pyro.page({ as, route })` takes one
  and attaches in one call. `route` is a same-document navigation
  (`pushState` + `popstate`) run as the first step of the first `run`, so the
  app does not boot again. `url` navigates instead. With no page waiting, one
  is booted for the test.
- **Connections.** `connect(ws, { connections: 4 })` opens one browser thread
  per connection, each with its own pools. A test takes a page from the
  connection with the fewest open pages, and a connection boots replacements
  as soon as it has no open page, in parallel with tests on the others.
- **Fork mode.** Against `pyro serve --fork` each `pyro.page()` opens
  its own connection, which the server forks from a process that booted the
  profile once: the page starts booted in about 20 ms, shares nothing with
  other tests, and its process exits when the page closes. `connections` and
  `pool` then make no difference. `setup` steps in a profile run once on the
  booted page (sign-in flows, opening a view, filling filters) and every page
  starts from their result. `base: 'otherProfile'` builds a snapshot on top of
  another one: only its own `setup` runs, on a fork of that snapshot (boot on
  the login page once, sign in once, then one snapshot per test file).
- **One server, many workers.** A fork-mode server keeps profiles by name
  for all its clients. Launch it once (`Pyro.launch({ args:
  ['--fork'] })` in the process that starts `bun test`), register the shared
  profiles there, and hand `pyro.wsEndpoint` to the workers, which
  `Pyro.connect()` to it. A worker's `pyro.page({ as: 'app' })` or
  `base: 'app'` then uses the profile the launcher booted (`pyro.adopt(name)`
  does it explicitly); `pyro.profile(name, def)` with the same definition
  adopts instead of booting again, and a different definition under a name
  another client holds is refused. A client holds the profiles it registered
  or adopted until `releaseProfile(name)` or until it disconnects, and the
  server retires a profile once no client holds it and no other profile
  derives from it. A launched server exits with the process that launched
  it, however that process ends.
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
  fails the test; `PYRO_UPDATE_SNAPSHOTS=1` (or `bun test -u`) rewrites
  baselines; a mismatch throws `ScreenshotMismatchError` and writes
  `home.actual.png` and `home.diff.png`. Options: `threshold` (0-1 per
  channel), `maxDiffPixels`, `maxDiffPixelRatio`, `clip`, `fullPage`, `dir`.
  `expect.extend(screenshotMatchers)` adds `await expect(page).toMatchScreenshot(name)`.
  Baselines come from pyro's renderer: Chromium screenshots do not
  match them pixel for pixel.

## One process for the whole suite: `pyro e2e`

`bun test` runs one file's tests after another's in a process, so overlapping
cases of different files takes worker processes, each with its own browser
and profiles. `pyro e2e` runs every spec file in one process
instead: tests of all files share one queue (up to `--concurrency` at a time)
and one fork-mode browser, so every profile boots once and every case forks.

```ts
// positions.test.ts
import { beforeAll, browser, defer, describe, expect, test } from '@povio/pyro/e2e';
import { button, check, text } from '@povio/pyro';

describe('positions', () => {
  beforeAll(async () => {
    const pyro = await browser();                        // the run's browser
    await pyro.profile('positions', { url, setup });     // once, for this describe
    defer(() => pyro.releaseProfile('positions'));       // after its last test
  });
  test('closes a position', async () => {
    await using page = await (await browser()).page({ as: 'positions' });
    await page.run([button('Settings').click(), check(text('Closed')).visible()]);
  }, 30_000);
});
```

```sh
pyro e2e apps/fe/e2e --setup e2e/setup.ts --concurrency 16 --timeout 30000 \
  --shard 1/4 --timings e2e/timings.json --junit junit.xml
```

- `describe`, `test` (`.serial`, `.skip`, `.todo`, `.only`, `.if`, `.skipIf`),
  `beforeAll`, `afterAll`, `beforeEach`, `afterEach` and `setDefaultTimeout`
  take `bun:test`'s arguments; `expect` is Bun's. A describe's `beforeAll`
  runs before its first test and `afterAll` after its last; a `test.serial`
  runs alone in its describe, after the tests declared before it and before
  those after it (`describe.serial`: all of them).
- `browser()` is the run's `pyro serve --fork`, launched on first
  use and closed when the run ends, also on SIGINT/SIGTERM. Pages a test
  opens through it are closed when the test ends, a timed-out test included.
- `defer(fn)` runs `fn` when what runs now is done: after the test, or after
  the describe (or file) whose `beforeAll` called it.
- `--setup <module>`: its `setup()` runs before any spec loads (start servers,
  seed data); specs read the result with `setupResult()`; `browser(result)`
  returns the browser's options (`args`, `bin`); `teardown(result)` runs at
  the end, also when the run is interrupted. A default export with those
  names works too (`satisfies Setup<T>`).
- `-j`/`--concurrency` caps the tests running at once across all files,
  `--max-concurrency` those of one file (as in `bun test`), so
  `-j 8 --max-concurrency 1` runs like `bun test --parallel=8
  --max-concurrency=1`, with one browser instead of eight.
- Paths are files, directories (`*.test.*`, `*.spec.*`), globs or path
  filters. `-t` filters by name, `--only` runs `.only` tests, `--bail [n]`
  stops after n failures, `-u` writes screenshot baselines.
- `--shard i/n` splits the files by their durations in `--timings`
  (`{"version": 1, "files": {"<path>": ms}}`), longest first onto the least
  loaded shard; `--update-timings` records this run's. `--junit <file>` (or
  `--reporter junit --reporter-outfile <file>`) writes a JUnit report.

## Playwright tests, unchanged

Playwright tests run on Pyro by replacing the imports of
`playwright` and `@playwright/test` with `@povio/pyro/wright`.
Nothing else in a test changes, and Playwright itself is not needed:

```ts
import { test, expect, type Page } from '@povio/pyro/wright'; // was '@playwright/test'

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
npx pyro run                      # as `npx playwright test`
npx pyro run -c playwright.e2e.config.ts positions-close -j 4
```

`pyro run` takes Playwright's usual flags (`-c`, `--project`,
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
  project's), `projects` (`$PYRO_PROJECT`, else the first), `timeout`,
  `retries`, `expect.timeout`, `outputDir`, `webServer` (started, waited for
  and stopped; `reuseExistingServer`), `globalSetup` and `globalTeardown`. The
  file is `$PYRO_CONFIG`, else `pyro.config.*` or `playwright.config.*`
  in the working directory.

The browser is `$PYRO_WS`, else one launched per run
(`$PYRO_BIN` or the bundled one), configurable with `use: {
pyro: { wsEndpoint, bin, args, boot } }`. Each awaited call is one
`Pyro.run` command, so locating, the actionability checks, auto-waiting and
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
- **Reporting:** passes, failures (with their errors and the files attached
  to them, such as videos), skips (declared or `test.skip()` at run time),
  flaky tests (passed on a retry) and expected failures, then a summary. The
  tests' own output is shown per worker.

Plain `bun test ./e2e/foo.e2e.ts` (with `$PYRO_CONFIG` naming the config) also
runs the tests, in one process: Bun then reports a run-time `test.skip()` as
a pass, since it cannot mark a running test as skipped. Not there either way:
Playwright's HTML report, trace viewer and sharding.

The TOS close flow (`test/wright/tos-flow.ts`, Playwright code as TOS
writes it) takes 14.7 ms per test this way (median of 40, fresh page),
against 41–44 ms for stock Playwright driving the same browser over CDP.
Actions settle before they return (the next call starts once the page is
quiet), so a test that asserts a transient state between an action and its
result (a spinner) can miss it.

**Pages booted ahead.** A page gets its target on its first command. With

```ts
// playwright.config.ts
use: { pyro: { boot: [{ url: '/platform', routes: '/platform/**' }] } }
```

a context's first page whose first call is `goto` to a `routes` match (or to
`url` itself) is taken from a pool of pages that already loaded `url` with
the context's init scripts, `storageState` local storage and viewport, and
the `goto` becomes a same-document route change (`pushState` + `popstate`,
settled), returning `null`. The app's shell was booted before the test
seeded anything, so use it for routes that fetch their data when they
mount. Contexts with cookies, `extraHTTPHeaders`, `userAgent`, media
emulation or any option of the next section that changes how a page loads
(proxy, locale, time zone, geolocation, permissions, credentials, device,
offline, JavaScript off, HTTPS errors), and any other first call, get a fresh
page.

**Context options.** `browser.newContext()` and `use: {}` take Playwright's
options with Playwright's meaning:

- `viewport` and `screen`; `deviceScaleFactor` (`devicePixelRatio`, and
  screenshots in device pixels, Playwright's default `scale: 'device'`);
  `isMobile` (the device's `mobile` flag and portrait or landscape screen
  orientation); `hasTouch` (`navigator.maxTouchPoints`, a coarse pointer
  without hover)
- `locale`: `navigator.language(s)`, `Accept-Language` and the default locale
  of `Intl` and the `toLocale*String` methods (JavaScriptCore's own default is
  the process's: a script that runs first in every document makes `locale`
  the default; the constructors stay the built-in ones to `instanceof` and
  subclasses)
- `timezoneId`: `Date`, `Intl` and `Temporal`. A time zone belongs to a
  connection's JavaScript VM, so the contexts of each zone get a connection
  of their own (opened in a few milliseconds, closed with its last context)
  and contexts in different zones run side by side; contexts without one
  keep the host's zone
- `geolocation` (`context.setGeolocation()`) and `permissions`
  (`context.grantPermissions(permissions, { origin })`, `clearPermissions()`),
  with Playwright's permission names; without the `geolocation` permission
  `navigator.geolocation` reports permission denied
- `proxy`: an HTTP proxy per context (`server` as `host:port` or
  `http://host:port`, `bypass`), loopback addresses included as with
  Playwright; `username` and `password` go to the proxy, and only to it, from
  the first request
- `httpCredentials`: a page's 401 challenge from `origin` (from any origin
  without one) gets the Basic credentials once; wrong ones end in the 401
  response; other origins and proxies get nothing. `context.request` and the
  `request` fixture send them after a 401 too, or with every request with
  `send: 'always'` (as in Playwright, `send` applies to API requests only)
- `offline` (`context.setOffline()`), `javaScriptEnabled: false` (no script of
  the page's own runs, event handlers and timers included, and `<noscript>`
  shows; `page.evaluate` still runs), `ignoreHTTPSErrors`, `colorScheme`,
  `reducedMotion`, `userAgent`, `extraHTTPHeaders`, `storageState`, `baseURL`
  and `recordHar`
- `recordVideo` (`dir`, `size`): a WebM of each page, popups included, as
  Playwright records it: VP8 at 25 frames a second, `size` or else the
  viewport scaled down to fit 800x800, frames padded with gray to the size;
  `page.video()` gives `path()`, `saveAs()` and `delete()`. Pyro sends a
  frame (CDP `Page.startScreencast`) whenever what the page shows changes, at
  most 25 a second, rendered as screenshots are (so CSS animations show at
  their end); a page that does not change costs nothing. The frames wait in a
  temporary directory and are encoded when the page closes, and the page's
  state at that moment gets a frame of its own: `page.close()` and
  `context.close()` return once the file is written (a few tenths of a
  second per 10 s of video). The encoder is ffmpeg: `$PYRO_FFMPEG`, else
  `ffmpeg` on PATH, else Playwright's (`bunx playwright install ffmpeg`);
  without one, `newContext({ recordVideo })` fails and says how to get one.
  The runner's `video` option (`'on'`, `'retain-on-failure'`,
  `'on-first-retry'`, or `{ mode, size }`) records the `context` fixture's
  pages into the test's output directory (`video.webm`, `video-1.webm`, ...),
  attached to the test and listed under a failure by `pyro run`; under
  `retain-on-failure` the pages a passing test leaves open are not encoded
  at all

Also covered:

- element and JS handles (`page.$`, `$$`, `evaluateHandle`,
  `locator.elementHandle()`, handles as `evaluate` arguments), in the main
  frame and in child frames; element handle actions run natively on the
  element, which carries a `data-pyro-handle` attribute for the length of the
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
  locators, handles; their `<iframe>` gets a `data-pyro-frame` attribute once
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
- `page.goto()` and `page.reload()` return the document's response: status,
  status text, headers, the request's headers, the redirects on the way
  (`request().redirectedFrom()`) and `body()`/`text()`/`json()`. Without a
  `request`/`response` listener on the page the headers come from the
  navigation's own record, which leaves out the cookie and credential
  headers (`set-cookie`, `authorization`, ...); with one they are all there
- files written where a `path` option says (`screenshot`, `pdf`,
  `storageState`, HAR recordings, `download.saveAs`) get missing directories
  made, as in Playwright

Types are Playwright's own shapes, checked by `bun run test:types`:
`expect()` offers Locator, Page or APIResponse matchers by what it is given
(`toPass` only on functions), `expect.extend` matchers are typed from their
arguments, `getByRole` takes the ARIA role union, and `evaluate`/`$eval`
infer argument and return types. Deprecated APIs (`locator.type`,
`elementHandle.type`, `noWaitAfter`, `page.accessibility`, FrameLocator
`first`/`last`/`nth`) are left out, so using one is a type error.

- `page.clock` / `context.clock`: `install`, `pauseAt`, `resume`,
  `fastForward`, `runFor`, `setFixedTime`, `setSystemTime`. Date, timers,
  `requestAnimationFrame`, `requestIdleCallback` and `performance.now` are
  faked in every document, new ones included; a paused clock does not hold
  up an action's settling
- `locator.ariaSnapshot()`, `page.ariaSnapshot()` and
  `expect(locator).toMatchAriaSnapshot()`, with a template or a `.aria.yml`
  file: partial matching, regexes, `/children` and the state attributes. The
  tree is built in the browser from the same role and name code `getByRole`
  uses, CSS `::before` / `::after` text included (strings, `attr()` and alt
  text after `/`; counters, quotes and images add nothing, as in
  Playwright). `mode: 'ai'` as Playwright's: `[ref=e5]`, `[active]`,
  `[cursor=pointer]`, generic containers and the snapshots of iframes
  (refs `f1e5` inside them); `locator('aria-ref=e5')` finds the element of
  a ref from the last AI snapshot of its frame
- `expect(value).toMatchSnapshot()` for strings and Buffers, at Playwright's
  paths (`snapshotPathTemplate`, `testInfo.snapshotPath(name, { kind })`).
  PNGs compare by pixel (`threshold`, `maxDiffPixels`,
  `maxDiffPixelRatio`), the rest by bytes. A missing snapshot is written and
  fails the test; `pyro run -u [all|changed|missing|none]` rewrites
- `toHaveScreenshot(name)`: baselines at Playwright's snapshot path
  (`<test file>-snapshots/<name>-<platform>.png`, or `snapshotPathTemplate`),
  rendered by pyro; a mismatch writes `-actual`, `-expected` and
  `-diff` images into the test's output directory. With both
  `maxDiffPixels` and `maxDiffPixelRatio`, the stricter limit applies
- `page.coverage`: JS block coverage in V8's format (`startJSCoverage`
  options `resetOnNavigation`, `reportAnonymousScripts`) and CSS rule usage.
  Started after the page ran, it reports what Chrome does: counts start at
  zero, and a function is listed only when it ran since (or the listed
  function around it did), so a script's top level and functions that do
  not run again are left out. Start it before `goto` for whole scripts.
  Anonymous functions carry V8's inferred names (`ns.a = function () {}`
  is `ns.a`), from a parse of the script that follows V8's own rules.
  One gap: an async function or generator that was suspended when coverage
  started and resumes after it is not reported (it resumes in code compiled
  without the profiler)

Not supported (a clear error): the deprecated layout pseudo-classes
(`:left-of()`, `:right-of()`, `:above()`, `:below()`, `:near()`); a `+` or `~`
combinator right after a Playwright pseudo-class; SOCKS and HTTPS proxies
(`proxy.server`); `timezoneId` on the Intel macOS browser (system
JavaScriptCore).

Not supported, accepted so configs and `devices` descriptors keep working:

- context options with no effect: `bypassCSP`, `serviceWorkers`,
  `forcedColors`, `strictSelectors`, and `acceptDownloads`
  (downloads are always accepted)
- `recordVideo.showActions` and the runner's `video.show` draw no annotations
- `isMobile` does not apply `<meta name="viewport">`: pages lay out at the
  viewport's width, as with `width=device-width`
- `toHaveScreenshot` renders at the context's `deviceScaleFactor` (Playwright's
  default there is `scale: 'css'`), and `screenshot()` has no `scale` option
- `context.request` and the `request` fixture do not go through `proxy`
- `locale` reaches `Intl` in documents only, not in workers

## Install

```sh
npm install --save-dev @povio/pyro
# Or: bun add --dev @povio/pyro
npx pyro serve --port 9222
```

For release candidates, install `@povio/pyro@next`.

The package bundles browsers for **macOS 14+ on Apple Silicon and Intel** and
**Linux x64 and arm64 (glibc 2.39+, e.g. Ubuntu 24.04)**, with
layout, screenshots and PDF rendering enabled. No Rust toolchain, install
scripts or binary downloads are needed. The API supports Node.js 22+ and Bun;
Bun loads TypeScript sources, while Node loads compiled JavaScript and types.
On other operating systems, use `Pyro.connect()` with a remote
browser. Installing/importing the client does not launch a local browser.

`Pyro.launch()` selects `bin`, then `$PYRO_BIN`, then the
browser beside the package in a release archive, then the bundled browser for
the current architecture. The CLI also respects `$PYRO_BIN`.

On Apple silicon the bundled browser is the **fork-capable build** with its
patched JavaScriptCore included, so `args: ['--fork']` works without a separate
browser. The Intel browser uses macOS's system JavaScriptCore and has no fork
mode.

`args` takes any `pyro serve` flag (`pyro serve --help`
lists them). For screenshots and PDFs that should wrap like Chrome's, pass
`args: ['--hyphen-digit-breaks', 'on']` (or set
`PYRO_HYPHEN_DIGIT_BREAKS=on`): text may then break between a hyphen
and a digit when the hyphen follows a letter or digit (`REG-be08b23e-89be-48b8-`
/ `9a6f-…`, `10-` / `20`), while a minus sign stays with its number (`-120,00`,
`A -5`). Table columns and other min-content widths follow the same breaks. It
is off by default: Unicode's line breaking rules (UAX #14 LB25) have no break
there.

The package is licensed under AGPL-3.0-only; see the included `LICENSE` and
`NOTICE`. Maintainers: see
[publishing instructions](https://github.com/povio/pyro/blob/main/docs/NPM_PUBLISHING.md).

Coding agents: read [LLM.md](LLM.md) before writing tests.

`examples/positions-close.test.ts` is TOS's `positions-close.e2e.ts` ported.
`bench/scripts/cdp-perf.mjs --filter '^(pw\.)?tos\.'` runs the same flow
against a TOS-shaped app in the bench site, as Playwright, step by step, as one
run, and as one run on a pre-booted page.
