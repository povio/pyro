# Sign in once with a setup project's storageState

Your Playwright suite probably signs in once in a `setup` project, saves `storageState`, and lets every other project depend on it. `pyro run` ignores project `dependencies`, so that flow breaks as it is. This recipe shows what you see, then moves the sign-in into `globalSetup`, which `pyro run` runs once, after the web server is up and before any worker starts. Your tests do not change.

Every file below was run as shown with Bun 1.4 on macOS, against the `@povio/pyro` package built from the repository (see [Before the first release](/guide/installation#try-today)).

## What breaks

A typical setup project:

```ts
// playwright.config.ts (as it was, imports already rewritten)
import { defineConfig, devices } from '@povio/pyro/wright';

const port = Number(process.env.PORT ?? 4517);

export default defineConfig({
  testDir: './e2e',
  use: { baseURL: `http://localhost:${port}` },
  projects: [
    { name: 'setup', testMatch: /.*\.setup\.ts/ },
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'], storageState: 'e2e/.auth/user.json' },
      dependencies: ['setup'],
    },
  ],
  webServer: {
    command: 'bun app/server.ts',
    url: `http://localhost:${port}/login`,
    env: { PORT: String(port) },
  },
});
```

```ts
// e2e/auth.setup.ts
import { test as setup, expect } from '@povio/pyro/wright';

setup('sign in', async ({ page }) => {
  await page.goto('/login');
  await page.getByLabel('Email').fill('ana@example.com');
  await page.getByLabel('Password').fill('secret');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.waitForURL('/todos');
  await expect(page.getByRole('heading', { name: 'Todos' })).toBeVisible();
  await page.context().storageState({ path: 'e2e/.auth/user.json' });
});
```

Under `bunx pyro run`, the setup file is just another test file. It runs in a worker at the same time as the tests that need its output:

```text
Running 2 files using 5 workers

  ✘   1 e2e/todos.spec.ts › lists the todos (26ms)
  ✘   2 e2e/todos.spec.ts › adds a todo (0ms)
  ✘   3 e2e/todos.spec.ts › shows the empty state (0ms)
  ✘   4 e2e/auth.setup.ts › sign in (89ms)

  1) e2e/todos.spec.ts › lists the todos ─────────────────────────

    Error: ENOENT: no such file or directory, open 'e2e/.auth/user.json'
```

The setup test fails too, with the same `ENOENT` from `writeFileSync`: `storageState({ path })` does not create missing directories, as Playwright's does.

## The fix: sign in from globalSetup

Four files, plus the app (open the block to see it). The sign-in steps live in one module, so global setup and any test that must sign in itself share them.

::: details app/server.ts: the app under test (sign-in, a list, a form)

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
import { mkdirSync } from 'node:fs';
import { dirname } from 'node:path';
import { chromium } from '@povio/pyro/wright';
import { authFile, baseURL, signIn } from './auth';

export default async function globalSetup() {
  const browser = await chromium.launch();
  const context = await browser.newContext({ baseURL });
  const page = await context.newPage();
  await signIn(page);
  mkdirSync(dirname(authFile), { recursive: true }); // storageState({ path }) does not create it
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
  webServer: {
    command: 'bun app/server.ts',
    url: `${baseURL}/login`,
    env: { PORT: String(port) },
  },
});
```

The tests are ordinary Playwright tests. They start signed in:

```ts
// e2e/todos.spec.ts
import { test, expect } from '@povio/pyro/wright';

test('lists the todos', async ({ page }) => {
  await page.goto('/todos');
  await expect(page.getByRole('listitem')).toHaveText(['Buy milk', 'Walk the dog']);
});

test('adds a todo', async ({ page }) => {
  await page.goto('/todos');
  await page.getByLabel('New todo').fill('Call mum');
  await page.getByRole('button', { name: 'Add' }).click();
  await expect(page.getByRole('listitem')).toContainText(['Call mum']);
});
```

Tests that must start signed out override the option with an empty state, as in Playwright:

```ts
// e2e/signed-out.spec.ts
import { test, expect } from '@povio/pyro/wright';

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

```text
# .gitignore
e2e/.auth/
test-results/
```

## Run it

```sh
bunx pyro run
```

```text
pyro: CDP server listening on ws://127.0.0.1:53569/

Running 2 files using 5 workers

  ✓   1 e2e/signed-out.spec.ts › redirects to sign-in (29ms)
  ✓   2 e2e/signed-out.spec.ts › rejects a wrong password (65ms)
  ✓   3 e2e/todos.spec.ts › lists the todos (89ms)
  ✓   4 e2e/todos.spec.ts › adds a todo (163ms)

  4 passed (0.3s)
```

The first line comes from the browser that global setup launched. The order is Playwright's: web servers start, global setup signs in and writes `e2e/.auth/user.json`, then the workers start. Global setup runs once per `pyro run`, not once per worker.

One file also runs with plain `bun test`, which loads the config and runs global setup before the first file:

```sh
PYRO_CONFIG=playwright.config.ts bun test ./e2e/todos.spec.ts
```

```text
 2 pass
 0 fail
Ran 2 tests across 1 file. [430.00ms]
```

## Things to know

- **Global setup gets the config object as you wrote it**, not Playwright's resolved `FullConfig`. `config.projects[0].use` is that project's own `use`, without the top-level options merged in, and is `undefined` when the project sets none. The common Playwright snippet `const { baseURL, storageState } = config.projects[0].use` then throws `Cannot destructure property 'baseURL' from null or undefined value`. Read such values from a shared module, as `e2e/auth.ts` does above. `@povio/pyro/wright` does not export a `FullConfig` type, so leave the parameter out or type it yourself.
- **Paths are relative to the working directory.** `storageState: 'e2e/.auth/user.json'` is read from where you run `pyro run`. Run it from the directory that holds the config.
- **Remove the setup project.** Without `--project`, `pyro run` runs every project's files, so a leftover `auth.setup.ts` runs as a test, in parallel with the rest. Delete the project, or keep it only in the Playwright config if you [run both side by side](/recipes/playwright-side-by-side).
- **Cookies turn off booted pages.** A context with cookies from `storageState` never takes a [booted page](/playwright/boot-pools). With `use.pyro.boot` added to this suite, `browser._stats` read `{"pooled":0,"fresh":2}` after two tests. Booted pages work when the signed-in state lives in `localStorage` only.
- **Keeping the setup project instead.** `bunx pyro run --project setup && bunx pyro run --project chromium` runs the setup file first, then the tests. Each command starts and stops its own web server, so the session must survive a server restart. With this recipe's in-memory app it does not, and every test failed. It works when the server keeps sessions in a database, or when you start the server yourself before both commands (`webServer` reuses a server that already answers).

## Next

- Run it in CI: [CI without sharding](/recipes/playwright-ci-without-sharding).
- Run the same tests on Chromium and Pyro and compare: [Side by side](/recipes/playwright-side-by-side).
- Storage state details: [Network: storageState and cookies](/playwright/network#storagestate-and-cookies).
