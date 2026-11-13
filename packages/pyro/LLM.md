# Writing E2E tests with @povio/pyro: instructions for coding agents

Read this before writing or porting a test. It is the whole contract; the
source is `src/steps.ts` (builders) and `src/index.ts` (pages and runs).

Porting an existing Playwright suite? Do not rewrite it: replace the imports
of `playwright` and `@playwright/test` with `@povio/pyro/wright`,
keep the Playwright code and run it with `pyro run` (README, "Playwright
tests, unchanged"). Context options (`locale`, `timezoneId`, `proxy`,
`geolocation` and `permissions`, `httpCredentials`, `offline`, device
emulation, `javaScriptEnabled`, `ignoreHTTPSErrors`) mean what they mean in
Playwright; the README's "Context options" and "Not supported" list the
differences. Rewrite a test into one `page.run([...])` only where its last
milliseconds matter.

## The model

- A test is **seeding, then one `page.run([...steps])`**. The browser runs every
  step itself and replies once. Do not split a scenario into several `run`
  calls unless plain code (an API call, a computation) must happen between
  them.
- **Nothing is polled from the test.** Every step waits in the browser until it
  holds (default 5 s per step). Never write `sleep`, retry loops,
  `expect.poll`, `waitForTimeout` or `waitForResponse`.
- **Actions settle.** After `click`, `fill`, `press`, `check`, `uncheck` and
  `route`, the next step starts only when the page is quiet: the requests the
  action started have finished, no task is queued, no timeout is due within
  50 ms, no animation frame is pending and no navigation is half done. Intervals,
  later timeouts and animation loops do not count. Assert the outcome with the
  next `check`; do not wait for the request.
- **Requests are recorded, not awaited.** `check.request(method, path)` looks
  at the requests made during *this* run (it waits for one to appear and
  finish). Put it in the same `run` as the action that sends the request.

## Skeleton

Install the browser and client with `bun add --dev @povio/pyro`
(or `@povio/pyro@next` for an RC). `Pyro.launch()` finds
the bundled macOS browser automatically. On Apple silicon it is the
fork-capable build (patched JavaScriptCore bundled), so `--fork` works; the
Intel browser has no fork mode.
On other operating systems, connect to a remote browser with
`Pyro.connect()`.

```ts
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { Pyro, button, check, dialog, text, label } from '@povio/pyro';

let pyro: Pyro;
beforeAll(async () => {
  pyro = await Pyro.connect(process.env.PYRO_WS!, { connections: 4 });
  await pyro.profile('employee', {
    url: `${APP}/platform`,                 // any page of the app: it boots signed in and waits
    localStorage: authEntries,              // what the app reads at boot (tokens, user store)
    viewport: { width: 1440, height: 900 },
    pool: 2,
  });
});
afterAll(() => pyro.close());

test.concurrent('closes a position', async () => {
  const position = await api.createPosition();                    // seed through the API, not the UI
  await using page = await pyro.page({ as: 'employee', route: `/positions/${position.id}` });
  const r = await page.run([
    check(text(position.number)).visible(),
    button('Settings', { exact: true }).click(),
    dialog().hasText('will be closed').button('Close Position', { exact: true }).click(),
    check.request('POST', `/positions/${position.id}/close`).status(200),
    check(button('Reopen Position')).count(1),
    text(/^Closed by: /).read('closedBy'),
  ]);
  expect(r.values.closedBy).toContain('E2E');                     // bun:test expect on values read
});
```

- `pyro.page({ as, route })`: a pre-booted, signed-in page; `route` changes the
  URL in place (pushState + popstate), so the app does not boot again. Use
  `url` instead only when a full page load is the point of the test.
- `await using page` closes the page when the test ends (do not leave pages
  open: an open page stops its connection from refilling).
- One profile per kind of user (`as`). Several profiles are fine.

### A whole suite in one process: `pyro e2e`

Prefer this over `bun test --parallel` for large suites: specs import
`describe`/`test`/hooks/`expect` from `@povio/pyro/e2e` instead of
`bun:test`, and the run uses one browser and one case queue across files.

