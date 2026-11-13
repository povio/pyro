# Mocking backend states

Some screens matter most when the backend misbehaves: an error banner, an empty list, a spinner, a disabled button while saving. A real API produces those states rarely and never on cue. A `mock` step answers requests from inside the browser instead, so each state is one deterministic test with no sleeps and no test-only backend switches.

This recipe tests six states of the projects screen from [Project layout](/recipes/e2e-project-layout):

| State | Mock option |
|---|---|
| Error banner, then recovery on Retry | `status: 500`, `times: 1` |
| Empty list | `json: []` |
| Spinner while loading | `hold` and `release` |
| A slow API | `delay` |
| Disabled button while saving | `hold` on the `POST` |
| Server error on save | `status: 500` on the `POST` |

Every file is shown; it was run as shown with Bun 1.4.3.

## Files

```text
projects-app/
├── package.json
├── api/server.ts
├── dist/index.html
├── dist/assets/index-Bx7f3k2a.js
└── e2e/
    ├── setup.ts
    ├── profiles.ts
    └── backend-states.test.ts
```

::: details package.json
```json
{
  "name": "projects-app",
  "private": true,
  "type": "module",
  "scripts": {
    "api": "bun api/server.ts",
    "e2e": "pyro e2e e2e --setup e2e/setup.ts --timeout 30000",
    "e2e:ci": "pyro e2e e2e --setup e2e/setup.ts --timeout 30000 --junit junit.xml"
  }
}
```
:::

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

## Mock before the request

A mock answers requests made after its step. A page from a profile booted before the test, so whatever the app fetched while booting has already been fetched. The profiles here boot on `/`, a welcome screen that fetches nothing. Each test registers its mocks first, then routes to `/projects`. The route change renders the screen, the screen fetches the list, and the mock answers.

If your app's boot screen already fetches what you want to mock, boot the profile on another route, or see [Mocks](/e2e/network#mocks-across-the-stack) for mocking before a full navigation.

## The tests

```ts
// e2e/backend-states.test.ts
import { describe, expect, test } from '@povio/pyro/e2e';
import { button, check, label, mock, release, role, route, text } from '@povio/pyro';
import { pageAs } from './profiles';

// The page boots at `/`; each test registers its mocks, then routes to /projects,
// so the list request the route starts already meets them.
const page = () => pageAs('member', '/');

describe('projects list', () => {
  test('shows an error banner, then recovers on Retry', async () => {
    await using p = await page();
    await p.run([
      mock('/api/projects', { method: 'GET', status: 500, json: { error: 'down' }, times: 1 }),
      route('/projects'),
      check(role('alert')).text('Could not load projects (500)'),
      button('Retry').click(),                   // the mock is used up: this request reaches the API
      check(label('Project name')).visible(),
      check(role('alert')).hidden(),
    ]);
  });

  test('shows the empty state', async () => {
    await using p = await page();
    await p.run([
      mock('/api/projects', { method: 'GET', json: [] }),
      route('/projects'),
      check(text('No projects yet')).visible(),
    ]);
  });

  test('shows a spinner while the list loads', async () => {
    await using p = await page();
    await p.run([
      mock('/api/projects', { method: 'GET', hold: 'list', json: [{ id: 1, name: 'Held', owner: 'x' }] }),
      route('/projects', { settle: false }),     // its request is parked: do not wait for it
      check(text('Loading projects…')).visible(),
      release('list'),
      check(text('Held')).visible(),
      check(text('Loading projects…')).hidden(),
    ]);
  });

  test('waits out a slow API without any sleep', async () => {
    await using p = await page();
    const r = await p.run([
      mock('/api/projects', { method: 'GET', delay: 400, json: [{ id: 1, name: 'Slow', owner: 'x' }] }),
      route('/projects'),                        // settles once the delayed response arrived
      check(text('Slow')).visible(),
    ]);
    expect((r.results[1] as { settleMs: number }).settleMs).toBeGreaterThan(400);
  });
});

describe('create form', () => {
  test('disables Create while saving', async () => {
    await using p = await page();
    await p.run([
      mock('/api/projects', { method: 'POST', hold: 'save', status: 201, json: { id: 9999, name: 'Draft', owner: 'x' } }),
      route('/projects'),
      label('Project name').fill('Draft'),
      button('Create').click({ settle: false }),
      check(button('Saving…')).disabled(),
      release('save'),
      check(text('Created Draft')).visible(),
      check(button('Create')).enabled(),
    ]);
  });

  test('shows the server error and lets the user try again', async () => {
    await using p = await page();
    await p.run([
      mock('/api/projects', { method: 'POST', status: 500, json: { error: 'database is down' } }),
      route('/projects'),
      label('Project name').fill('Doomed'),
      button('Create').click(),
      check.request('POST', '/api/projects').status(500),
      check(text('Could not create the project (500)')).visible(),
      check(button('Create')).enabled(),
      check(label('Project name')).value('Doomed'),
    ]);
  });
});
```

