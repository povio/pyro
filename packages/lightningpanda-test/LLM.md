# Writing E2E tests with lightningpanda-test: instructions for coding agents

Read this before writing or porting a test. It is the whole contract; the
source is `src/steps.ts` (builders) and `src/index.ts` (pages and runs).

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

The client comes with each Lightningpanda release: install
`lightningpanda-test-<version>.tgz` from the release with `bun add`, or import
`lightningpanda-test/` from the unpacked archive. `Lightningpanda.launch()`
finds the archive's binary on its own; set `LIGHTNINGPANDA_BIN` otherwise.

```ts
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { Lightningpanda, button, check, dialog, text, label } from 'lightningpanda-test';

let lp: Lightningpanda;
beforeAll(async () => {
  lp = await Lightningpanda.connect(process.env.LIGHTNINGPANDA_WS!, { connections: 4 });
  await lp.profile('employee', {
    url: `${APP}/platform`,                 // any page of the app: it boots signed in and waits
    localStorage: authEntries,              // what the app reads at boot (tokens, user store)
    viewport: { width: 1440, height: 900 },
    pool: 2,
  });
});
afterAll(() => lp.close());

test.concurrent('closes a position', async () => {
  const position = await api.createPosition();                    // seed through the API, not the UI
  await using page = await lp.page({ as: 'employee', route: `/positions/${position.id}` });
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

- `lp.page({ as, route })`: a pre-booted, signed-in page; `route` changes the
  URL in place (pushState + popstate), so the app does not boot again. Use
  `url` instead only when a full page load is the point of the test.
- `await using page` closes the page when the test ends (do not leave pages
  open: an open page stops its connection from refilling).
- One profile per kind of user (`as`). Several profiles are fine.

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
| `evaluate('expression', 'name')` | escape hatch: JSON value of a page expression into `r.values.name`; a returned promise is awaited (within the step timeout) and a rejection fails the step |

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
  the run. There is no clock control: `delay` is real time.

## Porting from Playwright

| Playwright | lightningpanda-test |
|---|---|
| `page.getByRole('button', { name: 'X' })` | `button('X')` |
| `locator.filter({ visible: true }).first()` | `loc.visible().first()` |
| `page.getByRole('dialog').filter({ hasText }).last().getByRole('button', …)` | `dialog().hasText(…).last().button(…)` |
| `await expect(loc).toBeVisible()` / `toHaveText` / `toHaveCount` | `check(loc).visible()` / `.text()` / `.count()` |
| `await expect.poll(() => loc.count()).toBe(1)` | `check(loc).count(1)` |
| `Promise.all([page.waitForResponse(…), loc.click()])` | `loc.click()` then `check.request(method, path).status(200)` |
| `browser.newContext({ storageState }) + newPage + goto` | `lp.profile(...)` once, then `lp.page({ as, route })` |
| `page.evaluate(fn)` | `evaluate('expression', 'name')` (an expression string, no closures; promises are awaited) |
| `page.addInitScript(src)` | `lp.page({ as, initScripts: [src] })`, or `await page.addInitScript(src)` before a `goto` / `reload` |
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