```ts
import { beforeAll, browser, defer, describe, test } from '@povio/pyro/e2e';
describe('feature', () => {
  beforeAll(async () => {
    const pyro = await browser();
    await pyro.profile('feature', { base: 'app', url, setup });
    defer(() => pyro.releaseProfile('feature')); // retire it when the describe is done
  });
  test('does it', async () => {
    await using page = await (await browser()).page({ as: 'feature' });
    await page.run(steps);
  });
});
```

- Run: `pyro e2e <dirs|files|globs> --setup setup.ts -j 16 --timeout 30000`.
- `setup.ts` exports `setup()` (servers, seed; result via `setupResult()`),
  `browser(result)` (`{ args: ['--static', ...] }`) and `teardown(result)`.
- Register a profile shared by many files once (memoize the promise), and
  per-describe profiles in that describe's `beforeAll` with `defer` to release.
- Module state is shared by every file (one process): no per-file globals.

### One fork-mode server for every worker

With `pyro serve --fork`, start one server for the whole run and
let every `bun test` worker connect to it: profiles boot once per server,
not once per worker.

```ts
// launcher (the process that runs `bun test`)
const pyro = await Pyro.launch({ args: ['--fork', '--cdp-max-connections', '64'] });
await pyro.profile('app', { url: `${APP}/`, setup: signIn });   // boots once
await pyro.warm('app');
const tests = Bun.spawn(['bun', 'test', '--parallel'], { env: { ...process.env, PYRO_WS: pyro.wsEndpoint } });
await tests.exited;
pyro.close();

// each worker
const pyro = await Pyro.connect(process.env.PYRO_WS!);
await pyro.profile(`feature-${process.pid}-${file}`, { base: 'app', url, setup });  // 'app' is the launcher's
await using page = await pyro.page({ as: 'app' });                                   // adopts it too
```

- Names are shared: the same definition under the same name adopts the
  booted profile; a different one is refused while another client holds the
  name (give per-worker profiles unique names).
- `pyro.adopt(name)` holds a profile explicitly; `pyro.page({ as })` and
  `base: name` adopt a name they do not know by themselves.
- A client holds what it registered or adopted until `releaseProfile(name)`
  or until it disconnects; a profile is retired once nobody holds it and no
  profile derives from it.
- `Pyro.launch()` passes its pid to the server, which exits when
  that process does (even when a runner stops it without exit handlers).

## Locators

Build them with the exported helpers; they are data, so build them once and
reuse them.

| Want | Write |
|---|---|
| Button / link / heading / dialog … by accessible name | `button('Save')`, `link('Docs')`, `heading('Positions')`, `dialog()`, `role('tab', 'Air')` |
| Form control by its label | `label('Email')` |
| By placeholder | `placeholder('Search')` |
| By visible text | `text('Saved')`, `text(/^Updated by .*/)` |
| By test id | `testId('row-42')` (`testId(id, 'data-test')` for another attribute) |
| CSS as a last resort | `css('#out')` |
| Inside another locator | `dialog().button('OK')`, `row('EXP-1001').button('Edit')`, `css('.card').text('Total')` |
| Inside an iframe | `css('#editor').frame().button('Bold')`, `inFrame(css('#editor')).text('Hi')` |
| Narrow the matches | `.visible()`, `.hasText('beta')`, `.hasNotText('alpha')`, `.has(button('Edit'))`, `.hasNot(…)` |
| Pick one of several | `.first()`, `.last()`, `.nth(i)` (after the filters) |

- Strings match **case-insensitive substrings** after whitespace
  normalization; pass `{ exact: true }` for a whole, case-sensitive match. A
  `RegExp` (flags `i`, `m`, `s` only) tests the normalized text.
- **Strict mode:** an action on a locator that matches several elements fails
  at once and lists them. Fix the locator (scope it, filter it, `exact`), and
  use `.first()` only when any match will do.
