# Visual regression in CI

`matchScreenshot` renders the page in the browser and compares it there with a baseline PNG. This recipe sets up a visual suite you can run locally and in CI:

- fixed data from mocks, so pixels change only when your code does,
- one baseline directory per operating system,
- recording, reviewing and updating baselines, one test at a time,
- a CI job that uploads the actual and diff images when a screenshot does not match, and one that records the Linux baselines.

It uses the app, API and profiles of [Project layout](/recipes/e2e-project-layout). The tests and commands were run as shown with Bun 1.4.3 on macOS (Apple silicon). The GitHub Actions workflows run the same scripts; they were not run on GitHub for this page.

## Files

```text
projects-app/
├── package.json
├── .gitignore
├── .github/workflows/
│   ├── e2e.yml
│   └── visual-baselines.yml
├── api/server.ts
├── dist/index.html
├── dist/assets/index-Bx7f3k2a.js
└── e2e/
    ├── setup.ts
    ├── profiles.ts
    └── visual/
        ├── projects.test.ts
        └── __screenshots__/
            ├── darwin/      # recorded on macOS
            └── linux/       # recorded on ubuntu-24.04 in CI
```

`package.json`:

```json
{
  "name": "projects-app",
  "private": true,
  "type": "module",
  "scripts": {
    "api": "bun api/server.ts",
    "e2e": "pyro e2e e2e --setup e2e/setup.ts --timeout 30000",
    "e2e:ci": "pyro e2e e2e --setup e2e/setup.ts --timeout 30000 --junit junit.xml",
    "e2e:visual": "pyro e2e e2e/visual --setup e2e/setup.ts --timeout 30000"
  }
}
```

`e2e:visual` runs only the visual specs. `e2e` and `e2e:ci` run them with everything else.

```text
# .gitignore
node_modules
junit.xml
*.actual.png
*.diff.png
```

Commit the baselines (`.png` and the `.json` beside each) and ignore the `.actual.png` and `.diff.png` files a mismatch writes.

::: details api/server.ts (the REST API)
```ts
// api/server.ts: the REST API. In your project this is your backend.
type User = { email: string; name: string; role: 'admin' | 'member'; password: string };
type Project = { id: number; name: string; owner: string };

const users: User[] = [
  { email: 'ada@example.com', name: 'Ada Admin', role: 'admin', password: 'admin-pass' },
  { email: 'max@example.com', name: 'Max Member', role: 'member', password: 'member-pass' },
];
const sessions = new Map<string, User>();
const projects = new Map<number, Project>();
let nextId = 1;

export function startApi(port = 0) {
  return Bun.serve({
    port,
    async fetch(req) {
      const url = new URL(req.url);
      const path = url.pathname;
      if (path === '/api/health') return new Response('ok');
      if (path === '/api/login' && req.method === 'POST') {
        const { email, password } = await req.json();
        const user = users.find((u) => u.email === email && u.password === password);
        if (!user) return Response.json({ error: 'wrong email or password' }, { status: 401 });
        const token = crypto.randomUUID();
        sessions.set(token, user);
        return Response.json({ token, user: { name: user.name, role: user.role } });
      }
      const user = sessions.get(req.headers.get('authorization')?.replace(/^Bearer /, '') ?? '');
      if (!user) return Response.json({ error: 'signed out' }, { status: 401 });
      if (path === '/api/logout' && req.method === 'POST') {
        sessions.delete(req.headers.get('authorization')!.replace(/^Bearer /, ''));
        return new Response(null, { status: 204 });
      }
      if (path === '/api/projects' && req.method === 'GET') return Response.json([...projects.values()]);
      if (path === '/api/projects' && req.method === 'POST') {
        const { name } = await req.json();
        if (!name) return Response.json({ error: 'name is required' }, { status: 400 });
        const project = { id: nextId++, name, owner: user.email };
        projects.set(project.id, project);
        return Response.json(project, { status: 201 });
      }
      const m = /^\/api\/projects\/(\d+)$/.exec(path);
      if (m && req.method === 'DELETE') {
        return new Response(null, { status: projects.delete(Number(m[1])) ? 204 : 404 });
      }
      if (path === '/api/users' && req.method === 'GET') {
        if (user.role !== 'admin') return Response.json({ error: 'admins only' }, { status: 403 });
        return Response.json(users.map(({ email, name, role }) => ({ email, name, role })));
      }
      return Response.json({ error: 'not found' }, { status: 404 });
    },
  });
}

if (import.meta.main) console.log(`API on ${startApi(Number(process.env.PORT ?? 0)).url.origin}`);
```
:::

