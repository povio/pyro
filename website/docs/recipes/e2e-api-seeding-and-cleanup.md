# API seeding and cleanup

A fast test does not click its way to the state it checks. It creates that state through your API, takes a signed-in page, runs the scenario in one `page.run([...])`, and removes its data afterwards. This recipe shows the pieces:

- seed helpers that call the API with a role's token,
- unique data per test, because tests of every file run at the same time against one backend,
- cleanup with `defer`, which runs after the test whether it passed or failed,
- a teardown that fails the run when a test left data behind,
- `check.request` on the request body, headers and count, in the same run as the click that sends it.

It uses the app, API and profiles of [Project layout](/recipes/e2e-project-layout). Every file is shown; it was run as shown with Bun 1.4.3.

## Files

```text
projects-app/
├── package.json
├── api/server.ts
├── dist/index.html
├── dist/assets/index-Bx7f3k2a.js
└── e2e/
    ├── setup.ts               # signs in each role, seeds shared data, checks for leaks
    ├── profiles.ts            # one profile per role
    ├── api.ts                 # seed and cleanup helpers
    └── projects-api.test.ts
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

::: details e2e/profiles.ts (one profile per role)
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

## Shared data once, per-test data in the test

Data that every test reads and no test changes goes in `setup()`: here, one project called `Handbook`. Everything else is created by the test that needs it.

The teardown lists what is left. A project other than `Handbook` means a test forgot its cleanup, and the run fails with a hook failure that names it.

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
  async teardown(rt) {
    // Every test cleans up after itself: anything left but the shared seed is a leak.
    const res = await fetch(`${rt.origin}/api/projects`, {
      headers: { authorization: `Bearer ${rt.sessions.admin.token}` },
    });
    const left = (await res.json()).filter((p: { name: string }) => p.name !== 'Handbook');
    rt.stop();
    if (left.length) throw new Error(`tests left ${left.length} projects behind: ${left.map((p: { name: string }) => p.name).join(', ')}`);
  },
} satisfies Setup<Runtime>;
```

## The helpers

```ts
// e2e/api.ts: test data through the REST API, never through the UI.
import { defer, testInfo } from '@povio/pyro/e2e';
import { rt, type Role } from './profiles';

async function call(role: Role, method: string, path: string, body?: unknown) {
  const res = await fetch(`${rt().origin}${path}`, {
    method,
    headers: { 'content-type': 'application/json', authorization: `Bearer ${rt().sessions[role].token}` },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`${method} ${path}: ${res.status} ${await res.text()}`);
  return res.status === 204 ? undefined : res.json();
}

/** A name no other test uses: tests run at the same time against one API. */
export const unique = (what: string) => `${what} ${crypto.randomUUID().slice(0, 8)}`;

/** Creates a project and deletes it after the test, pass or fail. */
export async function seedProject(role: Role, name = unique(testInfo()?.name ?? 'project')) {
  const project: { id: number; name: string } = await call(role, 'POST', '/api/projects', { name });
  defer(() => deleteProject(project.id));
  return project;
}

export async function deleteProject(id: number) {
  await fetch(`${rt().origin}/api/projects/${id}`, {
    method: 'DELETE',
    headers: { authorization: `Bearer ${rt().sessions.admin.token}` },
  });                                           // 404 is fine: the test deleted it already
}

export async function findProject(name: string) {
  const list: { id: number; name: string }[] = await call('admin', 'GET', '/api/projects');
  return list.find((p) => p.name === name);
}
```

- **`unique()`** gives each test its own name. Two tests that both create `Roadmap` at the same time would see each other's rows, and a check such as `text('Roadmap')` would fail strict mode.
- **`seedProject()`** calls `defer()` right after the create, before the test does anything else. `defer` from `@povio/pyro/e2e` runs after the test ends, also when it failed or timed out, and later calls run first.
- **The cleanup tolerates 404.** A test may have deleted the record itself, as `deletes a seeded project` does.
- **The default name is the test's name** (`testInfo()?.name`), so a leaked record says which test made it.

## The tests