- `text()` follows Playwright's `getByText`: the scope element itself can match
  (`css('td').text('Domestic', { exact: true })` finds the cell), and an exact
  string also matches an element one of whose own text runs equals it
  (`<span><span>|</span>TS-1</span>` for `text('TS-1', { exact: true })`).
- Prefer role + name, then label / placeholder / text, then test id, then CSS.
- Hidden elements (`display: none`, `aria-hidden`) have no role: `button('X')`
  does not match a hidden button. Add `.visible()` to `text()` / `css()`
  locators when hidden copies exist.

## Steps

| Step | Notes |
|---|---|
| `loc.click()` | visible, enabled, not covered, scrolled into view, then a real mouse click; `{ button: 'right', position: { x, y }, force: true }` |
| `loc.dblclick()` / `loc.hover()` | same pointer options as `click` |
| `loc.type('text')` | focuses, then one key press per character (`pressSequentially`) |
| `loc.blur()` | moves focus away (`blur` / `focusout`) |
| `loc.upload([{ name, text \| base64, mimeType? }])` | sets the files of an `<input type=file>`; a string is a path on the browser's machine |
| `loc.dropFiles([{ name, text \| base64, mimeType? }])` | external file drag into a visible target; real dragenter/dragover/drop and File data, preserving inline names |
| `loc.drag(target)` | native drag and drop onto `target`, else a pointer drag |
| `loc.wheel(deltaX, deltaY)` / `mouse('click', { x, y })` | mouse wheel over the element / raw mouse at viewport coordinates (`click`, `dblclick`, `move`, `down`, `up`, `wheel`) |
| `loc.fill('text')` | replaces the value of an input, textarea or contenteditable |
| `loc.press('Enter')` | focuses, then a key or chord: `Escape`, `Tab`, `ArrowDown`, `Shift+Tab`, `ControlOrMeta+A`, `a` |
| `loc.check()` / `loc.uncheck()` | checkbox or radio; no-op when already in that state |
| `loc.focus()` | |
| `loc.read('name', 'text' \| 'value' \| 'count' \| 'visible' \| 'box' \| 'name' \| 'focused' \| 'enabled')` | value lands in `r.values.name`; `loc.read('c', 'css', 'color')` for a computed style |
| `check(loc).visible()` / `.hidden()` | |
| `check(loc).text('…')` / `.containsText('…')` / `.value('…')` / `.count(n)` / `.checked()` | `check(loc).not.visible()` negates |
| `check(loc).css('color', 'rgb(0, 0, 0)')` / `.focused()` / `.name('…')` / `.enabled()` / `.disabled()` | computed style, focus, accessible name, disabled state |
| `check.request('POST', '/exact/path').status(200)` | path is exact (no query); a `RegExp` tests the full URL; `.status([200, 304])` for any of several; `.count(n)` for exactly n; `.body(…)`, `.bodyKeys([…])`, `.headers({ … })`; `.state('started')` also counts requests in flight |
| `goto(url)` / `route(path)` | full navigation / same-document route change; `route(path, { settle: false })` does not wait for its requests |
| `reload()` / `waitForUrl('/positions/')` | reload and wait for `load` / wait until the URL contains (or matches) it |
| `loaded()` | wait for the page's document to load (added for you on a page still booting) |
| `idle({ timeout })` | wait until the page is quiet, as after an action; never fails |
| `mock('/api/items', { json: [] })` / `unmock(url?)` | answer matching requests (`status`, `body`, `headers`, `delay`, `abort`, `times`); `{ passthrough: true }` lets them through, `{ patch: { … } }` rewrites the real response, `{ hold: 'name' }` holds them until `release('name')` |
| `download({ as, saveTo })` / `popup('name')` | wait for the next download / popup of the run |
| `viewport(w, h)` / `media({ colorScheme: 'dark' })` | resize the viewport / emulate media features mid-test |
| `matchScreenshot('name', { … })` / `loc.matchScreenshot('name')` | renders the page (or `clip`, `fullPage`, the element) in the browser and compares it with `__screenshots__/name.png` beside the test; see Screenshots below |
| `evaluate('expression', 'name')` | escape hatch: JSON value of a page expression into `r.values.name`; a returned promise is awaited (within the step timeout) and a rejection fails the step |

