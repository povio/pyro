# Your first test

This page builds one test from nothing: install the package, start a tiny app, launch the browser, declare a profile, take a page, run the scenario, read values back and close everything. Every file below was run as shown with `@povio/lightning-panda` 0.0.33 and Bun.

## Install

```sh
bun add --dev @povio/lightning-panda
```

The package bundles the browser for macOS 14+ (Apple silicon and Intel) and Linux x64 and arm64 with glibc 2.39 or newer (Ubuntu 24.04, for example). Nothing is downloaded at install time. The client runs on Bun and on Node 22+; the examples here use `bun test`. See [Installation](/guide/installation) and [Platforms and limits](/guide/platforms).

## A tiny app

The app shows a list of todos from `GET /api/items` and adds one with `POST /api/items`. It treats a `token` in `localStorage` as being signed in.

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

```js
// public/app.js
const root = document.getElementById('root');
const token = localStorage.getItem('token');
const api = (path, init = {}) =>
  fetch(path, { ...init, headers: { 'content-type': 'application/json', authorization: `Bearer ${token}` } });

async function render(status = '') {
  if (!token) {
    root.innerHTML = '<h1>Please sign in</h1>';
    return;
  }
  const items = await (await api('/api/items')).json();
  root.innerHTML = `
    <h1>Todos</h1>
    <ul>${items.map((i) => `<li data-testid="item-${i.id}">${i.title}</li>`).join('')}</ul>
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
}
render();
```

A small Bun server serves the files and the API:

```ts
// server.ts
const items: { id: number; title: string }[] = [];

export function startServer(port = 0) {
  return Bun.serve({
    port,
    async fetch(req) {
      const { pathname } = new URL(req.url);
      if (pathname === '/api/items' && req.method === 'GET') return Response.json(items);
      if (pathname === '/api/items' && req.method === 'POST') {
        const item = { id: items.length + 1, title: (await req.json()).title };
        items.push(item);
        return Response.json(item, { status: 201 });
      }
      const file = pathname === '/app.js' ? 'app.js' : 'index.html';
      return new Response(Bun.file(new URL(`./public/${file}`, import.meta.url)));
    },
  });
}

if (import.meta.main) console.log(startServer(3000).url.href);
```

In a real project the app is your front-end build and API. In CI you often do not need a front-end server at all: see [Serving a build: --static](/e2e/static).

## The test

```ts
// todos.test.ts
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { Lightningpanda, button, check, label, testId, text } from '@povio/lightning-panda';
import { startServer } from './server';

const server = startServer();
const APP = server.url.origin;
let lp: Lightningpanda;

beforeAll(async () => {
  // Starts `lightningpanda serve` on a free port; it exits with this process.
  lp = await Lightningpanda.launch();
  // Pages of this profile boot at APP with a token in localStorage, ahead of the tests.
  await lp.profile('user', {
    url: `${APP}/`,
    localStorage: { token: 'test-token' },
    viewport: { width: 1280, height: 800 },
  });
});

afterAll(() => {
  lp?.close();
  server.stop(true);
});

test.concurrent('adds a todo', async () => {
  await using page = await lp.page({ as: 'user' });
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
  await using page = await lp.page({ as: 'user', url: `${APP}/` });
  await page.run([check(testId(`item-${item.id}`)).text('Walk the dog')]);
});
```

```sh
$ bun test todos.test.ts
lightningpanda: CDP server listening on ws://127.0.0.1:51467/

 2 pass
 0 fail
 1 expect() calls
Ran 2 tests across 1 file. [308.00ms]
```

## What each part does

### `Lightningpanda.launch()`

Starts `lightningpanda serve` on a free local port and connects to it. The binary is `bin` if you pass one, else `$LIGHTNINGPANDA_BIN`, else the browser bundled for your platform. The server is told the test process's pid (`--parent-pid`) and exits when that process ends, however it ends. `lp.close()` stops it sooner. Extra server flags go in `args`:

```ts
const lp = await Lightningpanda.launch({ args: ['--fork'] });
```

To use a browser that is already running, connect instead: `await Lightningpanda.connect('ws://127.0.0.1:9222/')`. Start one with `bunx lightningpanda serve --port 9222`.

### `lp.profile(name, { ... })`

A profile is a kind of page your tests ask for by name. The browser boots pages of it ahead of time: it loads `url` with the `localStorage` entries written before the app's scripts run, at the given viewport, and keeps them waiting. `lp.page({ as: 'user' })` then takes one that is already loaded and signed in. [Profiles and page pools](/e2e/profiles) lists every option.

### `lp.page({ as, route, url })`

Returns a `Page`. With `as`, the page comes from that profile's pool; without it, you get a blank page in its own browser context. `route` changes the URL in place as a client-side router would see a link (`pushState` plus `popstate`), so the app does not boot again. `url` navigates for real. Either one runs as the first step of the first `run`.

The second test uses `url` on purpose. The pooled page booted before the test seeded its todo, and this app reads the list only when it starts, so the test reloads it. An app whose routes fetch their data when they mount can use `route` instead and skip the reload.

### `await using`

`Page` implements `Symbol.asyncDispose`, so `await using page = ...` closes the page and its browser context when the test function returns or throws. Close every page: an open page stops its connection from booting the next pooled page. Without `await using`, call `page.close()` in a `finally` block.

`Lightningpanda` itself has no dispose method. Call `lp.close()` in `afterAll`.

### `page.run([...])`

Sends every step as one `LP.run` command and resolves with:

| Field | Contents |
|---|---|
| `values` | What `read(as)`, `evaluate(expr, as)`, `download({ as })`, `downloadCount(as)` and `popup(as)` steps stored, by name |
| `results` | One entry per step you passed: what a check saw (`{ actual }`), the request a request check matched, `settleMs` after an action |
| `requests` | Every request the page made during the run: `method`, `url`, `status`, `error`, `ms`, plus `requestHeaders`, `responseHeaders` and `postData` |
| `screenshots` | Results of `matchScreenshot` steps, by name |
| `ms` | Time the run took in the browser |

Assert on `values` with `bun:test`'s `expect`. Everything else is asserted inside the run with `check(...)` steps, because they wait in the browser until they hold.

When a step fails, `page.run` throws a `RunError` that names the step and why it was still waiting when its timeout passed:

```text
RunError: step 0 (expect Text {"text":"Todos"}): timed out after 300 ms: expect Text {"text":"Todos"}: expected Text "Something", got "Todos"
```

`error.step` is the index into the array you passed. [Debugging failures](/e2e/debugging) lists the messages and what they mean.

## Concurrency with `bun test`

`test.concurrent` lets tests of one file overlap. They all share `lp`, and each takes its own page, so they do not interfere. A pooled page is taken in one call; the pool refills in the background.

How much runs in parallel depends on the server:

- **One connection (the default).** All pages live on one browser thread. Concurrent tests interleave on it.
- **Several connections.** `Lightningpanda.launch({ connections: 4 })` opens four browser threads, each with its own pools. A test takes its page from the connection with the fewest open pages. See [Profiles and page pools](/e2e/profiles#connections).
- **Fork mode.** `Lightningpanda.launch({ args: ['--fork'] })` gives every page its own process, forked from one that booted the profile once. See [Fork mode in tests](/e2e/fork).

`bun test` runs the files of one process one after another. For a suite of many files, use [`lightningpanda e2e`](/e2e/runner), which runs every file in one process with one queue and one browser.

## Next

- [Locators](/e2e/locators): how to find elements.
- [Steps and checks](/e2e/steps): every step a run can contain.
- [Requests and mocks](/e2e/network): assert on requests and fake responses.
