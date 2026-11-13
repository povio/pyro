# Sign in once with a setup project's storageState

Your Playwright suite probably signs in once in a `setup` project, saves `storageState`, and lets every other project depend on it. `pyro run` runs project `dependencies` as Playwright does, so that flow works as it is. This recipe shows the config, what a run does with it, how the flags treat the setup project, and the `globalSetup` variant for suites that prefer one.

## The setup project

```ts
// playwright.config.ts (imports already rewritten)
import { defineConfig } from '@povio/pyro/wright';

const port = Number(process.env.PORT ?? 4517);

export default defineConfig({
  testDir: './e2e',
  use: { baseURL: `http://localhost:${port}` },
  projects: [
    { name: 'setup', testMatch: /.*\.setup\.ts/ },
    {
      name: 'app',
      use: { storageState: 'e2e/.auth/user.json' },
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

`bunx pyro run` then goes in Playwright's order:

1. The web server starts and answers.
2. The `setup` project runs to completion. Its test signs in and writes `e2e/.auth/user.json`. `storageState({ path })` makes the `e2e/.auth` directory.
3. The `app` project starts. Every context begins with the saved cookies, so every test starts signed in.

The report names the project of each test, for example `[setup] › e2e/auth.setup.ts › sign in` and `[app] › e2e/todos.spec.ts › lists the todos`.

If the sign-in fails, the `app` project does not run. Its tests are counted as "did not run" in the summary, and the exit status is 1.

::: tip Device descriptors
A Playwright config often has `use: { ...devices['Desktop Chrome'], storageState }` in the project. It works here too. The project above leaves it out because a descriptor sets `userAgent`, and a context with a `userAgent` gets no [booted pages](/playwright/boot-pools).
:::

## The app and the tests

The app under test (open the block to see it) has a sign-in page, a list and a form.

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

## What the flags do

| Command | What runs |
| --- | --- |
| `bunx pyro run` | `setup`, then `app`. |
| `bunx pyro run --project app` | The same: a selected project takes its dependencies along. |
| `bunx pyro run --project app --no-deps` | `app` only. It reads the storage state a previous run left. |
| `bunx pyro run todos` | `setup` in full, then the `app` files whose path matches `todos`. File filters and `--grep` do not filter dependencies. |
| `bunx pyro run --shard 1/3` | `setup` in full, then a third of the `app` tests. Each shard signs in for itself. |
| `PYRO_CONFIG=playwright.config.ts bun test ./e2e/todos.spec.ts` | That file only. Plain `bun test` runs no dependencies, so the storage state must exist already. |

`--no-deps` and plain `bun test` rely on a saved session that is still valid. Each `pyro run` starts and stops its own web server, so the session must survive a server restart. With this recipe's in-memory app it does not. It works when the server keeps sessions in a database, or when you start the server yourself before both commands (`webServer` reuses a server that already answers).

## Clean up with a teardown project

A `teardown` project runs after every project that depends on its setup project has finished, whatever the result:

```ts
projects: [
  { name: 'setup', testMatch: /.*\.setup\.ts/, teardown: 'cleanup' },
  { name: 'cleanup', testMatch: /.*\.teardown\.ts/ },
  { name: 'app', use: { storageState: 'e2e/.auth/user.json' }, dependencies: ['setup'] },
],
```

```ts
// e2e/auth.teardown.ts
import { rmSync } from 'node:fs';
import { test as teardown } from '@povio/pyro/wright';

teardown('remove the saved session', async () => {
  rmSync('e2e/.auth', { recursive: true, force: true });
});
```

## The variant: sign in from globalSetup

Global setup also works, and it is the way to go when the sign-in must happen before any project, or under plain `bun test`, which runs global setup before the first file. The sign-in steps live in one module, so global setup and any test that must sign in itself share them.

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
  webServer: {
    command: 'bun app/server.ts',
    url: `${baseURL}/login`,
    env: { PORT: String(port) },
  },
});
```

```sh
bunx pyro run
```

A run of this variant on macOS with Bun 1.4:

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

## Things to know

- **Paths are relative to the working directory.** `storageState: 'e2e/.auth/user.json'` is read from where you run `pyro run`. Run it from the directory that holds the config.
- **Global setup gets Playwright's resolved `FullConfig`.** `const { baseURL, storageState } = config.projects[0].use` works, and the `FullConfig` type is exported from `@povio/pyro/wright`. The shared module above is a choice, not a need.
- **The setup project's tests are tests.** They are counted in the summary, they retry with the project's `retries`, and `--list` lists them.
- **Cookies turn off booted pages.** A context with cookies from `storageState` never takes a [booted page](/playwright/boot-pools). With `use.pyro.boot` added to this suite, `browser._stats` read `{"pooled":0,"fresh":2}` after two tests. Booted pages work when the signed-in state lives in `localStorage` only.

## Next

- Run it in CI: [Sharding in CI](/recipes/playwright-ci-without-sharding).
- Run the same tests on Chromium and Pyro and compare: [Side by side](/recipes/playwright-side-by-side).
- Dependencies, teardown and `--no-deps`: [Config and the test runner](/playwright/runner#dependencies-and-teardown).
- Storage state details: [Network: storageState and cookies](/playwright/network#storagestate-and-cookies).
