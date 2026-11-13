# Project layout for a new E2E suite

This recipe is a complete small project: a single-page app built into `dist/`, a REST API with sign-in, a setup module that starts the API and signs in two roles, one profile per role, shared API helpers, and specs for two features. `pyro e2e` runs it in one process with one browser, and writes a JUnit report for CI.

Every file is shown below. The project was run as shown with Bun 1.4.3.

## The tree

```text
projects-app/
├── package.json
├── .gitignore
├── api/
│   └── server.ts          # the REST API (your backend)
├── dist/                  # the front-end build (`vite build` output)
│   ├── index.html
│   └── assets/
│       └── index-Bx7f3k2a.js
└── e2e/
    ├── setup.ts           # once per run: start the API, sign in, seed shared data
    ├── profiles.ts        # one profile per role, registered once
    ├── api.ts             # seed helpers: unique data, cleanup with defer
    ├── projects.test.ts   # feature: projects
    └── users.test.ts      # feature: users (admin only)
```

Where things go:

| Folder or file | Holds | Runs |
|---|---|---|
| `e2e/setup.ts` | Starting servers, signing in each role through the API, data every test reads | Once, before any spec loads |
| `e2e/profiles.ts` | One `pyro.profile()` per role, behind a memoized promise | Once, on the first test that asks for a page |
| `e2e/api.ts` | Functions that create and delete test data through your API | In tests |
| `e2e/<feature>.test.ts` | One file per feature, a `describe` per screen | In one shared queue |

## The app

The app has a sign-in screen, a projects list where anyone can create a project and admins delete them, and a users page only admins may open. It keeps its session in `localStorage` under `session`, as `{ token, user }`.

In your project `dist/` is the output of `vite build` and `api/` is your backend. Here both are small enough to show. The bundle is plain JavaScript that behaves like a React app: a client-side router, data fetched when a route renders, the session read from `localStorage`.

::: details api/server.ts
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

::: details dist/assets/index-Bx7f3k2a.js
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

## package.json and .gitignore

`package.json`:

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

Install the package; this adds it to `devDependencies`:

```sh
bun add --dev @povio/pyro
```

