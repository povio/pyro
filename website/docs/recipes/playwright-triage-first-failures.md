# Triage the first red run

The first `pyro run` of a migrated suite usually fails in a handful of ways, each repeated across many tests. This page is a checklist: catch what you can before running, run once and group the errors, then fix each kind with the pattern below. Every failure here was reproduced on a small suite whose tests all pass on Chromium, and every fix was run on both Pyro and Chromium (Playwright 1.59.1), with Bun 1.4 on macOS. The output shown is from those runs.

## Before the first run

These checks take minutes and find failures without running anything.

**1. Type-check against Pyro's types.** The selector-first page methods and the deprecated APIs Pyro leaves out are type errors:

```sh
npx tsc --noEmit
```

```text
e2e/first-run.spec.ts(5,14): error TS2339: Property 'fill' does not exist on type 'Page'.
```

Unsupported `toHaveScreenshot` options (`animations`, `mask`, ...) are in the types, so `tsc` does not catch them. Search for them:

```sh
grep -rnE "animations|mask|maskColor|caret|scale|stylePath" e2e --include='*.ts' | grep -i screenshot
```

```text
e2e/first-run.spec.ts:27:  await expect(page).toHaveScreenshot('login-still.png', { animations: 'disabled' });
```

The search only finds options written on the same line as the call.

**2. Search for runner features that are not there.**

```sh
grep -rn "show-report\|merge-reports\|\-\-ui\|\-\-trace\|\-\-headed\|\-\-debug" package.json .github
grep -rn "reporter\|trace" playwright.config.ts
```

- `--trace` is an unknown option and ends the run with status 2. `--ui`, `--headed` and `--debug` print a warning and do nothing.
- `show-report` and `merge-reports` are not commands of `pyro run`. The reporters are `list`, `line` and `dot`; the config's `reporter` is ignored.
- `use.trace` is accepted and records nothing. For a failed test's evidence use `screenshot` and `video`: see [Config and the test runner](/playwright/runner#screenshots-of-failed-tests).

Setup projects (`dependencies`, `teardown`), `--shard`, and `testInfo.status` in an `afterEach` work as in Playwright and need no change.

**3. Check the browser starts.** Before the first `@povio/pyro` release, a package installed from a checkout has no bundled browser. The first run then fails in global setup or in the first test with:

```text
error: Missing bundled browser: /Users/xban1x/Povio/pyro-rename/packages/pyro/native/darwin-arm64/pyro. Reinstall @povio/pyro or set PYRO_BIN to a built browser.
```