::: details dist/index.html
```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <title>Projects</title>
    <script type="module" crossorigin src="/assets/index-Bx7f3k2a.js"></script>
  </head>
  <body>
    <div id="root"></div>
  </body>
</html>
```
:::

::: details dist/assets/index-Bx7f3k2a.js (the built app)
```js
// dist/assets/index-Bx7f3k2a.js: stands in for the bundle `vite build` writes.
// It behaves like a small React app: a client-side router, data fetched when a
// route mounts, a session in localStorage.
const root = document.getElementById('root');
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
const session = () => JSON.parse(localStorage.getItem('session') ?? 'null');
const api = (path, init = {}) =>
  fetch(path, {
    ...init,
    headers: { 'content-type': 'application/json', authorization: `Bearer ${session()?.token ?? ''}` },
  });
const go = (path) => { history.pushState(null, '', path); render(); };

addEventListener('popstate', () => render());
document.addEventListener('click', (e) => {
  const a = e.target.closest('a[href^="/"]');
  if (a) { e.preventDefault(); go(a.getAttribute('href')); }
});

function render() {
  const s = session();
  if (!s) return login();
  root.innerHTML = `
    <header>
      <nav><a href="/projects">Projects</a> <a href="/users">Users</a></nav>
      <span>Signed in as ${esc(s.user.name)}</span> <button id="logout">Sign out</button>
    </header>
    <main></main>`;
  root.querySelector('#logout').onclick = async () => {
    await api('/api/logout', { method: 'POST' });
    localStorage.removeItem('session');
    go('/login');
  };
  const main = root.querySelector('main');
  if (location.pathname === '/users') return usersPage(main);
  if (location.pathname === '/projects') return projectsPage(main);
  main.innerHTML = `<h1>Welcome, ${esc(s.user.name)}</h1>`;
}

function login() {
  if (location.pathname !== '/login') history.replaceState(null, '', '/login');
  root.innerHTML = `
    <h1>Sign in</h1>
    <form>
      <label>Email <input name="email" type="email"></label>
      <label>Password <input name="password" type="password"></label>
      <button>Sign in</button>
      <p role="alert"></p>
    </form>`;
  const form = root.querySelector('form');
  form.onsubmit = async (e) => {
    e.preventDefault();
    const res = await fetch('/api/login', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ email: form.querySelector('[name=email]').value, password: form.querySelector('[name=password]').value }),
    });
    if (!res.ok) { form.querySelector('[role=alert]').textContent = 'Wrong email or password'; return; }
    localStorage.setItem('session', JSON.stringify(await res.json()));
    go('/projects');
  };
}

async function projectsPage(main, status = '') {
  main.innerHTML = `<h1>Projects</h1><p role="status">Loading projects…</p>`;
  const res = await api('/api/projects');
  if (!res.ok) {
    main.innerHTML = `<h1>Projects</h1>
      <div role="alert">Could not load projects (${res.status})</div><button id="retry">Retry</button>`;
    main.querySelector('#retry').onclick = () => projectsPage(main);
    return;
  }
  const list = await res.json();
  main.innerHTML = `
    <h1>Projects</h1>
    ${list.length === 0 ? '<p>No projects yet</p>' : `<ul>${list.map((p) => `
      <li data-testid="project-${p.id}">${esc(p.name)}
        <button aria-label="Delete ${esc(p.name)}" data-id="${p.id}">Delete</button></li>`).join('')}</ul>`}
    <form>
      <label>Project name <input name="name"></label>
      <button>Create</button>
    </form>
    <p role="status">${esc(status)}</p>`;
  for (const b of main.querySelectorAll('li button')) {
    b.onclick = async () => {
      await api(`/api/projects/${b.dataset.id}`, { method: 'DELETE' });
      projectsPage(main, 'Deleted');
    };
  }
  const form = main.querySelector('form');
  form.onsubmit = async (e) => {
    e.preventDefault();
    const button = form.querySelector('button');
    button.disabled = true;
    button.textContent = 'Saving…';
    const name = form.querySelector('[name=name]').value;
    const res = await api('/api/projects', { method: 'POST', body: JSON.stringify({ name }) });
    if (!res.ok) {
      button.disabled = false;
      button.textContent = 'Create';
      main.querySelector('[role=status]').textContent = `Could not create the project (${res.status})`;
      return;
    }
    projectsPage(main, `Created ${name}`);
  };
}

async function usersPage(main) {
  main.innerHTML = `<h1>Users</h1><p role="status">Loading users…</p>`;
  const res = await api('/api/users');
  if (res.status === 403) { main.innerHTML = `<h1>Users</h1><p role="alert">Only admins can see users</p>`; return; }
  const list = await res.json();
  main.innerHTML = `<h1>Users</h1><table>
    <tr><th>Name</th><th>Email</th><th>Role</th></tr>
    ${list.map((u) => `<tr><td>${esc(u.name)}</td><td>${esc(u.email)}</td><td>${u.role}</td></tr>`).join('')}
  </table>`;
}

render();
```
:::

