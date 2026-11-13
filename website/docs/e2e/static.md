# Serving a build: --static

`pyro serve --static ORIGIN=DIR` answers the browser's requests for `ORIGIN` from the files in `DIR`, as a static file server would, without anything listening at that origin. Point it at your front-end build and a CI job needs no front-end dev server or preview server: build once, then run the tests against the files. API calls that the directory cannot answer still go to the network, so your backend, or [mocks](/e2e/network#mocks), serve the data.

```sh
bunx pyro serve --fork --static http://localhost:3000=./dist
```

```ts
const pyro = await Pyro.launch({ args: ['--fork', '--static', `http://localhost:3000=${process.cwd()}/dist`] });
```

## How requests are answered

For a `GET` or `HEAD` request whose origin matches:

1. `DIR/<path>` when that file exists.
2. A directory's `index.html`.
3. For a navigation (a document request) to a path with no file: `DIR/index.html`, so single-page-app routes such as `/orders/42` load the app. This is the default `spa` fallback.

Everything else goes to the network as usual: a missing file that is not a navigation (`/api/items`), any `POST`/`PUT`/`DELETE`, and every other origin. So with the API on the same origin, `--static http://localhost:3000=dist` serves the build from disk while `fetch('/api/items')` still reaches the server listening on port 3000.

Files are read into memory when the server starts (up to 512 MB per process) and answered from there; a file past that budget, or added later, is read from disk. Responses are not cached by the HTTP cache: the directory is the cache. Paths with `..` are refused.

The directory sits behind interception: [mocks](/e2e/network#mocks) and Playwright routes see these requests first and can override them.

### --static-fallback network

```sh
bunx pyro serve --static http://localhost:3000=./dist --static-fallback network
```

With `network`, a missing path goes to the network at its original URL, navigations included. Existing assets still come from the directory. Use it when the server at that origin owns the app's routes or renders some documents itself, and you only want the built assets served from disk.

## A run against a static build

Here the "backend" is a Bun server that only answers `/api/items`. The HTML and JavaScript are the `public/` directory of the app from [Your first test](/e2e/first-test), served by the browser on the same origin:

```ts
import { Pyro, check, heading, text } from '@povio/pyro';

const api = Bun.serve({
  port: 0,
  fetch(req) {
    const { pathname } = new URL(req.url);
    if (pathname === '/api/items') return Response.json([{ id: 1, title: 'From API' }]);
    return new Response('api server: not found', { status: 404 });
  },
});
const origin = api.url.origin;
const dist = new URL('./public/', import.meta.url).pathname;

const pyro = await Pyro.launch({ args: ['--fork', '--static', `${origin}=${dist}`] });
try {
  await using page = await pyro.page({
    url: `${origin}/settings`,                                  // no such file: index.html (SPA fallback)
    initScripts: ['localStorage.setItem("token", "t")'],        // the app's sign-in
  });
  const r = await page.run([
    check(heading('Todos')).visible(),
    check(text('From API')).visible(),
    check.request('GET', '/api/items').status(200),
  ]);
  console.log(r.requests.map((q) => `${q.method} ${q.url} ${q.status}`));
} finally {
  pyro.close();
  api.stop(true);
}
```

```text
pyro: serving http://localhost:62233 from /private/tmp/docs-r1-e2e/public/
pyro: CDP server listening on ws://127.0.0.1:62234/ (fork mode)
[ "GET http://localhost:62233/settings 200", "GET http://localhost:62233/app.js 200",
  "GET http://localhost:62233/api/items 200"
]
```

`/settings` and `/app.js` came from the directory; `/api/items` reached the API server, which never served a file.

### A made-up origin

The origin does not have to exist: `--static http://app.test=./dist` works, and the page loads `http://app.test/`. Requests the directory cannot answer then go to the network for `app.test` and fail, so mock the API:

```ts
import { Pyro, check, goto, mock, text } from '@povio/pyro';

const dist = new URL('./public/', import.meta.url).pathname;
const pyro = await Pyro.launch({ args: ['--static', `http://app.test=${dist}`] });
await using page = await pyro.page({ initScripts: ['localStorage.setItem("token", "t")'] });
await page.run([
  mock('http://app.test/api/items', { json: [{ id: 7, title: 'Mocked item' }] }),
  goto('http://app.test/'),
  check(text('Mocked item')).visible(),
]);
pyro.close();
```

Use your real API origin when the tests should hit a real backend.

## Several origins

`--static` is repeatable. A micro-frontend shell and its remotes, or an app and a CDN origin, each get their own directory:

```sh
bunx pyro serve --static http://localhost:3000=./apps/shell/dist --static http://localhost:3001=./apps/orders/dist
```

## Seeding localStorage for every context

```sh
bunx pyro serve --static http://localhost:3000=./dist --local-storage auth.json
```

```json
{
  "http://localhost:3000": { "token": "seeded", "settings": { "theme": "dark" } },
  "http://app.test": { "token": "x" }
}
```

Top-level keys are origins (or URLs; only their origin is used). Values are objects of entries. A non-string value is stored as its JSON text, so `settings` above reads back as the string `'{"theme":"dark"}'`. Every connection and every new browser context starts with a fresh copy of the seed, and pages write only to their own context.

When a profile also sets `localStorage`, the profile's entries are written after the seed, so they win for the same key.

## Environment variables

For launchers that cannot pass flags:

| Variable | Same as |
|---|---|
| `PYRO_STATIC` | `--static`; several entries separated by `;`: `http://localhost:3000=/app/dist;http://localhost:3001=/orders/dist` |
| `PYRO_LOCAL_STORAGE` | `--local-storage <file>` |

`--static-fallback` has no environment variable.

At start the server prints what it serves and seeds on standard error (at the default `--log-level info`):

```text
pyro: localStorage preload for http://localhost:3000: 1 entries
pyro: serving http://localhost:3000 from ./dist
```

## With the suite runner

Return the flags from the setup module's `browser()`, so the build directory and the API URL come from what `setup()` prepared:

```ts
// e2e/setup.ts
import type { Setup } from '@povio/pyro/e2e';

type Runtime = { origin: string; dist: string };

export default {
  async setup() {
    // start the API (or point at a deployed one), build or locate the front end
    return { origin: 'http://localhost:3000', dist: `${process.cwd()}/apps/web/dist` };
  },
  browser: (rt) => ({ args: ['--static', `${rt.origin}=${rt.dist}`] }),
} satisfies Setup<Runtime>;
```

## Combining with profiles

Profiles, pools and fork mode work the same on a static origin: a profile's `url` is a URL on that origin, and its pages boot from the directory.

```ts
await pyro.profile('employee', { url: 'http://localhost:3000/platform', localStorage: tokens });
```

## Why this helps in CI

- No front-end server to start, wait for or keep alive: the build output is enough.
- No port conflicts between parallel jobs for the front end; only the API needs a port, or none if it is mocked.
- The browser answers files from memory without a socket, which is cheaper than a dev server that compiles on request.

See [CI recipes](/e2e/ci) for complete workflows.