Pointer options (wire shape; the builders take the same names): `click` and
`dblclick` accept `button: 'right' | 'middle'` (a right click fires
`contextmenu` after the press and `auxclick` after the release, never
`click`, as Chromium does); `click`, `dblclick`, `hover`
and `wheel` accept `position: { x, y }` (an offset from the element's
top-left instead of its center) and `force: true` (skip the covered and
enabled checks). `hover` never needs an enabled element: hovering a disabled
input or an `aria-disabled` menu item works. An element wider than its
scroll container is clicked at the center of its visible part, as Playwright
does.

| Step (wire) | Notes |
|---|---|
| `{ act: { locator, action: 'wheel', deltaX, deltaY } }` | hovers the element, fires `wheel` there and scrolls the nearest scrollable container unless canceled |
| `{ mouse: { action: 'click' \| 'dblclick' \| 'move', x, y, button?, steps? } }` | `page.mouse.*` at viewport coordinates; `move` with `steps` sends intermediate moves |
| `{ mouse: { action: 'down' \| 'up', button?, x?, y? } }` | at the current pointer position unless `x`/`y` are given; buttons stay held across `move` steps until `up` |
| `{ mouse: { action: 'wheel', deltaX, deltaY, x?, y? } }` | wheel at the current pointer position (or `x`/`y`) |
| `{ viewport: { width, height } }` | same as `viewport(w, h)`: resizes mid-run, fires `resize` and `matchMedia` `change`, settles like an action |
| `{ media: { colorScheme?, reducedMotion?, media? } }` | same as `media({ … })`: replaces the whole emulation (an omitted field resets: `light`, `no-preference`, `screen`); `media: 'print'` makes `print` queries match; `matchMedia(…)` `change` listeners fire |
| `{ page: { target: 'tab' \| targetId \| 'main' } }` | the following steps drive another page of the context: the `as` name of an earlier `popup('tab')`, a target id, or `main` (the page the run started on); add `waitForUrl` / `loaded()` after switching to a fresh popup. Builder: `switchTo('tab')`, `switchTo('main')` |

Every action and check takes `{ timeout }` (ms); `page.run(steps, { timeout })`
sets the default for the run. Pass `{ settle: false }` to an action whose
requests never finish (polling, streaming) or that you do not want to wait for.

### Requests and mocks in detail

- A `RegExp` anywhere (locators, `mock`, `waitForUrl`, `check.request`) runs
  with JavaScript semantics: look-ahead `(?=…)`, negative look-ahead `(?!…)`,
  look-behind, backreferences and named groups work; flags `i`, `m`, `s`.
- `check.request(…)` and the reply's `requests` carry `requestHeaders`,
  `postData` (text; base64 with `postDataBase64: true` when not UTF-8; cut at
  1 MiB with `postDataTruncated: true`) and `responseHeaders`.
  `.bodyKeys(['a', 'b'])` wants exactly these top-level JSON keys;
  `.headers({ 'X-Key': /^k\d$/ })` matches names case-insensitively.
- `.state('started')` counts a request as soon as it is sent, so
  `.count(1).state('started')` holds while a mock still delays or holds it.
- `mock(url, { patch: { status, headers, json, body } })` sends the request to
  the real backend and rewrites its response; `json` is a JSON merge patch
  (`null` deletes a key) onto a JSON object body, else the new body.
- `mock(url, { hold: 'save', json: … })` keeps the request unsent until
  `release('save')`, then answers it (or sends it on, without a response
  option); with `passthrough: true` (or `patch`) the request reaches the
  backend and its response is held instead. `release` waits until something
  is held, so it can come before the action's request starts; holds outlive
  the run. The native `advanceTime` step explicitly advances page Date and timers;
  mock `delay` remains a network delay.

## Porting from Playwright

