# lightningpanda-test

E2E tests for `bun test` on Lightningpanda. A test sends its whole scenario as
one `LP.run` command: the browser runs every step itself (locating, acting,
waiting for the app to settle, checking) and replies once.

```ts
import { Lightningpanda, button, check, dialog, text } from 'lightningpanda-test';

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

Each release ships the client with the browser:

- **Release archive.** `lightningpanda-test/` sits beside the `lightningpanda`
  binary, and `Lightningpanda.launch()` starts that binary when nothing else is
  given (`bin`, then `$LIGHTNINGPANDA_BIN`, then the archive's binary, then
  `lightningpanda` on PATH).
- **Release asset.** `lightningpanda-test-<version>.tgz` installs into a test
  project: `bun add ./lightningpanda-test-0.0.4.tgz` (or its download URL).
  Bun loads the TypeScript sources; Node gets `dist/index.js` and types.

`scripts/pack-test-client.sh OUT_DIR [ARCHIVE_DIR]` builds both, versioned
from `Cargo.toml`.

Coding agents: read [LLM.md](LLM.md) before writing tests.

`examples/positions-close.test.ts` is TOS's `positions-close.e2e.ts` ported.
`bench/scripts/cdp-perf.mjs --filter '^(pw\.)?tos\.'` runs the same flow
against a TOS-shaped app in the bench site, as Playwright, step by step, as one
run, and as one run on a pre-booted page.
