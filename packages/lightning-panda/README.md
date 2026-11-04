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
bundled browser for the current architecture, then a sibling browser in a
legacy release archive. The CLI also respects `$LIGHTNINGPANDA_BIN`.

The bundled browsers use macOS's system JavaScriptCore. **Fork mode requires
a separately built patched JavaScriptCore browser**; select that browser with
`bin` or `$LIGHTNINGPANDA_BIN` when using `args: ['--fork']`.

The package is licensed under AGPL-3.0-only; see the included `LICENSE` and
`NOTICE`. Maintainers: see
[publishing instructions](https://github.com/povio/lightningpanda/blob/main/docs/NPM_PUBLISHING.md).

Coding agents: read [LLM.md](LLM.md) before writing tests.

`examples/positions-close.test.ts` is TOS's `positions-close.e2e.ts` ported.
`bench/scripts/cdp-perf.mjs --filter '^(pw\.)?tos\.'` runs the same flow
against a TOS-shaped app in the bench site, as Playwright, step by step, as one
run, and as one run on a pre-booted page.