| Playwright | @povio/pyro |
|---|---|
| `page.getByRole('button', { name: 'X' })` | `button('X')` |
| `locator.filter({ visible: true }).first()` | `loc.visible().first()` |
| `page.getByRole('dialog').filter({ hasText }).last().getByRole('button', …)` | `dialog().hasText(…).last().button(…)` |
| `await expect(loc).toBeVisible()` / `toHaveText` / `toHaveCount` | `check(loc).visible()` / `.text()` / `.count()` |
| `await expect.poll(() => loc.count()).toBe(1)` | `check(loc).count(1)` |
| `Promise.all([page.waitForResponse(…), loc.click()])` | `loc.click()` then `check.request(method, path).status(200)` |
| `browser.newContext({ storageState }) + newPage + goto` | `pyro.profile(...)` once, then `pyro.page({ as, route })` |
| `page.evaluate(fn)` | `evaluate('expression', 'name')` (an expression string, no closures; promises are awaited) |
| `page.addInitScript(src)` | `pyro.page({ as, initScripts: [src] })`, or `await page.addInitScript(src)` before a `goto` / `reload` |
| `page.waitForURL(url)` | `waitForUrl(url)` |
| `page.route(url, r => r.fulfill(…))` | `mock(url, { … })` as a step |
| `frameLocator('#editor').getByRole(…)` | `css('#editor').frame().role(…)` |
| helpers that retry clicks or re-open dropdowns | delete them: actions wait for actionability and settle |

## When a run fails

`page.run` throws `RunError` with the step index and why that step was still
waiting when it timed out, for example:

- `matched no element`: wrong locator, or the UI never got there; check the
  step before it.
- `strict mode violation: … matched 3 elements: <button> "Edit", …`: scope or
  filter the locator.
- `<div> intercepts pointer events`: an overlay covers the element; close it
  first or click what the user would click.
- `element is disabled` / `not editable` / `not visible`.
- `no matching request yet` / `last match was {…status: 500…}`: the request
  check; the reply's `requests` lists everything the page requested.

Do not raise timeouts to fix a failure: a step waits only for its own
condition, so a timeout means the condition never held.

## Do not

- Do not add waits, sleeps or retry loops; do not wrap steps in try/catch.
- Do not drive login or seeding through the UI; use the profile and the API.
- Do not send one `run` per step "to be safe": it costs a round trip per step
  and request checks only see their own run.
- Do not share a page between tests; take a new one per test.

## Screenshots

Visual checks run in the browser, in the same `run` as the steps that lead
to the state (needs a browser built with the `render` feature):

```ts
await page.run([
  label('Email').fill('ada@example.com'),
  matchScreenshot('login-filled'),                     // viewport
  css('.card').matchScreenshot('login-card'),          // one element's box
  matchScreenshot('login-full', { fullPage: true }),
]);
await page.matchScreenshot('dashboard', { maxDiffPixels: 20 });  // one-step form
```

- Baselines are `__screenshots__/<name>.png` (+ `<name>.json` with the
  display-list hash) beside the test file; `dir` overrides. Commit them.
- First run (no baseline): the image is written and the test fails. Run once
  with `PYRO_UPDATE_SNAPSHOTS=1` (or `bun test -u`) to write or refresh all.
- A mismatch throws `ScreenshotMismatchError` (`.result.diffPixels`,
  `.result.files`) and writes `<name>.actual.png` and `<name>.diff.png`
  (differing pixels in red). Look at the diff before updating a baseline.
- Rendering is deterministic on the CPU: compare exactly by default. Use
  `threshold` (0-1, per channel) or `maxDiffPixels` only for known noise.
- What is captured: the live DOM after script, every stylesheet (including
  ones added or edited by script), input/textarea/select state, the window
  scroll position, viewport and device scale factor, images and fonts loaded
  through the page's network (mocks apply). Not captured: canvas and video
  pixels, iframe contents, element scroll offsets, `:hover`/`:focus` styles,
  the caret, running animations. Pixels come from pyro's renderer
  (Blitz), never compare them with Chromium screenshots.

`downloadCount(as)` reads the current page's native initiated-download count without consuming positive download waits. Compare snapshots before and after a settled preview action to assert no new downloads; the count persists across batches on the same page.
