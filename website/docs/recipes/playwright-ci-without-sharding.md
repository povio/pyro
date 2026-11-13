# Run a large suite in CI without --shard

`pyro run` has no `--shard`. This recipe shows what to do instead on GitHub Actions: size the workers of one job first, then, if one machine is not enough, split the test files across a job matrix with a small script that passes them to `pyro run` as file filters. Every job computes the same split from the same checkout.

The job script and every command below were run locally with Bun 1.4 on macOS, against the `@povio/pyro` package built from the repository (see [Before the first release](/guide/installation#try-today)). The workflow file was not run on GitHub for this page.

## What is and is not supported

| You have in Playwright | Under `pyro run` |
| --- | --- |
| `--shard=1/4` | Not supported: `unknown option --shard`, exit status 2. Split files yourself (below). |
| `--workers` / `-j` | Supported, as a number or a percentage of the CPUs. Default: the config's `workers`, else half the CPUs. |
| `--grep`, `--grep-invert`, tags | Supported. Usable to split by tag. |
| `--retries`, `--max-failures`, `-x` | Supported. |
| `--reporter=html`, `junit`, `json`, blob reports, `merge-reports` | Not supported. Reporters are `list`, `line` and `dot`, printed to the log. |
| Traces, videos, screenshot on failure | Not supported. `test-results/` holds the `-actual`, `-expected` and `-diff` images of screenshot mismatches. |
| `npx playwright install --with-deps` | Not needed. The browser is in the npm package. |
| Exit status | 1 when a test failed or a worker crashed, 0 otherwise. 2 for an unknown flag. |

The native runner `pyro e2e` does have `--shard` and `--junit`, but it runs native tests, not Playwright ones. See [CI recipes](/e2e/ci).

## Step 1: size the workers of one job

Each `pyro run` worker is a `bun test` process with its own Pyro browser, and your app server runs on the same machine. Start with the default, then try more, and keep the fastest:

```sh
bunx pyro run --reporter dot             # half the CPUs
bunx pyro run --reporter dot -j 100%     # one worker per CPU
```

On a GitHub runner, `nproc` prints the CPU count. Files are dealt out whole, largest first, so a few very large files limit how far workers help. Split those files, or mark their describes `parallel`, before you add jobs.

## Step 2: split files across jobs

When one machine is not enough, run the same command in N jobs, each with its own share of the files. `pyro run` takes test filters: regular expressions that a file's path must match. The script below picks this job's files and runs `pyro run` with one exact filter per file.

```ts
// e2e/ci-job.ts
// bun e2e/ci-job.ts <job> <jobs> [pyro run options...]
// Runs the test files of CI job <job> (1-based) of <jobs> with `pyro run`.
// Files go to the job with the fewest bytes so far, largest first, so every
// job computes the same split from the same checkout.
import { statSync } from 'node:fs';
import { Glob } from 'bun';

const [jobArg, jobsArg, ...options] = process.argv.slice(2);
const [job, jobs] = [Number(jobArg), Number(jobsArg)];
if (!(job >= 1 && job <= jobs)) throw new Error('usage: bun e2e/ci-job.ts <job> <jobs> [pyro run options...]');

const files = [...new Glob('e2e/**/*.{spec,test}.ts').scanSync('.')]
  .map((f) => ({ f, size: statSync(f).size }))
  .sort((a, b) => b.size - a.size || a.f.localeCompare(b.f));

const load = Array<number>(jobs).fill(0);
const mine: string[] = [];
for (const { f, size } of files) {
  const k = load.indexOf(Math.min(...load));
  load[k] += size;
  if (k === job - 1) mine.push(f);
}
if (!mine.length) {
  console.log(`job ${job}/${jobs}: no files`);
  process.exit(0); // without filters, pyro run would run every file
}
console.log(`job ${job}/${jobs}: ${mine.join(' ')}`);

// A filter is a regular expression on the file path: match each file exactly.
const filters = mine.map((f) => `^${f.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`);
const run = Bun.spawn(['bunx', 'pyro', 'run', ...options, ...filters], { stdio: ['inherit', 'inherit', 'inherit'] });
process.exit(await run.exited);
```

How it works:

- It lists the test files with the same pattern as the config's `testMatch` (here the default, `*.spec.ts` and `*.test.ts` under `e2e/`). Change the glob if yours differs.
- It sorts them by size, largest first, and gives each to the job with the fewest bytes so far. The result depends only on the checkout, so the jobs agree without talking to each other.
- Each filter is `^<path>$` with the dots escaped, so `e2e/form.spec.ts` does not also match `e2e/form.spec.ts.bak` or `e2e/old-form.spec.ts`. Filters match the path relative to the working directory: run the script from the directory that holds the config.
- A job with no files exits 0 without running `pyro run`. Without that check, `pyro run` with no filters would run every file.
- Options after the two numbers go to `pyro run` unchanged, and its exit status becomes the script's.

The split ignores how long each file takes, only its size. If one job is always the slowest, move a file by hand or give that job's files their own tag.

The suite used here is the one from [Sign in once with a setup project's storageState](/recipes/playwright-setup-project-auth), with two more files:

::: details The suite: app, global setup, config and four spec files

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

```ts
// e2e/form.spec.ts
import { test, expect } from '@povio/pyro/wright';

test('clears the field after adding', async ({ page }) => {
  await page.goto('/todos');
  await page.getByLabel('New todo').fill('Pay rent');
  await page.getByRole('button', { name: 'Add' }).click();
  await expect(page.getByLabel('New todo')).toHaveValue('');
});

test('says "Saved" when done', { tag: '@slow' }, async ({ page }) => {
  await page.goto('/todos');
  await page.getByLabel('New todo').fill('Book flights');
  await page.getByRole('button', { name: 'Add' }).click();
  await expect(page.getByRole('status')).toHaveText('Saved');
});
```

```ts
// e2e/admin/empty.spec.ts
import { test, expect } from '@povio/pyro/wright';

test('shows the empty state', async ({ page }) => {
  await page.route('**/api/todos', (route) => route.fulfill({ json: [] }));
  await page.goto('/todos');
  await expect(page.getByText('Nothing to do')).toBeVisible();
});
```

:::

## Try the split locally

Run each job of a three-job split. Check that every file appears in exactly one job:

```sh
bun e2e/ci-job.ts 1 3 --reporter list
bun e2e/ci-job.ts 2 3 --reporter list
bun e2e/ci-job.ts 3 3 --reporter list
```

```text
job 1/3: e2e/signed-out.spec.ts

Running 1 file using 5 workers

  ✓   1 e2e/signed-out.spec.ts › redirects to sign-in (48ms)
  ✓   2 e2e/signed-out.spec.ts › rejects a wrong password (114ms)

  2 passed (0.3s)
job 2/3: e2e/form.spec.ts

Running 1 file using 5 workers

  ✓   1 e2e/form.spec.ts › clears the field after adding (286ms)
  ✓   2 e2e/form.spec.ts › says "Saved" when done (177ms)

  2 passed (0.6s)
job 3/3: e2e/todos.spec.ts e2e/admin/empty.spec.ts

Running 2 files using 5 workers

  ✓   1 e2e/todos.spec.ts › lists the todos (168ms)
  ✓   2 e2e/admin/empty.spec.ts › shows the empty state (166ms)
  ✓   3 e2e/todos.spec.ts › adds a todo (170ms)

  3 passed (0.4s)
```

(The global setup's `pyro: CDP server listening on ...` line is left out above.) `--list` shows a job's tests without running them:

```sh
bun e2e/ci-job.ts 3 3 --list
```

```text
job 3/3: e2e/todos.spec.ts e2e/admin/empty.spec.ts
  e2e/admin/empty.spec.ts › shows the empty state
  e2e/todos.spec.ts › lists the todos
  e2e/todos.spec.ts › adds a todo

Total: 3 tests in 2 files
```

More jobs than files is safe:

```sh
bun e2e/ci-job.ts 5 5
```

```text
job 5/5: no files
```

A failing test fails the job: with `--timeout 1` added, the script exited with status 1.

## The workflow

```yaml
# .github/workflows/e2e.yml
name: E2E

on:
  pull_request:
  push:
    branches: [main]

jobs:
  e2e:
    runs-on: ubuntu-24.04             # the Linux browser needs glibc 2.39+
    timeout-minutes: 30
    strategy:
      fail-fast: false
      matrix:
        job: [1, 2, 3, 4]
    steps:
      - uses: actions/checkout@v4
      - uses: oven-sh/setup-bun@v2
      - uses: actions/cache@v4
        with:
          path: ~/.bun/install/cache
          key: bun-${{ runner.os }}-${{ runner.arch }}-${{ hashFiles('bun.lock') }}
      - run: bun install --frozen-lockfile
      - name: E2E (job ${{ matrix.job }} of 4)
        run: bun e2e/ci-job.ts ${{ matrix.job }} 4 --reporter dot --retries 2 -j 100%
      - uses: actions/upload-artifact@v4
        if: failure()
        with:
          name: test-results-${{ matrix.job }}
          path: test-results/
          if-no-files-found: ignore
```

What each part is for:

- **`ubuntu-24.04`.** The bundled Linux browsers need glibc 2.39 or newer. `ubuntu-22.04` does not have it.
- **No browser install step.** Remove `npx playwright install --with-deps` and any `~/.cache/ms-playwright` cache.
- **The Bun cache.** The package carries the browsers for all four platforms, so it is large. Caching `~/.bun/install/cache` keyed on `bun.lock` saves downloading it in every job. If you install with npm, cache `~/.npm` keyed on `package-lock.json` instead.
- **Every job runs `webServer` and `globalSetup` itself.** Each job is its own machine, so each starts the app and signs in once.
- **`test-results/`** holds the images of screenshot mismatches. There is no HTML report or trace to upload.
- **The pass/fail summary** is in each job's log. There is no JUnit file for GitHub's test summary, and no report merging across jobs.

Change `4` in two places (the matrix and the command) to change the number of jobs.

## Splitting by tag instead

If a group of tests is slow or needs its own environment, tag it and give it its own job. Tags are part of what `--grep` matches:

```ts
test('says "Saved" when done', { tag: '@slow' }, async ({ page }) => {
  // ...
});
```

```sh
bunx pyro run -g @slow                  # the tagged tests
bunx pyro run --grep-invert @slow       # everything else
```

```text
Running 4 files using 5 workers

  ✓   1 e2e/form.spec.ts › says "Saved" when done (270ms)

  1 passed (0.3s)
```

With a tag split, every job still loads every test file to find its tests. A file split gives each job only its own files.

## Screenshot baselines in CI

Baselines carry the platform in their file name (`-linux`, `-darwin`), and Pyro renders with the operating system's fonts. Write the Linux baselines on the CI image, review them, and commit them. Run CI with `-u none` so a missing baseline fails instead of being written. See [Screenshots and snapshots](/playwright/snapshots#re-baselining-on-migration).

## Next

- Before you switch CI over, run both browsers on the same suite and compare: [Side by side](/recipes/playwright-side-by-side).
- The first red run: [Triage the first failures](/recipes/playwright-triage-first-failures).
- All runner flags: [Config and the test runner](/playwright/runner).