::: details e2e/setup.ts
```ts
// e2e/setup.ts: runs once before any spec file loads (`pyro e2e --setup`).
import type { Setup } from '@povio/pyro/e2e';
import { startApi } from '../api/server';

type Session = { token: string; user: { name: string; role: string } };
export type Runtime = {
  origin: string;
  dist: string;
  sessions: { admin: Session; member: Session };
  stop: () => void;
};

async function signIn(origin: string, email: string, password: string): Promise<Session> {
  const res = await fetch(`${origin}/api/login`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ email, password }),
  });
  if (!res.ok) throw new Error(`sign-in as ${email} failed: ${res.status}`);
  return res.json();
}

export default {
  async setup() {
    const api = startApi();                     // a free port; the browser serves dist/ on the same origin
    const origin = api.url.origin;
    const admin = await signIn(origin, 'ada@example.com', 'admin-pass');
    const member = await signIn(origin, 'max@example.com', 'member-pass');
    // Data every test may read but none changes.
    await fetch(`${origin}/api/projects`, {
      method: 'POST',
      headers: { authorization: `Bearer ${admin.token}` },
      body: JSON.stringify({ name: 'Handbook' }),
    });
    return {
      origin,
      dist: new URL('../dist', import.meta.url).pathname,
      sessions: { admin, member },
      stop: () => api.stop(true),
    };
  },
  browser: (rt) => ({ args: ['--static', `${rt.origin}=${rt.dist}`] }),
  teardown: (rt) => rt.stop(),
} satisfies Setup<Runtime>;
```
:::

::: details e2e/profiles.ts
```ts
// e2e/profiles.ts: one profile per role, registered once per run.
import { browser, setupResult } from '@povio/pyro/e2e';
import type { Runtime } from './setup';

export type Role = 'admin' | 'member';
export const rt = () => setupResult<Runtime>();

let registered: Promise<void> | undefined;
function profiles() {
  // Every spec file shares one module graph: register the profiles once, on first use.
  return (registered ??= (async () => {
    const pyro = await browser();
    const { origin, sessions } = rt();
    for (const role of ['admin', 'member'] as const) {
      await pyro.profile(role, {
        url: `${origin}/`,
        localStorage: { session: JSON.stringify(sessions[role]) },
        viewport: { width: 1280, height: 800 },
      });
    }
  })());
}

/** A booted, signed-in page for `role`, moved to `route` without reloading the app. */
export async function pageAs(role: Role, route: string) {
  await profiles();
  return (await browser()).page({ as: role, route });
}
```
:::

## The visual spec

