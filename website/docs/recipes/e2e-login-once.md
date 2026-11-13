# Signing in once

Signing in is often the slowest part of a test. In fork mode you pay for it once per role: one profile boots the sign-in screen, one profile per role signs in through the real form in its `setup`, and every test forks a page that is already signed in. On top of that, each feature gets its own snapshot (`base`), so work a whole file repeats, such as opening a screen, also runs once.

Use this when your app's session cannot simply be written into `localStorage` before it boots: cookie sessions, a sign-in flow that sets up client state, or a token you cannot get from an API. When you can get the token from the API, putting it in the profile's `localStorage` is simpler: see [Project layout](/recipes/e2e-project-layout).

This recipe uses the same app and API as [Project layout](/recipes/e2e-project-layout). Every file is shown; it was run as shown with Bun 1.4.3 on macOS (Apple silicon), where fork mode is available.

## The chain

```text
signed-out            boots the app on /login, once
├── admin             setup: sign in as ada@example.com, once
└── member            setup: sign in as max@example.com, once
    └── member-projects   setup: route to /projects, wait for the list, once per run of the file
```

Each profile's page is forked from its base's page after the base's setup, and runs only its own setup. A test's page is a fork of the profile it asks for. It starts signed in, on the screen the setup left, with the data the app had fetched by then.

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
    ├── projects.test.ts
    └── auth.test.ts
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

The setup module only starts the API and serves the build. Nothing signs in here: the profiles do it in the browser.

```ts
// e2e/setup.ts
import type { Setup } from '@povio/pyro/e2e';
import { startApi } from '../api/server';

export type Runtime = { origin: string; dist: string; stop: () => void };

export default {
  setup() {
    const api = startApi();
    return { origin: api.url.origin, dist: new URL('../dist', import.meta.url).pathname, stop: () => api.stop(true) };
  },
  browser: (rt) => ({ args: ['--static', `${rt.origin}=${rt.dist}`] }),
  teardown: (rt) => rt.stop(),
} satisfies Setup<Runtime>;
```

## Sign in once per role

`signIn()` returns steps, so a profile's `setup` and a test can both use them. End every setup with a check of the state it should reach. A setup that fails makes `pyro.profile()` reject with a `ProfileSetupError` that names the step, and no page of that profile is handed out (see [Setup steps](/e2e/fork#setup-steps)).

```ts
// e2e/profiles.ts: boot once, sign in once per role, through the real sign-in form.
import { browser, setupResult } from '@povio/pyro/e2e';
import { button, check, label, text } from '@povio/pyro';
import type { Runtime } from './setup';

export const rt = () => setupResult<Runtime>();

export const signIn = (email: string, password: string) => [
  label('Email').fill(email),
  label('Password').fill(password),
  button('Sign in').click(),
  check(text('Signed in as')).visible(),        // end every setup with a check of where it should be
];

let roles: Promise<void> | undefined;
export function roleProfiles() {
  return (roles ??= (async () => {
    const pyro = await browser();
    const url = `${rt().origin}/login`;
    await pyro.profile('signed-out', { url, viewport: { width: 1280, height: 800 } });   // the app boots once
    await pyro.profile('admin', { base: 'signed-out', url, setup: signIn('ada@example.com', 'admin-pass') });
    await pyro.profile('member', { base: 'signed-out', url, setup: signIn('max@example.com', 'member-pass') });
  })());
}
```

The role profiles are shared by every file, so they are registered once per run behind a memoized promise. `pyro e2e` loads every spec file into one module graph.

## One snapshot per feature

Each file registers its own profile on top of a role in its `beforeAll`, and releases it with `defer` once the file's tests are done, so its template process exits.

```ts
// e2e/projects.test.ts
import { beforeAll, browser, defer, describe, test } from '@povio/pyro/e2e';
import { button, check, heading, label, route, testId, text } from '@povio/pyro';
import { roleProfiles, rt } from './profiles';

// Test data goes through the API with a token of its own, not the browser's session.
let token: Promise<string> | undefined;
const apiToken = () =>
  (token ??= fetch(`${rt().origin}/api/login`, {
    method: 'POST',
    body: JSON.stringify({ email: 'max@example.com', password: 'member-pass' }),
  }).then(async (r) => (await r.json()).token));

async function api(method: string, path: string, body?: unknown) {
  const res = await fetch(`${rt().origin}${path}`, {
    method,
    headers: { authorization: `Bearer ${await apiToken()}` },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  return res.status === 204 ? undefined : res.json();
}

describe('projects', () => {
  beforeAll(async () => {
    await roleProfiles();
    const pyro = await browser();
    // One snapshot for this file: signed in as a member, on /projects, the list loaded.
    await pyro.profile('member-projects', {
      base: 'member',
      url: `${rt().origin}/login`,              // required, but a derived page stays where its base left it
      setup: [route('/projects'), check(label('Project name')).visible()],
    });
    defer(() => pyro.releaseProfile('member-projects'));   // after this describe's last test
  });

  test('creates a project', async () => {
    const name = `Roadmap ${crypto.randomUUID().slice(0, 8)}`;
    defer(async () => {                          // runs after the test, pass or fail
      const list: { id: number; name: string }[] = await api('GET', '/api/projects');
      for (const p of list.filter((p) => p.name === name)) await api('DELETE', `/api/projects/${p.id}`);
    });
    await using page = await (await browser()).page({ as: 'member-projects' });
    await page.run([
      label('Project name').fill(name),
      button('Create').click(),
      check.request('POST', '/api/projects').status(201),
      check(text(`Created ${name}`)).visible(),
    ]);
  });

  test('shows a project created after the snapshot', async () => {
    const project = await api('POST', '/api/projects', { name: `Seeded ${crypto.randomUUID().slice(0, 8)}` });
    defer(() => api('DELETE', `/api/projects/${project.id}`));
    // The snapshot loaded the list before this project existed: route again to fetch it.
    await using page = await (await browser()).page({ as: 'member-projects', route: '/projects' });
    await page.run([check(heading('Projects')).visible(), check(testId(`project-${project.id}`)).visible()]);
  });
});
```