`@povio/pyro` is not on npm yet: until its first release, see [Before the first release](/guide/installation#try-today).

The scripts call `pyro` by name, so they use the binary from `node_modules/.bin`. `--timeout 30000` is the default, written out: it gives each test more time than its steps (5 s each by default), so a failing step reports its reason instead of the test timing out first.

```text
# .gitignore
node_modules
junit.xml
*.actual.png
*.diff.png
```

## The setup module

`setup()` runs once, before any spec file loads. It starts the API on a free port, signs in each role through the API (not the UI), and creates data every test may read. Its return value is what specs get from `setupResult()`. `browser()` hands the browser its flags: `--static` serves `dist/` on the API's origin, so the app and `/api/...` share one origin and no front-end server runs. `teardown()` stops the API, also when the run is interrupted.

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

## One profile per role

A profile is a kind of page tests ask for by name. Each role's profile boots the app with that role's session in `localStorage`, so every page a test takes is already signed in. The run's browser is in fork mode, so each profile boots once and every test gets a fork of it.

All spec files share one module graph in `pyro e2e`, so `profiles()` registers both profiles once per run, on first use. Specs call `pageAs(role, route)`. `route` changes the URL in place, as a link would, so the app does not boot again and the screen fetches its data when it renders.

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

## Seed helpers

Tests create their data through the API with names no other test uses, because tests of every file run at the same time against one API. `seedProject()` registers its own cleanup with `defer`, so the project is deleted after the test whether it passed or failed. [API seeding and cleanup](/recipes/e2e-api-seeding-and-cleanup) goes into this in depth.

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

## The specs

Each test takes a page for one role, runs one `page.run([...])`, and asserts inside it.

```ts
// e2e/projects.test.ts
import { defer, describe, expect, test } from '@povio/pyro/e2e';
import { button, check, label, testId, text } from '@povio/pyro';
import { pageAs } from './profiles';
import { deleteProject, findProject, seedProject, unique } from './api';

describe('projects', () => {
  test('a member creates a project', async () => {
    const name = unique('Roadmap');
    defer(async () => {                         // the UI created it: find it and delete it
      const created = await findProject(name);
      if (created) await deleteProject(created.id);
    });
    await using page = await pageAs('member', '/projects');
    const r = await page.run([
      label('Project name').fill(name),
      button('Create').click(),
      check.request('POST', '/api/projects').status(201),
      check(text(`Created ${name}`)).visible(),
      text(`Created ${name}`).read('status'),
    ]);
    expect(r.values.status).toBe(`Created ${name}`);
  });

  test('an admin deletes a seeded project', async () => {
    const project = await seedProject('admin');
    await using page = await pageAs('admin', '/projects');
    await page.run([
      check(testId(`project-${project.id}`)).visible(),
      button(`Delete ${project.name}`).click(),
      check.request('DELETE', `/api/projects/${project.id}`).status(204),
      check(testId(`project-${project.id}`)).count(0),
    ]);
  });
});
```

```ts
// e2e/users.test.ts
import { describe, test } from '@povio/pyro/e2e';
import { check, heading, row, text } from '@povio/pyro';
import { pageAs } from './profiles';

describe('users', () => {
  test('an admin sees every user', async () => {
    await using page = await pageAs('admin', '/users');
    await page.run([
      check(heading('Users')).visible(),
      check(row('Max Member')).containsText('member'),
      check(row()).count(3),                    // the header row and two users
    ]);
  });

  test('a member is refused', async () => {
    await using page = await pageAs('member', '/users');
    await page.run([
      check.request('GET', '/api/users').status(403),
      check(text('Only admins can see users')).visible(),
    ]);
  });
});
```

## Run it

```text
$ bun run e2e:ci
$ pyro e2e e2e --setup e2e/setup.ts --timeout 30000 --junit junit.xml
pyro: serving http://localhost:65530 from /path/to/projects-app/dist
pyro: CDP server listening on ws://127.0.0.1:65533/ (fork mode)
✓ e2e/users.test.ts > users > an admin sees every user [57.99ms]
✓ e2e/users.test.ts > users > a member is refused [59.79ms]
✓ e2e/projects.test.ts > projects > a member creates a project [62.50ms]
✓ e2e/projects.test.ts > projects > an admin deletes a seeded project [63.81ms]

 4 pass
Ran 4 tests across 2 files. [75.37ms]
```

The first line of output is from `--static`: the browser answers every request for the app's origin from `dist/`, except `/api/...`, which no file matches and which therefore reaches the API. `junit.xml`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="pyro e2e" tests="4" failures="0" skipped="0" time="0.075">
  <testsuite name="e2e/projects.test.ts" file="e2e/projects.test.ts" tests="2" failures="0" skipped="0" time="0.126" hostname="...">
    <testcase name="projects &gt; a member creates a project" classname="e2e/projects.test.ts" time="0.063" />
    <testcase name="projects &gt; an admin deletes a seeded project" classname="e2e/projects.test.ts" time="0.064" />
  </testsuite>
  <testsuite name="e2e/users.test.ts" file="e2e/users.test.ts" tests="2" failures="0" skipped="0" time="0.118" hostname="...">
    <testcase name="users &gt; an admin sees every user" classname="e2e/users.test.ts" time="0.058" />
    <testcase name="users &gt; a member is refused" classname="e2e/users.test.ts" time="0.060" />
  </testsuite>
</testsuites>
```

## When a test fails

Break two checks in `users.test.ts` (expect 4 rows; expect the text `Only admins may see users`) and run again:

```text
$ bunx pyro e2e e2e --setup e2e/setup.ts --timeout 30000 --junit junit.xml
pyro: serving http://localhost:49300 from /path/to/projects-app/dist
pyro: CDP server listening on ws://127.0.0.1:49303/ (fork mode)
✓ e2e/projects.test.ts > projects > a member creates a project [64.26ms]
✓ e2e/projects.test.ts > projects > an admin deletes a seeded project [66.79ms]
✗ e2e/users.test.ts > users > a member is refused [5.06s]
    RunError: step 1 (expect Visible {"text":"Only admins may see users"}): timed out after 5000 ms: expect Visible {"text":"Only admins may see users"}: expected Visible null, got false
        at run (.../node_modules/@povio/pyro/src/index.ts:517:17)
        at async <anonymous> (/path/to/projects-app/e2e/users.test.ts:18:16)
✗ e2e/users.test.ts > users > an admin sees every user [5.06s]
    RunError: step 2 (expect Count {"role":"row"}): timed out after 5000 ms: expect Count {"role":"row"}: expected Count 4, got 3
        at run (.../node_modules/@povio/pyro/src/index.ts:517:17)
        at async <anonymous> (/path/to/projects-app/e2e/users.test.ts:9:16)

2 failures:
  ✗ e2e/users.test.ts > users > a member is refused: step 1 (expect Visible {"text":"Only admins may see users"}): timed out after 5000 ms: expect Visible {"text":"Only admins may see users"}: expected Visible null, got false
  ✗ e2e/users.test.ts > users > an admin sees every user: step 2 (expect Count {"role":"row"}): timed out after 5000 ms: expect Count {"role":"row"}: expected Count 4, got 3

 2 pass
 2 fail
Ran 4 tests across 2 files. [5.08s]
```

The exit code is 1, and `junit.xml` carries each message in a `<failure>` element. `step 1` is the index in that test's `page.run` array. The text after the last colon is what the browser saw: no visible match for the first, 3 rows instead of 4 for the second. [Debugging failures](/e2e/debugging) lists every reason.

With `--timeout 5000`, both tests would show only `TimeoutError: test timed out after 5000 ms`, because the test's timeout would equal the step's.

## Adapting it to your project

- **The build.** Run `vite build` before the tests and point `--static` at its `outDir`. Keep `dist` out of the test code: `setup()` returns its path.
- **The API.** Start your real backend in `setup()` instead of `startApi()`, for example with `Bun.spawn` and a health check (see [CI recipes](/e2e/ci#a-complete-workflow)).
- **Sign-in.** If your app keeps tokens in `localStorage`, sign in through the API in `setup()` as here. If it uses cookies, sign in through the UI once per role in a fork-mode `setup`: see [Signing in once](/recipes/e2e-login-once).
- **More roles.** Add a session in `setup()` and the role to the loop in `profiles.ts`.

## Next

- [Signing in once](/recipes/e2e-login-once): one snapshot per feature, and keeping tests independent in fork mode.
- [API seeding and cleanup](/recipes/e2e-api-seeding-and-cleanup): unique data per test and request body checks.
- [CI recipes](/e2e/ci): run `bun run e2e:ci` in GitHub Actions and upload `junit.xml`.