```ts
// e2e/visual/projects.test.ts
import { join } from 'node:path';
import { test } from '@povio/pyro/e2e';
import { check, css, matchScreenshot, mock, role, route, text } from '@povio/pyro';
import { pageAs } from '../profiles';

// System fonts differ between macOS and Linux: one baseline directory per platform.
const dir = join(import.meta.dir, '__screenshots__', process.platform);

// Fixed data, so the pixels depend only on the code under test.
const projects = [
  { id: 1, name: 'Handbook', owner: 'ada@example.com' },
  { id: 2, name: 'Roadmap', owner: 'max@example.com' },
];

test('projects list', async () => {
  await using page = await pageAs('member', '/');
  await page.run([
    mock('/api/projects', { method: 'GET', json: projects }),
    route('/projects'),
    check(text('Roadmap')).visible(),
    matchScreenshot('projects-list', { dir }),
    css('form').matchScreenshot('create-form', { dir }),   // one element's box
  ]);
});

test('projects error banner', async () => {
  await using page = await pageAs('member', '/');
  await page.run([
    mock('/api/projects', { method: 'GET', status: 503 }),
    route('/projects'),
    check(role('alert')).visible(),
    matchScreenshot('projects-error', { dir }),
  ]);
});
```

- **Mocked data.** The list comes from a `mock`, not from the shared API, where other tests add and delete projects at the same time. Mock everything a screenshot shows that can vary. See [Mocking backend states](/recipes/e2e-mock-backend-states) for why the mock comes before the `route`.
- **A check before the screenshot.** `check(text('Roadmap')).visible()` waits until the list has rendered. The screenshot step itself does not wait for content.
- **Baselines per OS.** Text in system fonts renders differently on macOS and Linux. `process.platform` (`darwin`, `linux`) picks the directory. Pages that load all their fonts as web fonts can share one set.
- **An element screenshot.** `css('form').matchScreenshot(...)` compares only that element's box, so a change elsewhere on the page does not fail it.

## Recording baselines

The first run has no baselines. Each test writes the first missing baseline it reaches, and fails:

```text
$ bun run e2e:visual
$ pyro e2e e2e/visual --setup e2e/setup.ts --timeout 30000
pyro: serving http://localhost:58745 from /path/to/projects-app/dist
pyro: CDP server listening on ws://127.0.0.1:58748/ (fork mode)
✗ e2e/visual/projects.test.ts > projects error banner [186.73ms]
    Error: screenshot 'projects-error': the baseline did not exist; wrote /path/to/projects-app/e2e/visual/__screenshots__/darwin/projects-error.png
✗ e2e/visual/projects.test.ts > projects list [190.07ms]
    Error: screenshot 'projects-list': the baseline did not exist; wrote /path/to/projects-app/e2e/visual/__screenshots__/darwin/projects-list.png
...
 0 pass
 2 fail
```

`projects list` stopped at its first screenshot, so `create-form` was not written. Record them all at once with `-u`, which writes every baseline the run reaches instead of comparing:

```text
$ bun run e2e:visual -u
$ pyro e2e e2e/visual --setup e2e/setup.ts --timeout 30000 -u
pyro: serving http://localhost:58757 from /path/to/projects-app/dist
pyro: CDP server listening on ws://127.0.0.1:58760/ (fork mode)
✓ e2e/visual/projects.test.ts > projects error banner [134.45ms]
✓ e2e/visual/projects.test.ts > projects list [140.89ms]

 2 pass
Ran 2 tests across 1 file. [153.86ms]
```

```text
e2e/visual/__screenshots__/darwin/create-form.json
e2e/visual/__screenshots__/darwin/create-form.png
e2e/visual/__screenshots__/darwin/projects-error.json
e2e/visual/__screenshots__/darwin/projects-error.png
e2e/visual/__screenshots__/darwin/projects-list.json
e2e/visual/__screenshots__/darwin/projects-list.png
```

Open the PNGs, check they show what you meant, and commit them.

## When a screenshot does not match

Rename the form's button from `Create` to `Add project` in the app and run again:

