# Porting from Playwright

You do not have to port anything to run a Playwright suite on Pyro: replace the `playwright` and `@playwright/test` imports with `@povio/lightning-panda/playwright` and run `lightningpanda test` (see [Migrating a suite](/playwright/migration)). Port a test to the native driver when its last milliseconds matter: the hot tests that run on every commit, the long tail of short flows, the ones that dominate a suite's wall time. This page maps Playwright code to native steps and shows complete before and after examples.

## Where porting pays off

On the TOS close flow (`bench/results/lp-tos-e501cef` and the package README):

| How the test runs | Per test |
|---|---:|
| Stock Playwright over CDP, same browser | 34 to 44 ms |
| Unchanged Playwright code on `@povio/lightning-panda/playwright` | 14.7 ms |
| Native, one `LP.run` on a page booted ahead | 6.8 ms |
| Native, booted pages over 4 connections | 2.7 ms |

The Playwright layer already runs each awaited call as one `LP.run` command, so locating, actionability and auto-waiting happen in the browser. Porting removes what is left: one round trip per call, the client-side code between calls, the page and app boot per test, and the request waits.

Keep the Playwright API for tests that use what the native driver does not have: HAR replay and recording, `page.clock` beyond advancing time, ARIA snapshots, element and JS handles, coverage, event listeners (`page.on('console')`, `page.on('dialog')`), and `test.step` reporting. Both styles run against the same browser and can live in one repository.

## Example 1: a form submit

**Before** (Playwright, run with `lightningpanda test`):

```ts
import { test, expect } from '@povio/lightning-panda/playwright';

test('adds a todo', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('token', 'test-token'));
  await page.goto('/');
  await page.getByLabel('New todo').fill('Buy milk');
  const created = page.waitForResponse((r) => r.url().endsWith('/api/items') && r.request().method() === 'POST');
  await page.getByRole('button', { name: 'Add' }).click();
  expect((await created).status()).toBe(201);
  await expect(page.getByText('Added Buy milk')).toBeVisible();
  expect(await page.getByText(/^Added /).textContent()).toBe('Added Buy milk');
});
```

**After** (native, run with `bun test`; the full file is on [Your first test](/e2e/first-test)):

```ts
beforeAll(async () => {
  lp = await Lightningpanda.launch();
  await lp.profile('user', { url: `${APP}/`, localStorage: { token: 'test-token' } });
});

test.concurrent('adds a todo', async () => {
  await using page = await lp.page({ as: 'user' });
  const r = await page.run([
    label('New todo').fill('Buy milk'),
    button('Add').click(),
    check.request('POST', '/api/items').status(201),
    check(text('Added Buy milk')).visible(),
    text(/^Added /).read('status'),
  ]);
  expect(r.values.status).toBe('Added Buy milk');
});
```

What changed:

- `addInitScript` plus `goto` per test became a profile: the page is booted and signed in before the test asks for it.
- `waitForResponse` before the click became a `check.request` after it. Requests are recorded; the check looks at the run's log.
- Six round trips became one.

Both versions were run against the same app.

## Example 2: TOS positions-close

**Before**, as TOS wrote it in Playwright (package README):