```ts
// e2e/projects-api.test.ts
import { defer, expect, test } from '@povio/pyro/e2e';
import { button, check, label, testId, text } from '@povio/pyro';
import { pageAs } from './profiles';
import { deleteProject, findProject, seedProject, unique } from './api';

test('the create form posts exactly the name', async () => {
  const name = unique('Budget');
  defer(async () => {
    const created = await findProject(name);
    if (created) await deleteProject(created.id);
  });
  await using page = await pageAs('member', '/projects');
  const r = await page.run([
    label('Project name').fill(name),
    button('Create').click(),
    check.request('POST', '/api/projects')
      .status(201)
      .body(`"name":"${name}"`)                  // a substring of the raw body
      .bodyKeys(['name'])                        // and no other top-level key
      .headers({ authorization: /^Bearer [0-9a-f-]{36}$/ })
      .count(1),                                 // one click, one request
    check(text(`Created ${name}`)).visible(),
  ]);
  const post = r.requests.find((q) => q.method === 'POST') as { postData?: string } | undefined;
  expect(JSON.parse(post?.postData ?? '{}')).toEqual({ name });
});

test('lists projects seeded for this test', async () => {
  const [a, b] = await Promise.all([seedProject('admin'), seedProject('admin')]);
  await using page = await pageAs('member', '/projects');
  await page.run([
    check(testId(`project-${a.id}`)).containsText(a.name),
    check(testId(`project-${b.id}`)).containsText(b.name),
  ]);
});

test('deletes a seeded project', async () => {
  const project = await seedProject('admin');
  await using page = await pageAs('admin', '/projects');
  await page.run([
    button(`Delete ${project.name}`).click(),
    check.request('DELETE', `/api/projects/${project.id}`).status(204),
    check(testId(`project-${project.id}`)).count(0),
  ]);
  expect(await findProject(project.name)).toBeUndefined();
});
```

The first test creates its project through the UI, so there is no id to clean up yet. It registers a `defer` that finds the project by its unique name instead.

Its request check asserts what the form sent, in the browser, while the run is still going:

| Modifier | Asserts |
|---|---|
| `.status(201)` | The response status |
| `.body('"name":"..."')` | The raw request body contains this text (or matches a `RegExp`) |
| `.bodyKeys(['name'])` | The body is a JSON object with exactly these top-level keys: nothing extra was sent |
| `.headers({ authorization: /.../ })` | The header is present and matches, against its real value |
| `.count(1)` | Exactly one matching request: no double submit |

For a structural comparison, parse the body from the run's request log, `r.requests`, after the run, as the last line of that test does. `postData` is present at run time but not declared in the `RunResult` type, hence the cast. [Requests and mocks](/e2e/network#checking-requests) lists every modifier.

## Run it

```text
$ bun run e2e
$ pyro e2e e2e --setup e2e/setup.ts --timeout 30000
pyro: serving http://localhost:58223 from /path/to/projects-app/dist
pyro: CDP server listening on ws://127.0.0.1:58230/ (fork mode)
✓ e2e/projects-api.test.ts > lists projects seeded for this test [56.43ms]
✓ e2e/projects-api.test.ts > the create form posts exactly the name [61.83ms]
✓ e2e/projects-api.test.ts > deletes a seeded project [63.92ms]

 3 pass
Ran 3 tests across 1 file. [77.87ms]
```

## A wrong body and a forgotten cleanup

Remove the `defer` from the first test and change `.bodyKeys(['name'])` to `.bodyKeys(['name', 'owner'])`:

```text
$ bunx pyro e2e e2e --setup e2e/setup.ts --timeout 30000
pyro: CDP server listening on ws://127.0.0.1:50282/ (fork mode)
✓ e2e/projects-api.test.ts > lists projects seeded for this test [51.63ms]
✓ e2e/projects-api.test.ts > deletes a seeded project [58.28ms]
✗ e2e/projects-api.test.ts > the create form posts exactly the name [5.06s]
    Error: step 2 (request {"body":"\"name\":\"Budget 5befe355\"","bodyKeys":["name","owner"],"count":1,"headers":{"authorization":"[REDACTED]"},"method":"POST","path":"/api/projects","status":201}): timed out after 5000 ms: request {"body":"\"name\":\"Budget 5befe355\"","bodyKeys":["name","owner"],"count":1,"headers":{"authorization":"[REDACTED]"},"method":"POST","path":"/api/projects","status":201}: 0 matching requests (0 finished), want 1
✗ e2e/setup.ts > (teardown)
    Error: tests left 1 projects behind: Budget 5befe355

 2 pass
 1 fail
 1 hook failure
Ran 3 tests across 1 file. [5.07s]
```

(The `pyro: serving` line and the stack lines are left out.)

- A request check with several conditions counts only requests that meet all of them. A body that does not match shows up as `0 matching requests`, not as a diff. Look at `r.requests` (or the API's own log) to see what was sent; [Debugging failures](/e2e/debugging#reading-results-and-requests) shows how to print it.
- The `authorization` matcher prints as `[REDACTED]`: sensitive header values are hidden in messages, and the check still compares the real value.
- The teardown names the leaked project. Its name came from `unique('Budget')`, so you know which test made it.

## When your API has no delete endpoint

- Give every test's data a unique prefix and delete by prefix in `teardown`, or reset the database there.
- Or create a tenant, workspace or user per test, and scope all its data to it, so leftovers never meet another test.
- Do not depend on order instead: `pyro e2e` runs tests of all files at once.

## Next

- [Mocking backend states](/recipes/e2e-mock-backend-states): states your API will not produce on demand, such as a 500 or a slow response.
- [Signing in once](/recipes/e2e-login-once): what else tests share in fork mode.
- [Requests and mocks](/e2e/network): every `check.request` modifier and failure reason.