```text
$ bun run e2e:visual
$ pyro e2e e2e/visual --setup e2e/setup.ts --timeout 30000
pyro: serving http://localhost:58808 from /path/to/projects-app/dist
pyro: CDP server listening on ws://127.0.0.1:58811/ (fork mode)
✓ e2e/visual/projects.test.ts > projects error banner [58.26ms]
✗ e2e/visual/projects.test.ts > projects list [72.02ms]
    Error: step 3 (matchScreenshot /path/to/projects-app/e2e/visual/__screenshots__/darwin/projects-list.png): matchScreenshot '/path/to/projects-app/e2e/visual/__screenshots__/darwin/projects-list.png': 953 pixels differ (1280x800); the actual image and the diff are in error.data
      actual: /path/to/projects-app/e2e/visual/__screenshots__/darwin/projects-list.actual.png
      diff:   /path/to/projects-app/e2e/visual/__screenshots__/darwin/projects-list.diff.png
...
 1 pass
 1 fail
error: script "e2e:visual" exited with code 1
```

`projects-list.actual.png` is the page now. `projects-list.diff.png` is the baseline faded, with the differing pixels in red: here, the button. Both are removed again the next time the step passes.

## Updating one test's baselines

When the change is intended, update only the tests it affects. Pass `-t` with the test's name through the script:

```text
$ bun run e2e:visual -u -t 'projects list'
$ pyro e2e e2e/visual --setup e2e/setup.ts --timeout 30000 -u -t "projects list"
pyro: serving http://localhost:58817 from /path/to/projects-app/dist
pyro: CDP server listening on ws://127.0.0.1:58820/ (fork mode)
✓ e2e/visual/projects.test.ts > projects list [73.03ms]

 1 pass
 1 filtered out
Ran 1 test across 1 file. [89.16ms]
```

This rewrote `projects-list.png` and `create-form.png` (both screenshots of that test) and removed the `.actual` and `.diff` files. `projects-error.png` was left alone. Review the changed PNGs in your diff tool, then commit.

Run it through `bun run`, not `bunx pyro e2e ... -t 'projects list'`: `bunx` splits the quoted name at the space (Bun 1.4.3), and the run finds no spec files.

On macOS, update the `darwin` baselines locally. The `linux` ones come from CI, below.

## CI: compare, and upload the images on a mismatch

```yaml
# .github/workflows/e2e.yml
name: E2E

on:
  pull_request:
  push:
    branches: [main]

jobs:
  e2e:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-24.04, macos-15]
    runs-on: ${{ matrix.os }}
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v6
      - uses: oven-sh/setup-bun@v2
      - run: bun install --frozen-lockfile
      # - run: bun run build      # your `vite build`, if dist/ is not committed
      - run: bun run e2e:ci
      - uses: actions/upload-artifact@v4
        if: always()
        with:
          name: junit-${{ matrix.os }}
          path: junit.xml
      - uses: actions/upload-artifact@v4
        if: failure()
        with:
          name: screenshot-diffs-${{ matrix.os }}
          path: |
            e2e/**/*.actual.png
            e2e/**/*.diff.png
          if-no-files-found: ignore
```

The job runs on both operating systems, each against its own baseline directory. When any test fails, the last step uploads every `.actual.png` and `.diff.png`. Download the `screenshot-diffs-<os>` artifact from the run's summary page to see what changed.

## CI: recording the Linux baselines

Record Linux baselines on the runner image CI compares on, not on your machine:

```yaml
# .github/workflows/visual-baselines.yml
name: Record Linux visual baselines

on:
  workflow_dispatch:

jobs:
  record:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v6
      - uses: oven-sh/setup-bun@v2
      - run: bun install --frozen-lockfile
      # - run: bun run build      # your `vite build`, if dist/ is not committed
      - run: bun run e2e:visual -u
      - uses: actions/upload-artifact@v4
        with:
          name: linux-baselines
          path: e2e/visual/__screenshots__/linux
```

Run it from the Actions tab (or `gh workflow run visual-baselines.yml --ref <your-branch>`), download the artifact into the baseline directory, review, and commit:

```sh
gh run download <run-id> --name linux-baselines --dir e2e/visual/__screenshots__/linux
git add e2e/visual/__screenshots__/linux
```

Until `linux/` has baselines, the Linux job fails each visual test with `the baseline did not exist`.

## Next

- [Visual tests](/e2e/screenshots): every option (`threshold`, `maxDiffPixels`, `clip`, `fullPage`), what is and is not captured, and the display-list hash.
- [CI recipes](/e2e/ci): caching, sharding and JUnit reports for the same job.