A derived profile's page does not navigate to its own `url`. The option is required, so pass the base's URL, and move with `route` in the `setup` (as here) or with `page({ as, route })`.

The second test shows the rule for data: the snapshot holds the list as it was when `setup` ran. A project created later appears only after the app fetches again, so that test takes its page with `route: '/projects'`, which makes the app render the route and fetch.

## Tests that sign out

Every page of `member` carries the same session token, the one its setup received. Signing out on one of those pages revokes that token on the server, for all of them. So a sign-out test signs in on its own page, starting from `signed-out`:

```ts
// e2e/auth.test.ts
import { browser, test } from '@povio/pyro/e2e';
import { button, check, heading, text } from '@povio/pyro';
import { roleProfiles, signIn } from './profiles';

test('an admin opens the users page', async () => {
  await roleProfiles();
  await using page = await (await browser()).page({ as: 'admin', route: '/users' });
  await page.run([check(heading('Users')).visible(), check(text('max@example.com')).visible()]);
});

test('signs out', async () => {
  await roleProfiles();
  // Signing out ends the session on the server. Every page of `member` shares
  // the session its setup created, so sign in here instead of using `member`.
  await using page = await (await browser()).page({ as: 'signed-out' });
  await page.run([
    ...signIn('max@example.com', 'member-pass'),
    button('Sign out').click(),
    check.request('POST', '/api/logout').status(204),
    check(heading('Sign in')).visible(),
  ]);
});
```

```text
$ bun run e2e
$ pyro e2e e2e --setup e2e/setup.ts --timeout 30000
pyro: serving http://localhost:57626 from /path/to/projects-app/dist
pyro: CDP server listening on ws://127.0.0.1:57627/ (fork mode)
✓ e2e/auth.test.ts > signs out [70.06ms]
✓ e2e/auth.test.ts > an admin opens the users page [78.61ms]
✓ e2e/projects.test.ts > projects > shows a project created after the snapshot [58.74ms]
✓ e2e/projects.test.ts > projects > creates a project [60.04ms]

 4 pass
Ran 4 tests across 2 files. [133.06ms]
```

Here is what happens when `signs out` takes its page from `member` instead (`page({ as: 'member', route: '/projects' })`, then only the Sign out click and the heading check), run with `-j 1`.

The sign-out test passes. The damage shows elsewhere: `member-projects` forks from `member` after the token was revoked, its list request gets a 401, and its setup fails. `pyro.profile('member-projects', ...)` in the describe's `beforeAll` rejects with a `ProfileSetupError`, and both tests of `projects` fail with it:

```text
HookError: beforeAll of describe 'projects' failed: profile member-projects: setup step 1 (expect Visible {"label":"Project name"}): timed out after 5000 ms: expect Visible {"label":"Project name"}: expected Visible null, got false
```

The step and its reason were recorded with a release up to 0.0.33, where they appear only on the server's stderr (`pyro: template: setup of profile 3: step 1 (...)`), `pyro.profile()` does not reject, and the two tests fail later on their own steps (`Fill {"label":"Project name"} matched no element`). With more concurrency the same mistake fails other tests at random, depending on which runs first.

## Keeping tests independent

Fork mode isolates the browser: each test's page is its own process, and nothing it does to the DOM, `localStorage`, cookies or mocks reaches another test. It does not isolate your backend. Tests stay independent when:

- **They create their own data**, with unique names, and delete it in a `defer` (above, and in depth in [API seeding and cleanup](/recipes/e2e-api-seeding-and-cleanup)).
- **They do not change what a shared session or shared data means for others**: no sign-out, password change or role change on a profile's session; sign in on a page of `signed-out` instead.
- **They do not rely on order.** Tests of all files run at once. Use `test.serial` or `describe.serial` only when tests must share server state.
- **They fetch what they need.** A snapshot holds the data of the moment it was taken; take the page with `route` to see data created since.

## Next

- [API seeding and cleanup](/recipes/e2e-api-seeding-and-cleanup): the data half of independence.
- [Fork mode in tests](/e2e/fork): the cost model and the platform limits of fork mode.
- [Mocking backend states](/recipes/e2e-mock-backend-states): states your backend cannot produce on demand.
