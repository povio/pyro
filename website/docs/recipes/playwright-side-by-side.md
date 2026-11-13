# Run the same suite on Chromium and Pyro

During a migration you want one set of test files that runs on both browsers: stock Playwright on Chromium, which you trust today, and `pyro run`. Then every red test on Pyro can be checked against Chromium in one step, and you can time both on the same machine before you decide.

This recipe sets that up with one switch: every test imports `test` and `expect` from a local `e2e/fixtures.ts`, which loads either `@playwright/test` or `@povio/pyro/wright` depending on `E2E_BROWSER`. A small script compares the two runs.

Every file below was run as shown with Bun 1.4 and Node 24 on macOS, with `@playwright/test` 1.59.1 and the `@povio/pyro` package built from the repository (see [Before the first release](/guide/installation#try-today)).

## Install both

Keep Playwright installed until the migration is done:

```sh
bun add --dev @povio/pyro @playwright/test
npx playwright install chromium
```

## The files

::: details app/server.ts: the app under test, the same as in the other Playwright recipes

```ts
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

:::

The switch. Your existing custom fixtures (here `todoPage`) move here, built on whichever `test` was loaded:

```ts
// e2e/fixtures.ts
// Every test imports `test` and `expect` from here, so one variable picks the
// implementation: E2E_BROWSER=chromium for stock Playwright, else Pyro.
import type * as Pyro from '@povio/pyro/wright';

const impl: typeof Pyro =
  process.env.E2E_BROWSER === 'chromium'
    ? ((await import('@playwright/test')) as any)
    : await import('@povio/pyro/wright');

export const chromium = impl.chromium;
export const expect = impl.expect;
export const test = impl.test.extend<{ todoPage: Pyro.Page }>({
  todoPage: async ({ page }, use) => {
    await page.goto('/todos');
    await use(page);
  },
});
export type { Page } from '@povio/pyro/wright';
```

Type-only imports from `@povio/pyro/wright` are erased at run time, so the Chromium run never loads Pyro, and the Pyro run never loads Playwright.

Shared values and the sign-in steps. Each browser writes its own storage state file:

```ts
// e2e/auth.ts
import type { Page } from './fixtures';

export const port = Number(process.env.PORT ?? 4517);
export const baseURL = `http://localhost:${port}`;
export const authFile = `e2e/.auth/user-${process.env.E2E_BROWSER ?? 'pyro'}.json`;

export async function signIn(page: Page, email = 'ana@example.com') {
  await page.goto('/login');
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill('secret');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.waitForURL('/todos');
}
```

Global setup signs in with whichever `chromium` the switch loaded:

```ts
// e2e/global-setup.ts
import { chromium } from './fixtures';
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

The options both runners share live in a module that imports neither runner. Each config wraps it with its own `defineConfig`. `pyro run` reads `pyro.config.ts` before `playwright.config.ts`; `playwright test` only reads `playwright.config.ts`.

```ts
// config.base.ts
// Options both runners share. No import of either runner here.
import { authFile, baseURL, port } from './e2e/auth';

export default {
  testDir: './e2e',
  globalSetup: './e2e/global-setup.ts',
  use: { baseURL, storageState: authFile },
  webServer: {
    command: 'bun app/server.ts',
    url: `${baseURL}/login`,
    env: { PORT: String(port) },
  },
};
```

```ts
// playwright.config.ts
import { defineConfig } from '@playwright/test';
import base from './config.base';
export default defineConfig(base);
```

```ts
// pyro.config.ts
import { defineConfig } from '@povio/pyro/wright';
import base from './config.base';

export default defineConfig(base, {
  // Pyro's baselines sit beside Chromium's instead of overwriting them.
  snapshotPathTemplate: '{testDir}/{testFilePath}-snapshots/{arg}-pyro-{platform}{ext}',
});
```

Pyro and Chromium render differently, so they need their own screenshot baselines. Without a project name both would write `login-darwin.png`; the `snapshotPathTemplate` in `pyro.config.ts` names Pyro's `login-pyro-darwin.png`, in the same folder.

The tests import from `./fixtures` and nothing else:

```ts
// e2e/todos.spec.ts
import { test, expect } from './fixtures';

test('lists the todos', async ({ todoPage }) => {
  await expect(todoPage.getByRole('listitem')).toHaveText(['Buy milk', 'Walk the dog']);
});

test('adds a todo', async ({ todoPage }) => {
  await todoPage.getByLabel('New todo').fill('Call mum');
  await todoPage.getByRole('button', { name: 'Add' }).click();
  await expect(todoPage.getByRole('listitem')).toContainText(['Call mum']);
});

test('shows "Saving..." while it saves', async ({ todoPage }) => {
  await todoPage.getByLabel('New todo').fill('Water plants');
  await todoPage.getByRole('button', { name: 'Add' }).click();
  await expect(todoPage.getByRole('status')).toHaveText('Saving...');
});

test('shows the empty state', async ({ page }) => {
  await page.route('**/api/todos', (route) => route.fulfill({ json: [] }));
  await page.goto('/todos');
  await expect(page.getByText('Nothing to do')).toBeVisible();
});
```

```ts
// e2e/signed-out.spec.ts
import { test, expect } from './fixtures';

// These tests start signed out.
test.use({ storageState: { cookies: [], origins: [] } });

test('redirects to sign-in', async ({ page }) => {
  await page.goto('/todos');
  await expect(page).toHaveURL('/login');
});

test('rejects a wrong password', async ({ page }) => {
  await page.goto('/login');
  await page.getByLabel('Email').fill('ana@example.com');
  await page.getByLabel('Password').fill('nope');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('alert')).toHaveText('Wrong email or password');
});
```

```ts
// e2e/visual.spec.ts
import { test, expect } from './fixtures';

test('sign-in page', async ({ page }) => {
  await page.goto('/login');
  await expect(page).toHaveScreenshot('login.png');
});
```

```text
# .gitignore
e2e/.auth/
test-results/
playwright-report/
```


## Run both

```sh
bunx pyro run                                    # Pyro
E2E_BROWSER=chromium npx playwright test         # Chromium
```

Each run starts its own web server on `PORT` (default 4517). To run both at once, give them different ports: `PORT=4518 bunx pyro run`.

```text
pyro: CDP server listening on ws://127.0.0.1:55762/

Running 3 files using 5 workers

  ✓   1 e2e/signed-out.spec.ts › redirects to sign-in (46ms)
  ✓   2 e2e/todos.spec.ts › lists the todos (104ms)
  ✓   3 e2e/signed-out.spec.ts › rejects a wrong password (70ms)
  ✓   4 e2e/visual.spec.ts › sign-in page (105ms)
  ✓   5 e2e/todos.spec.ts › adds a todo (160ms)
  ✘   6 e2e/todos.spec.ts › shows "Saving..." while it saves (5170ms)
  ✓   7 e2e/todos.spec.ts › shows the empty state (27ms)

  1) e2e/todos.spec.ts › shows "Saving..." while it saves ────────

    Error: expect(locator).toHaveText(expected) failed

    Locator: getByRole('status')
    Expected: "Saving..."
    Received: Saved
    Timeout: 5000ms
```

```text
Running 7 tests using 3 workers

  ✓  1 e2e/signed-out.spec.ts:6:1 › redirects to sign-in (161ms)
  ✓  2 e2e/todos.spec.ts:3:1 › lists the todos (218ms)
  ✓  3 e2e/visual.spec.ts:3:1 › sign-in page (379ms)
  ✓  4 e2e/signed-out.spec.ts:11:1 › rejects a wrong password (296ms)
  ✓  5 e2e/todos.spec.ts:7:1 › adds a todo (482ms)
  ✓  6 e2e/todos.spec.ts:13:1 › shows "Saving..." while it saves (227ms)
  ✓  7 e2e/todos.spec.ts:19:1 › shows the empty state (95ms)

  7 passed (4.1s)
```

The first visual run on each browser writes its baseline and fails, as Playwright does. Write both once with `-u`: `bunx pyro run visual -u` and `E2E_BROWSER=chromium npx playwright test visual -u`.

## Compare the results

`pyro run` has no JSON reporter, so the script reads both runs' `list` output. Save the two logs, then compare:

```ts
// compare.ts
// bun compare.ts chromium.txt pyro.txt
// Reads two `--reporter list` logs (Playwright's and pyro run's) and prints
// the tests whose result differs, then a timing summary.
import { readFileSync } from 'node:fs';

type Result = { status: 'passed' | 'failed' | 'skipped'; ms?: number };

function parse(file: string): Map<string, Result> {
  const out = new Map<string, Result>();
  const line = /^\s+([✓✘-])\s+\d+\s+(.+?)(?:\s+\((\d+(?:\.\d+)?)(ms|s|m)\))?(?:\s+\(.*\))?\s*$/;
  for (const raw of readFileSync(file, 'utf8').split('\n')) {
    const m = line.exec(raw.replace(/\x1b\[[0-9;]*m/g, ''));
    if (!m) continue;
    const key = m[2]
      .replace(/^\[[^\]]+\] › /, '')    // Playwright's [project]
      .replace(/:\d+:\d+ › /, ' › ')    // Playwright's file:line:column
      .replace(/ \(retry #\d+\)/, '');
    const ms = m[3] === undefined ? undefined : Number(m[3]) * (m[4] === 's' ? 1000 : m[4] === 'm' ? 60_000 : 1);
    const status = m[1] === '✓' ? 'passed' : m[1] === '✘' ? 'failed' : 'skipped';
    out.set(key, { status, ms }); // the last attempt wins
  }
  return out;
}

const [a, b] = [parse(process.argv[2]), parse(process.argv[3])];
const keys = [...new Set([...a.keys(), ...b.keys()])].sort();
const fmt = (r?: Result) => (r ? `${r.status}${r.ms === undefined ? '' : ` ${r.ms}ms`}` : 'missing');

console.log('Different results:');
let differ = 0;
for (const k of keys) {
  if (a.get(k)?.status === b.get(k)?.status) continue;
  differ++;
  console.log(`  ${k}\n    chromium: ${fmt(a.get(k))}\n    pyro:     ${fmt(b.get(k))}`);
}
if (!differ) console.log('  none');

const both = keys.filter((k) => a.get(k)?.status === 'passed' && b.get(k)?.status === 'passed');
const sum = (m: Map<string, Result>) => both.reduce((s, k) => s + (m.get(k)!.ms ?? 0), 0);
console.log(`\nTests passing on both: ${both.length} of ${keys.length}`);
console.log(`Sum of their durations: chromium ${sum(a)} ms, pyro ${sum(b)} ms`);
```

```sh
E2E_BROWSER=chromium npx playwright test --reporter=list > chromium.txt 2>&1
bunx pyro run --reporter list > pyro.txt 2>&1
bun compare.ts chromium.txt pyro.txt
```

```text
Different results:
  e2e/todos.spec.ts › shows "Saving..." while it saves
    chromium: passed 227ms
    pyro:     failed 5170ms

Tests passing on both: 6 of 7
Sum of their durations: chromium 1631 ms, pyro 512 ms
```

Every test under "Different results" is a migration problem, not an app bug: it passes on Chromium. This one checks a transient state that Pyro's settled click has already passed. [Triage the first failures](/recipes/playwright-triage-first-failures) lists the usual causes and fixes.

A test that fails on both is not Pyro's doing. Fix it, or leave it out of the comparison.

## Time both

Per-test durations leave out start-up: launching browsers, starting workers, global setup. Time whole runs too. Use the same worker count for both, leave out the tests that fail on Pyro, and alternate the runs so machine load hits both alike:

```sh
for i in 1 2; do
  E2E_BROWSER=chromium /usr/bin/time -p npx playwright test --reporter=dot --workers 4 --grep-invert Saving
  /usr/bin/time -p bunx pyro run --reporter dot -j 4 --grep-invert Saving
done
```

On this suite, on an Apple M1 Max (10 cores), the `real` times were:

| Run | Chromium (Playwright 1.59.1) | Pyro |
| --- | ---: | ---: |
| 1 | 2.80 s | 0.99 s |
| 2 | 3.08 s | 0.60 s |

Six tests say little about four hundred. Run your own suite this way, at least twice each, and on the CI runner type you use. Compare `real` time. The `user` and `sys` figures count only processes the runner waited for, so they can leave out browser processes and are not a fair CPU comparison. On CI, the job's duration is the number that turns into cost.

What usually makes the larger difference on a real app is its own JavaScript and its boot. See [Booting pages ahead](/playwright/boot-pools) and [Performance](/guide/performance).

## When you are done

Once the comparison shows no differences you care about:

1. Replace the switch in `e2e/fixtures.ts` with a plain `import { test as base, expect, chromium } from '@povio/pyro/wright'`.
2. Merge `config.base.ts` into one `playwright.config.ts` that imports `@povio/pyro/wright`, and delete `pyro.config.ts`. Keep the `snapshotPathTemplate`, or rename the Pyro baselines.
3. Remove `@playwright/test` and the Chromium baselines.

## Next

- Fix what differs: [Triage the first failures](/recipes/playwright-triage-first-failures).
- Move CI over: [Sharding in CI](/recipes/playwright-ci-without-sharding).
- Every known difference: [Behaviour differences](/playwright/differences).