What each one relies on:

- **`times: 1`** answers the first matching request and then removes the rule, so the Retry click reaches the real API. Without `times` the rule stays for the life of the page.
- **`method: 'GET'`** keeps the list mock from answering the create form's `POST` to the same URL.
- **`hold`** parks the request until `release('list')`. Between the two, the in-between state stays on screen for as long as the checks need. The step that starts a held request takes `{ settle: false }`; otherwise it waits up to 1 s for the request and carries on with `settled: false`.
- **`delay`** is a real delay. The route step settles only once the delayed response has arrived, so the next check needs no sleep. The test asserts that through the route step's `settleMs`. Use `hold` when you want to check the state during the wait, `delay` when you only want a slow backend.
- **A mock lives on the page's browser context.** Each page from `pyro.page()` has its own, so mocks never leak into another test.

## Run it

```text
$ bun run e2e
$ pyro e2e e2e --setup e2e/setup.ts --timeout 30000
pyro: serving http://localhost:50697 from /path/to/projects-app/dist
pyro: CDP server listening on ws://127.0.0.1:50700/ (fork mode)
✓ e2e/backend-states.test.ts > projects list > shows an error banner, then recovers on Retry [80.95ms]
✓ e2e/backend-states.test.ts > projects list > shows the empty state [82.28ms]
✓ e2e/backend-states.test.ts > projects list > shows a spinner while the list loads [85.58ms]
✓ e2e/backend-states.test.ts > create form > shows the server error and lets the user try again [101.73ms]
✓ e2e/backend-states.test.ts > create form > disables Create while saving [110.24ms]
✓ e2e/backend-states.test.ts > projects list > waits out a slow API without any sleep [498.46ms]

 6 pass
Ran 6 tests across 1 file. [521.15ms]
```

The slow-API test includes its 400 ms delay.

## A mock registered too late

Taking the page with `route: '/projects'` and mocking afterwards misses the request, which the route already sent:

```ts
await using p = await pageAs('member', '/projects');
await p.run([mock('/api/projects', { method: 'GET', json: [] }), check(text('No projects yet')).visible({ timeout: 1000 })]);
```

```text
✗ e2e/x.test.ts > mock after the route (wrong) [1.05s]
    RunError: step 1 (expect Visible {"text":"No projects yet"}): timed out after 1000 ms: expect Visible {"text":"No projects yet"}: expected Visible null, got false
```

The screen shows the real list, so the empty-state text never appears. Put the `mock` first and the `route` after it.

## Next

- [Requests and mocks](/e2e/network): every mock option, `patch` to rewrite a real response, and the errors for contradictory options.
- [Visual regression in CI](/recipes/e2e-visual-regression-ci): mocked data keeps screenshots stable.
- [API seeding and cleanup](/recipes/e2e-api-seeding-and-cleanup): real data when you do not want to mock.
