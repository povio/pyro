# Your first test

This page builds one test from nothing: install the package, start a small app, launch the browser, declare a profile, take a page, run the scenario, read values back and close everything. Every file below was run as shown with Bun 1.4.

## Install

```sh
bun add --dev @povio/pyro
```

The package bundles the browser for macOS 14+ (Apple silicon and Intel) and Linux x64 and arm64 with glibc 2.39 or newer (Ubuntu 24.04, for example). Nothing is downloaded at install time. The client runs on Bun and on Node 22+; the examples here use `bun test`. See [Installation](/guide/installation) and [Platforms and limits](/guide/platforms).

`@povio/pyro` is not on npm yet. Until its first release, follow [Before the first release](/guide/installation#try-today) to install it from a checkout or use the previous package name.

## The example app

Every example in this section runs against one small app. It is plain JavaScript with no build step, but it behaves like a single-page app: a client-side router, data fetched from a REST API, a session in `localStorage`.

- Without a `token` in `localStorage` it shows a sign-in form (`Email`, `Sign in`). Signing in stores `tok-<email>` as `token` and reloads.
- Signed in, it shows `Signed in`, a nav with `Todos` and `Settings`, the list from `GET /api/items`, and a form (`New todo`, `Add`) that sends `POST /api/items` and then shows `Added <title>`.
- Each item has a `Delete <title>` button that opens a confirmation dialog. Its `Delete` button sends `DELETE /api/items/<id>`.
- `/settings` is a client-side route with a `Settings` heading.
- `POST /api/seed` creates an item for tests, the same as `POST /api/items`.

Put the three files below in a new directory, then install the package there. Open each block to copy the file.

::: details public/index.html
```html
<!-- public/index.html -->
<!doctype html>
<html>
  <head><meta charset="utf-8"><title>Todos</title></head>
  <body>
    <main id="root">Loading…</main>
    <script type="module" src="/app.js"></script>
  </body>
</html>
```
:::

::: details public/app.js
```js
// public/app.js
const root = document.getElementById('root');
const token = localStorage.getItem('token');
const api = (path, init = {}) =>
  fetch(path, { ...init, headers: { 'content-type': 'application/json', authorization: `Bearer ${token}` } });

// Client-side routing: links change the URL in place; back, forward and `route` fire popstate.
addEventListener('popstate', () => render());
document.addEventListener('click', (event) => {
  const link = event.target.closest('a[href^="/"]');
  if (!link) return;
  event.preventDefault();
  history.pushState(null, '', link.getAttribute('href'));
  render();
});

function signIn() {
  root.innerHTML = `
    <h1>Please sign in</h1>
    <form>
      <label for="email">Email</label> <input id="email">
      <button type="submit">Sign in</button>
    </form>`;
  root.querySelector('form').onsubmit = (event) => {
    event.preventDefault();
    localStorage.setItem('token', `tok-${root.querySelector('#email').value}`);
    location.reload();
  };
}

async function render(status = '') {
  if (!token) return signIn();
  const nav = '<p>Signed in</p><nav><a href="/">Todos</a> <a href="/settings">Settings</a></nav>';
  if (location.pathname === '/settings') {
    root.innerHTML = `${nav}<h1>Settings</h1>`;
    return;
  }
  const items = await (await api('/api/items')).json();
  root.innerHTML = `${nav}
    <h1>Todos</h1>
    <ul>${items.map((i) => `<li><span data-testid="item-${i.id}">${i.title}</span>
      <button aria-label="Delete ${i.title}" data-id="${i.id}">Delete</button></li>`).join('')}</ul>
    <form>
      <label for="title">New todo</label> <input id="title">
      <button type="submit">Add</button>
    </form>
    <p role="status">${status}</p>`;
  root.querySelector('form').onsubmit = async (event) => {
    event.preventDefault();
    const title = root.querySelector('#title').value;
    await api('/api/items', { method: 'POST', body: JSON.stringify({ title }) });
    await render(`Added ${title}`);
  };
  for (const button of root.querySelectorAll('li button')) {
    button.onclick = () => confirmDelete(button.dataset.id, button.previousElementSibling.textContent);
  }
}

function confirmDelete(id, title) {
  const dialog = document.createElement('div');
  dialog.setAttribute('role', 'dialog');
  dialog.innerHTML = `<p>Delete ${title}? This cannot be undone.</p>
    <button>Cancel</button> <button>Delete</button>`;
  const [cancel, confirm] = dialog.querySelectorAll('button');
  cancel.onclick = () => dialog.remove();
  confirm.onclick = async () => {
    await api(`/api/items/${id}`, { method: 'DELETE' });
    dialog.remove();
    await render(`Deleted ${title}`);
  };
  root.append(dialog);
}

render();
```
:::

::: details server.ts (the API, and the files for any other path)
```ts
// server.ts
const items: { id: number; title: string }[] = [];
let nextId = 1;

export function startServer(port = 0) {
  return Bun.serve({
    port,
    async fetch(req) {
      const { pathname } = new URL(req.url);
      if (pathname === '/api/items' && req.method === 'GET') return Response.json(items);
      // POST /api/items is what the app calls; POST /api/seed is the same for tests.
      if ((pathname === '/api/items' || pathname === '/api/seed') && req.method === 'POST') {
        const item = { id: nextId++, title: (await req.json()).title };
        items.push(item);
        return Response.json(item, { status: 201 });
      }
      const m = /^\/api\/items\/(\d+)$/.exec(pathname);
      if (m && req.method === 'DELETE') {
        const i = items.findIndex((item) => item.id === Number(m[1]));
        if (i >= 0) items.splice(i, 1);
        return new Response(null, { status: i >= 0 ? 204 : 404 });
      }
      // Everything else is the app: index.html for any route, app.js by name.
      const file = pathname === '/app.js' ? 'app.js' : 'index.html';
      return new Response(Bun.file(new URL(`./public/${file}`, import.meta.url)));
    },
  });
}

if (import.meta.main) console.log(startServer(3000).url.href);
```
:::

`bun server.ts` serves it on port 3000 if you want to click through it yourself. The tests start it on a free port instead.

In a real project the app is your front-end build and your API. In CI you often need no front-end server at all: see [Serving a build: --static](/e2e/static). [Project layout](/recipes/e2e-project-layout) shows a whole suite for a built single-page app with an API and two roles.

## The test

```ts
// todos.test.ts
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { Pyro, button, check, label, testId, text } from '@povio/pyro';
import { startServer } from './server';

const server = startServer();
const APP = server.url.origin;
let pyro: Pyro;

beforeAll(async () => {
  // Starts `pyro serve` on a free port; it exits with this process.
  pyro = await Pyro.launch();
  // Pages of this profile boot at APP with a token in localStorage, ahead of the tests.
  await pyro.profile('user', {
    url: `${APP}/`,
    localStorage: { token: 'test-token' },
    viewport: { width: 1280, height: 800 },
  });
});

afterAll(() => {
  pyro?.close();
  server.stop(true);
});

test.concurrent('adds a todo', async () => {
  await using page = await pyro.page({ as: 'user' });
  const r = await page.run([
    label('New todo').fill('Buy milk'),
    button('Add').click(),
    check.request('POST', '/api/items').status(201),
    check(text('Added Buy milk')).visible(),
    text(/^Added /).read('status'),
  ]);
  expect(r.values.status).toBe('Added Buy milk');
});

test.concurrent('shows a todo created through the API', async () => {
  const res = await fetch(`${APP}/api/items`, { method: 'POST', body: JSON.stringify({ title: 'Walk the dog' }) });
  const item = (await res.json()) as { id: number };
  await using page = await pyro.page({ as: 'user', url: `${APP}/` });
  await page.run([check(testId(`item-${item.id}`)).text('Walk the dog')]);
});
```

```sh
$ bun test --timeout 10000 todos.test.ts
bun test v1.4.3 (c6da4a4d3)
pyro: CDP server listening on ws://127.0.0.1:53878/

 2 pass
 0 fail
 1 expect() calls
Ran 2 tests across 1 file. [90.00ms]
```

Give the test a longer timeout than its steps. `bun test` stops a test after 5,000 ms by default, and a step waits up to 5,000 ms too, so with the defaults a failing step shows only `this test timed out after 5000ms` and you never see why. With `--timeout 10000` the step fails first and you get its reason (see [When it fails](#when-it-fails)).

## What each part does

### `Pyro.launch()`

Starts `pyro serve` on a free local port and connects to it. The binary is `bin` if you pass one, else `$PYRO_BIN`, else the browser bundled for your platform. The server is told the test process's pid (`--parent-pid`) and exits when that process ends, however it ends. `pyro.close()` stops it sooner. Extra server flags go in `args`:

```ts
const pyro = await Pyro.launch({ args: ['--fork'] });
```

To use a browser that is already running, connect instead: `await Pyro.connect('ws://127.0.0.1:9222/')`. Start one with `bunx pyro serve --port 9222`.

### `pyro.profile(name, { ... })`

A profile is a kind of page your tests ask for by name. The browser boots pages of it ahead of time: it loads `url` with the `localStorage` entries written before the app's scripts run, at the given viewport, and keeps them waiting. `pyro.page({ as: 'user' })` then takes one that is already loaded and signed in. [Profiles and page pools](/e2e/profiles) lists every option.

### `pyro.page({ as, route, url })`

Returns a `Page`. With `as`, the page comes from that profile's pool; without it, you get a blank page in its own browser context. `route` changes the URL in place as a client-side router would see a link (`pushState` plus `popstate`), so the app does not boot again. `url` navigates for real. Either one runs as the first step of the first `run`.

The second test needs the list as it is after the seed. The pooled page booted before the test seeded its todo, so it shows the old list. `url` reloads the app to fetch it again. `route: '/'` would work as well here, because this app fetches the list each time a route renders, and it skips the reload. Use `route` when your app fetches on route changes, and `url` when it reads its data only once at boot.

### `await using`

`Page` implements `Symbol.asyncDispose`, so `await using page = ...` closes the page and its browser context when the test function returns or throws. Close every page: an open page stops its connection from booting the next pooled page. Without `await using`, call `page.close()` in a `finally` block.

`Pyro` itself has no dispose method. Call `pyro.close()` in `afterAll`.

### `page.run([...])`

Sends every step as one `Pyro.run` command and resolves with:

| Field | Contents |
|---|---|
| `values` | What `read(as)`, `evaluate(expr, as)`, `download({ as })`, `downloadCount(as)` and `popup(as)` steps stored, by name |
| `results` | One entry per step you passed: what a check saw (`{ actual }`), the request a request check matched, `settleMs` after an action |
| `requests` | Every request the page made during the run: `method`, `url`, `status`, `error`, `ms`, plus `requestHeaders`, `responseHeaders` and `postData` |
| `screenshots` | Results of `matchScreenshot` steps, by name |
| `ms` | Time the run took in the browser |

Assert on `values` with `bun:test`'s `expect`. Everything else is asserted inside the run with `check(...)` steps, because they wait in the browser until they hold.

When a step fails, `page.run` throws a `RunError` that names the step and why it was still waiting when its timeout passed. `error.step` is the index into the array you passed. [Debugging failures](/e2e/debugging) lists the messages and what they mean.

## When it fails

Change the check in the first test to `check(text('Added Buy bread')).visible()` and run it again:

```text
$ bun test --timeout 10000 todos.test.ts
bun test v1.4.3 (c6da4a4d3)
pyro: CDP server listening on ws://127.0.0.1:54218/

todos.test.ts:
error: step 3 (expect Visible {"text":"Added Buy bread"}): timed out after 5000 ms: expect Visible {"text":"Added Buy bread"}: expected Visible null, got false
 step: 3,

      at run (.../node_modules/@povio/pyro/src/index.ts:517:13)
      at async <anonymous> (/path/to/todos.test.ts:28:24)
(fail) adds a todo [5125.87ms]

 1 pass
 1 fail
Ran 2 tests across 1 file. [5.23s]
```

Read it from the right. `expected Visible null, got false` is the reason: nothing visible matched. `step 3` is the fourth step of the array, the check you changed. The steps before it passed, so the todo was added, and the text on screen is something else. Bun also prints a few lines of the SDK's source above the message (left out here); skip them.

## Concurrency with `bun test`

`test.concurrent` lets tests of one file overlap. They all share `pyro`, and each takes its own page, so they do not interfere. A pooled page is taken in one call; the pool refills in the background.

How much runs in parallel depends on the server:

- **One connection (the default).** All pages live on one browser thread. Concurrent tests interleave on it.
- **Several connections.** `Pyro.launch({ connections: 4 })` opens four browser threads, each with its own pools. A test takes its page from the connection with the fewest open pages. See [Profiles and page pools](/e2e/profiles#connections).
- **Fork mode.** `Pyro.launch({ args: ['--fork'] })` gives every page its own process, forked from one that booted the profile once. See [Fork mode in tests](/e2e/fork).

`bun test` runs the files of one process one after another. For a suite of many files, use [`pyro e2e`](/e2e/runner), which runs every file in one process with one queue and one browser.

## Next

- [Locators](/e2e/locators): how to find elements.
- [Steps and checks](/e2e/steps): every step a run can contain.
- [Requests and mocks](/e2e/network): assert on requests and fake responses.
- Starting a real suite? [Project layout](/recipes/e2e-project-layout) gives you the folders, the setup module, one profile per role and the `pyro e2e` scripts in one place.