```ts
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

**After**, from `packages/lightning-panda/examples/positions-close.test.ts`:

```ts
test.concurrent('closes an open Position from the Position overview', async () => {
  const position = await createPosition();                       // seeded through the API
  await using page = await lp.page({ as: 'employee', route: overview(position.id) });

  await page.run([
    check(text(position.number)).visible(),
    button('Settings', { exact: true }).press('Enter'),
    button('Close Position').visible().first().click(),
    dialog().hasText('This position will be closed').last().button('Close Position', { exact: true }).click(),
    check.request('POST', `${positionsApi}/${position.id}/close`).status(200),
    check.request('GET', `${positionsApi}/${position.id}`).status(200),
    check(text(/^Position Closed · .* · Manual · Closed by:/).visible()).count(1),
    check(button('Reopen Position')).count(1),
  ]);
});
```

The profile in that file boots the app at `/platform` with the tokens and stores that TOS's own `createFreshAuthenticatedContext` writes, `pool: 4`, viewport 1440 × 900. `route` moves the booted app to the position in place.

## Mapping

### Locators

| Playwright | Native |
|---|---|
| `page.getByRole('button', { name: 'Save' })` | `button('Save')` (also `link`, `heading`, `dialog`, `textbox`, `checkbox`, `combobox`, `option`, `row`, `cell`, `tab`, `menuitem`, or `role('tablist')`) |
| `getByRole(..., { exact: true })` | `button('Save', { exact: true })` |
| `page.getByLabel('Email')` | `label('Email')` |
| `page.getByPlaceholder('Search')` | `placeholder('Search')` |
| `page.getByText('Saved')` | `text('Saved')` |
| `page.getByTestId('row-42')` | `testId('row-42')` |
| `page.locator('#out')` | `css('#out')` |
| `locator.getByRole(...)` | `loc.role(...)`, `loc.button(...)` |
| `locator.filter({ hasText })` / `{ hasNotText }` | `.hasText(...)` / `.hasNotText(...)` |
| `locator.filter({ has })` / `{ hasNot }` | `.has(loc)` / `.hasNot(loc)` |
| `locator.filter({ visible: true })` | `.visible()` |
| `.first()`, `.last()`, `.nth(i)` | the same (after filters) |
| `page.frameLocator('#editor').getByRole(...)` | `css('#editor').frame().role(...)` |
| `locator.and(...)`, `locator.or(...)`, XPath, `getByRole` state options | `new Locator({ ..., and / or / xpath / checked ... })` (see [Locators](/e2e/locators#locators-without-a-builder)) |

### Actions

| Playwright | Native |
|---|---|
| `click()`, `dblclick()`, `hover()` | `.click()`, `.dblclick()`, `.hover()` with `button`, `position`, `force` |
| `fill(v)` | `.fill(v)` |
| `pressSequentially(t)` | `.type(t)` |
| `press('Enter')`, `press('ControlOrMeta+A')` | `.press('Enter')`, `.press('ControlOrMeta+A')` |
| `check()`, `uncheck()` | `.check()`, `.uncheck()` |
| `focus()`, `blur()` | `.focus()`, `.blur()` |
| `setInputFiles(files)` | `.upload(files)` |
| `dragTo(target)` | `.drag(target)` |
| `page.mouse.click(x, y)`, `mouse.move`, `mouse.wheel` | `mouse('click', { x, y })`, `mouse('move', ...)`, `loc.wheel(dx, dy)` or `mouse('wheel', ...)` |

### Assertions

| Playwright | Native |
|---|---|
| `expect(loc).toBeVisible()` / `toBeHidden()` | `check(loc).visible()` / `.hidden()` |
| `toHaveText(t)` / `toContainText(t)` | `.text(t)` / `.containsText(t)` |
| `toHaveValue(v)` | `.value(v)` |
| `toHaveCount(n)` | `.count(n)` |
| `toBeChecked()` | `.checked()` |
| `toHaveCSS(p, v)` | `.css(p, v)` |
| `toBeFocused()` | `.focused()` |
| `toHaveAccessibleName(n)` | `.name(n)` |
| `toBeEnabled()` / `toBeDisabled()` | `.enabled()` / `.disabled()` |
| `expect(loc).not.toBeVisible()` | `check(loc).not.visible()` |
| `expect.poll(() => loc.count()).toBe(1)` | `check(loc).count(1)` |
| `expect(page).toHaveURL(u)` | `waitForUrl(u)` |
| `expect(page).toHaveScreenshot(n)` | `matchScreenshot(n)` (own baseline paths, see [Visual tests](/e2e/screenshots)) |

Native `.text()` compares exact after whitespace normalization; it takes a string, not a `RegExp`. For a pattern, use a locator: `check(text(/^Closed by /)).visible()`.

### Navigation, network, pages

| Playwright | Native |
|---|---|
| `browser.newContext({ storageState })` + `newPage()` + `goto()` | `lp.profile(name, { url, localStorage })` once, then `lp.page({ as, route })` |
| `page.goto(url)` | `lp.page({ url })` or a `goto(url)` step |
| `page.reload()` | `reload()` |
| `page.waitForURL(u)` | `waitForUrl(u)` |
| `page.waitForLoadState('load')` | `loaded()` |
| `page.waitForLoadState('networkidle')` | Usually nothing (actions settle); else `idle()` |
| `Promise.all([page.waitForResponse(...), loc.click()])` | `loc.click()`, then `check.request(method, path).status(200)` |
| `page.route(url, r => r.fulfill({ json }))` | `mock(url, { json })` |
| `route.abort()` | `mock(url, { abort: 'Failed' })` |
| `route.continue()` in front of a broader route | `mock(url, { passthrough: true })` |
| `route.fetch()` then `fulfill` with a changed body | `mock(url, { patch: { json } })` |
| `page.unroute(url)` | `unmock(url)` |
| `page.evaluate(fn)` | `evaluate('expression', 'name')` (a string; promises are awaited) |
| `page.addInitScript(src)` | profile `initScripts`, `lp.page({ initScripts })`, or `await page.addInitScript(src)` before a `goto`/`reload` |
| `page.setViewportSize(s)` | `viewport(w, h)` |
| `page.emulateMedia({ colorScheme })` | `media({ colorScheme })` |
| `page.waitForEvent('popup')` | `popup('name')`, then `switchTo('name')` |
| `page.waitForEvent('download')` | `download({ as, saveTo })` |
| `page.clock.runFor(ms)` | the wire step `{ advanceTime: { ms } }` (see [Steps](/e2e/steps#wire-only-steps)) |
| `loc.textContent()`, `inputValue()`, `count()`, `isVisible()`, `boundingBox()` | `loc.read(as, 'text' / 'value' / 'count' / 'visible' / 'box')` |

### Things to delete

- `waitForTimeout`, sleeps, retry loops around clicks, "wait for spinner to disappear" helpers: actions wait for actionability and settle.
- `waitForResponse` set up before an action: assert with `check.request` after it.
- Login through the UI in `beforeEach`: put the session in the profile, or sign in once in a fork-mode `setup`.
- Seeding through the UI: call your API in the test before the run.

## How to port a test

1. Move the session and the starting page into a profile. Seed data through the API in the test.
2. Translate each line with the tables above into a step, in order, inside one `page.run([...])`.
3. Replace response waits with `check.request` after the action.
4. Turn `textContent()`-style reads that feed later logic into `read` steps, and assert on `r.values` after the run.
5. Run it, then delete what the native driver does for you (waits, retries).
6. If the test needs plain code mid-scenario (an API call that changes data), split it into two runs on the same page.

Coding agents can do much of this mechanically: point them at the package's `LLM.md` (see [Writing tests with AI agents](/e2e/ai-agents)).