Set `PYRO_BIN` to the browser you built. See [Before the first release](/guide/installation#try-today).

## Run once and group the errors

Run the whole suite once, keep the log, and count the error lines. Fix the biggest group first.

```sh
bunx pyro run --reporter dot > run.txt 2>&1
grep -E '^    [A-Za-z]*Error' run.txt | cut -c1-120 | sort | uniq -c | sort -rn
```

On the example suite below, with its five typical failures:

```text
   1     TypeError: page.fill is not a function. (In 'page.fill("input[name=text]", "Call mum")', 'page.fill' is undefined)
   1     TimeoutError: locator.click: Timeout 2000ms exceeded.
   1     Error: expect(locator).toHaveText(expected) failed
   1     Error: Pyro.run: step 0: unknown field `animations`, expected one of `name`, `baseline`, `sceneHash`, `threshold`, `
   1     Error: Pyro.run: step 0 (matchScreenshot /private/tmp/docs-r2-pw/r4/e2e/first-run.spec.ts-snapshots/login-chromium-d
```

On a 400-test suite, the counts tell you which fix pays most. To check one test against Chromium, run the same suite on both: see [Side by side](/recipes/playwright-side-by-side).

While you fix, rerun only what you touch: `bunx pyro run e2e/checkout` (a file path regex) or `bunx pyro run -g "adds a todo"` (a title regex). `-x` stops at the first failure.

## The example suite

All five tests pass on Chromium with stock Playwright, with baselines it wrote. Under `pyro run` all five fail.

::: details The suite: app, sign-in helpers and global setup (as in the sign-in recipe), config

```ts
// app/server.ts
// A small app to test: sign in, a list of todos, a form that adds one.
const port = Number(process.env.PORT ?? 4517);
const sessions = new Map<string, string[]>(); // session id -> todos

const page = (title: string, body: string) =>
  new Response(
    `<!doctype html><html><head><meta charset="utf-8"><title>${title}</title>
<style>body{font:16px system-ui;margin:2rem}li{margin:.25rem 0}</style></head>
<body>${body}</body></html>`,
    { headers: { 'content-type': 'text/html' } },
  );

const sid = (req: Request) => /(?:^|; )sid=([^;]+)/.exec(req.headers.get('cookie') ?? '')?.[1];

Bun.serve({
  port,
  async fetch(req) {
    const url = new URL(req.url);
    const id = sid(req);
    const signedIn = id !== undefined && sessions.has(id);

    if (url.pathname === '/login') {
      return page('Sign in', `
<h1>Sign in</h1>
<form id="f">
  <label>Email <input name="email" type="email"></label>
  <label>Password <input name="password" type="password"></label>
  <button>Sign in</button>
</form>
<p role="alert" id="err"></p>
<script>
  f.onsubmit = async (e) => {
    e.preventDefault();
    const r = await fetch('/api/login', { method: 'POST', body: new FormData(f) });
    if (r.ok) location.href = '/todos'; else err.textContent = 'Wrong email or password';
  };
</script>`);
    }

    if (url.pathname === '/api/login' && req.method === 'POST') {
      const form = await req.formData();
      if (form.get('password') !== 'secret') return new Response('no', { status: 401 });
      const newId = crypto.randomUUID();
      sessions.set(newId, ['Buy milk', 'Walk the dog']);
      return new Response('ok', { headers: { 'set-cookie': `sid=${newId}; Path=/; HttpOnly` } });
    }

    if (url.pathname === '/api/todos') {
      if (!signedIn) return new Response('[]', { status: 401 });
      const todos = sessions.get(id!)!;
      if (req.method === 'POST') {
        const { title } = (await req.json()) as { title: string };
        await Bun.sleep(150); // a slow save, so the app shows "Saving..."
        todos.push(title);
        return Response.json(todos, { status: 201 });
      }
      return Response.json(todos);
    }

    if (url.pathname === '/todos') {
      if (!signedIn) return Response.redirect('/login', 302);
      return page('Todos', `
<h1>Todos</h1>
<user-badge></user-badge>
<ul id="list"></ul>
<p id="empty" hidden>Nothing to do</p>
<form id="add">
  <label>New todo <input name="text"></label>
  <button>Add</button>
  <span id="state" role="status"></span>
</form>
<script>
  customElements.define('user-badge', class extends HTMLElement {
    connectedCallback() {
      this.attachShadow({ mode: 'open' }).innerHTML = '<button>Sign out</button>';
    }
  });
  const render = (todos) => {
    list.replaceChildren(...todos.map((t) => Object.assign(document.createElement('li'), { textContent: t })));
    empty.hidden = todos.length > 0;
  };
  fetch('/api/todos').then((r) => r.json()).then(render);
  add.onsubmit = async (e) => {
    e.preventDefault();
    state.textContent = 'Saving...';
    const r = await fetch('/api/todos', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ title: add.text.value }),
    });
    render(await r.json());
    add.reset();
    state.textContent = 'Saved';
  };
</script>`);
    }

    return Response.redirect('/login', 302);
  },
});
console.log(`app on http://localhost:${port}`);
```

```ts
// e2e/auth.ts
import type { Page } from '@povio/pyro/wright';

export const port = Number(process.env.PORT ?? 4517);
export const baseURL = `http://localhost:${port}`;
export const authFile = 'e2e/.auth/user.json';

/** The sign-in steps, shared by global setup and by tests that sign in themselves. */
export async function signIn(page: Page, email = 'ana@example.com') {
  await page.goto('/login');
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill('secret');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.waitForURL('/todos');
}
```

```ts
// e2e/global-setup.ts
import { chromium } from '@povio/pyro/wright';
import { authFile, baseURL, signIn } from './auth';

export default async function globalSetup() {
  const browser = await chromium.launch();
  const context = await browser.newContext({ baseURL });
  const page = await context.newPage();
  await signIn(page);
  await context.storageState({ path: authFile });
  await browser.close();
}
```

```ts
// playwright.config.ts
import { defineConfig } from '@povio/pyro/wright';
import { authFile, baseURL, port } from './e2e/auth';

export default defineConfig({
  testDir: './e2e',
  globalSetup: './e2e/global-setup.ts',
  use: { baseURL, storageState: authFile },
  projects: [{ name: 'chromium' }],
  webServer: { command: 'bun app/server.ts', url: `${baseURL}/login`, env: { PORT: String(port) } },
});
```

:::

```ts
// e2e/first-run.spec.ts
import { test, expect } from '@povio/pyro/wright';

test('selector-first fill', async ({ page }) => {
  await page.goto('/todos');
  await page.fill('input[name=text]', 'Call mum');
});

test('sign out button in a shadow root', async ({ page }) => {
  await page.goto('/todos');
  await page.getByRole('button', { name: 'Sign out' }).click({ timeout: 2000 });
});

test('transient "Saving..."', async ({ page }) => {
  await page.goto('/todos');
  await page.getByLabel('New todo').fill('Water plants');
  await page.getByRole('button', { name: 'Add' }).click();
  await expect(page.getByRole('status')).toHaveText('Saving...');
});

test('login page looks right', async ({ page }) => {
  await page.goto('/login');
  await expect(page).toHaveScreenshot('login.png');
});

test('login page, animations off', async ({ page }) => {
  await page.goto('/login');
  await expect(page).toHaveScreenshot('login-still.png', { animations: 'disabled' });
});
```


`bunx pyro run first-run` (stack frames left out):

```text
Running 1 file using 5 workers

  ✘   1 e2e/first-run.spec.ts › selector-first fill (44ms)
  ✘   2 e2e/first-run.spec.ts › sign out button in a shadow root (2009ms)
  ✘   3 e2e/first-run.spec.ts › transient "Saving..." (5186ms)
  ✘   4 e2e/first-run.spec.ts › login page looks right (23ms)
  ✘   5 e2e/first-run.spec.ts › login page, animations off (4ms)
```

## The failures, one by one

### Selector-first page methods

```text
TypeError: page.fill is not a function. (In 'page.fill("input[name=text]", "Call mum")', 'page.fill' is undefined)
```

**Spot it:** `is not a function` on `page.` or `frame.`, or `tsc` errors. The same goes for `page.click`, `page.check`, `page.hover`, `page.press`, `page.textContent(selector)`, `page.getAttribute`, `page.isVisible(selector)`, `page.selectOption`, `page.dispatchEvent` and the rest of the selector-first forms.

**Fix:** call the same method on a locator. `page.$()`, `page.$$()`, `page.$eval()` and `page.waitForSelector()` exist and need no change.

```ts
await page.locator('input[name=text]').fill('Call mum');
```

### Elements inside a shadow root

```text
TimeoutError: locator.click: Timeout 2000ms exceeded.
Call log:
  - waiting for getByRole('button', { name: 'Sign out' })
  - {"name":"Sign out","role":"button"} matched no element
```

**Spot it:** `matched no element` for an element you can see, inside a web component. Playwright finds it because its CSS and `getByRole` locators enter open shadow roots. Pyro's CSS, `getByRole`, `getByLabel` and `getByPlaceholder` locators do not; `getByText` does.

**Fix:** locate it by text.

```ts
await page.getByText('Sign out').click();
```

### Transient states

```text
Error: expect(locator).toHaveText(expected) failed

Locator: getByRole('status')
Expected: "Saving..."
Received: Saved
Timeout: 5000ms
```

**Spot it:** the received value is the state after the one the test expects: "Saved" for "Saving...", a list for a spinner. The test takes the full `expect` timeout (5 s) to fail. In Pyro, `click()` returns after the page settles, including the request the click started, so the in-between state is already gone.

**Fix:** assert the final state, and check the request if it matters:

```ts
const saved = page.waitForResponse((r) => r.url().endsWith('/api/todos') && r.request().method() === 'POST');
await page.getByRole('button', { name: 'Add' }).click();
expect((await saved).status()).toBe(201);
await expect(page.getByRole('status')).toHaveText('Saved');
```

If the in-between state is what the test is about, hold the request in a route. The click then returns while the request waits, and the state is there to see:

```ts
await page.route('**/api/todos', async (route) => {
  if (route.request().method() === 'POST') await new Promise((r) => setTimeout(r, 1000));
  await route.continue();
});
```

### Screenshot options Pyro rejects

```text
Error: Pyro.run: step 0: unknown field `animations`, expected one of `name`, `baseline`, `sceneHash`, `threshold`, `maxDiffPixels`, `maxDiffPixelRatio`, `clip`, `fullPage`, `locator`, `update`, `captureOnly`, `as`, `timeout` at line 1 column 132
```

**Spot it:** `unknown field` in a `toHaveScreenshot` call. Pyro accepts `threshold`, `maxDiffPixels`, `maxDiffPixelRatio`, `clip`, `fullPage`, `omitBackground` and `timeout`.

**Fix:** remove the option. Pyro renders a still frame, so there is no animation or caret to turn off. `mask` has no replacement; clip the screenshot to a stable region, or screenshot a locator.

### Screenshot baselines from Chromium

```text
Error: Pyro.run: step 0 (matchScreenshot /private/tmp/docs-r2-pw/r4/e2e/first-run.spec.ts-snapshots/login-chromium-darwin.png): matchScreenshot '/private/tmp/docs-r2-pw/r4/e2e/first-run.spec.ts-snapshots/login-chromium-darwin.png': 3311 pixels differ (1280x720); the actual image and the diff are in error.data
  actual: /private/tmp/docs-r2-pw/r4/test-results/e2e-first-run.spec.ts-login-page-looks-right/login-actual.png
  diff:   /private/tmp/docs-r2-pw/r4/test-results/e2e-first-run.spec.ts-login-page-looks-right/login-diff.png
```

**Spot it:** `pixels differ` on every `toHaveScreenshot` with a baseline Chromium wrote. Pyro renders with its own CPU renderer, so they never match. `login-expected.png` is written beside the two images named in the message.

**Fix:** remove rejected options first (above). Then look at the `-actual` images: a page Pyro renders wrongly would otherwise become the new baseline. Then rewrite the baselines. Do it on the platform CI uses, since the file name carries the platform:

```sh
bunx pyro run -u all -g "login page"
```

```text
Running 2 files using 5 workers

  ✓   1 e2e/first-run.spec.ts › login page looks right (79ms)
  ✓   2 e2e/first-run.spec.ts › login page, animations off (8ms)

  2 passed (0.1s)
```

Each baseline gets a `.json` file beside it (`login-chromium-darwin.json`). Commit both. See [Screenshots and snapshots](/playwright/snapshots#re-baselining-on-migration).

## The fixed tests

```ts
// e2e/fixed.spec.ts
import { test, expect } from '@povio/pyro/wright';

test('locator fill', async ({ page }) => {
  await page.goto('/todos');
  await page.locator('input[name=text]').fill('Call mum');
  await expect(page.getByLabel('New todo')).toHaveValue('Call mum');
});

test('sign out button by text', async ({ page }) => {
  await page.goto('/todos');
  await expect(page.getByText('Sign out')).toBeVisible();
  await page.getByText('Sign out').click();
});

test('final state after saving', async ({ page }) => {
  await page.goto('/todos');
  const saved = page.waitForResponse((r) => r.url().endsWith('/api/todos') && r.request().method() === 'POST');
  await page.getByLabel('New todo').fill('Water plants');
  await page.getByRole('button', { name: 'Add' }).click();
  expect((await saved).status()).toBe(201);
  await expect(page.getByRole('status')).toHaveText('Saved');
});

test('"Saving..." with a held request', async ({ page }) => {
  await page.route('**/api/todos', async (route) => {
    if (route.request().method() === 'POST') await new Promise((r) => setTimeout(r, 1000));
    await route.continue();
  });
  await page.goto('/todos');
  await page.getByLabel('New todo').fill('Water plants');
  await page.getByRole('button', { name: 'Add' }).click();
  await expect(page.getByRole('status')).toHaveText('Saving...');
});
```


On Pyro:

```text
Running 1 file using 5 workers

  ✓   1 e2e/fixed.spec.ts › locator fill (85ms)
  ✓   2 e2e/fixed.spec.ts › sign out button by text (6ms)
  ✓   3 e2e/fixed.spec.ts › final state after saving (163ms)
  ✓   4 e2e/fixed.spec.ts › "Saving..." with a held request (14ms)
```

The same file, with its import changed to `@playwright/test`, on Chromium:

```text
  ✓  1 [chromium] › e2e/fixed.spec.ts:3:1 › locator fill (107ms)
  ✓  2 [chromium] › e2e/fixed.spec.ts:9:1 › sign out button by text (85ms)
  ✓  3 [chromium] › e2e/fixed.spec.ts:15:1 › final state after saving (247ms)
  ✓  4 [chromium] › e2e/fixed.spec.ts:24:1 › "Saving..." with a held request (128ms)
```

Each fix keeps the test valid on Chromium, so you can make them while both browsers still run the suite.

## Other failures to look for

These did not occur in the example suite, but show up in real ones. Each has a row in [Typical first failures](/playwright/migration#typical-first-failures):

- A `confirm()` dialog acts as if Cancel was pressed: no `page.on('dialog')` listener was registered before the action.
- `toHaveRole('button')` fails on a `<button>`: Pyro's `toHaveRole` reads the `role` attribute only.
- `getByTestId(/regex/)` or `click({ clickCount: 3 })` throws "not supported yet".
- The app shows an "unsupported browser" page, or a web API is `undefined`: see [Web platform support](/reference/web-platform).
- `page.goto()` returns `null` and the test reads `.status()` of it: the page was a [booted page](/playwright/boot-pools), whose app changes route without loading a document.
- A context with `recordVideo`, or a test with `video` on, fails with a message about ffmpeg: install one or set `PYRO_FFMPEG`. See [Video](/playwright/runner#video).
- Code that parses V8 stack traces gets JavaScriptCore's format.

## Next

- Compare every result with Chromium automatically: [Side by side](/recipes/playwright-side-by-side).
- The full list of differences: [Behaviour differences](/playwright/differences).
- Check one API: [API coverage](/playwright/api-coverage).
