# Network, routes and HAR

Request interception, network events, HAR replay and recording, API requests and storage state work with Playwright's API. Routes intercept in the browser (CDP `Fetch`), so a mocked response never leaves the machine. This page covers each feature and the places where Pyro differs.

## Routes

```ts
test('shows an empty state', async ({ page }) => {
  await page.route('**/api/todos', (route) =>
    route.fulfill({ status: 200, json: [] }),
  );
  await page.goto('/');
  await expect(page.getByText('Nothing to do')).toBeVisible();
});
```

`page.route(url, handler, { times })` and `context.route(url, handler, { times })` take a glob string (resolved against `baseURL`), a RegExp, or a predicate on the `URL`. The newest route is tried first, page routes before context routes. A request no route handles goes to the network.

A route handler must decide: `fulfill`, `continue`, `abort` or `fallback`. Calling a second one throws `Route is already handled!`.

| Method | Options |
| --- | --- |
| `route.fulfill()` | `status`, `headers`, `contentType`, `body` (string or Buffer), `json`, `path` (content type from the extension), `response` (a fetched response to base it on) |
| `route.continue()` | `url`, `method`, `headers`, `postData` (string, Buffer or object) |
| `route.abort(errorCode)` | `aborted`, `accessdenied`, `addressunreachable`, `blockedbyclient`, `blockedbyresponse`, `connectionaborted`, `connectionclosed`, `connectionfailed`, `connectionrefused`, `connectionreset`, `internetdisconnected`, `namenotresolved`, `timedout`, `failed` (default) |
| `route.fallback()` | Same overrides as `continue`; the next matching route (or the network) gets the request with them. |
| `route.fetch()` | `url`, `method`, `headers`, `postData`, `maxRedirects`. Returns a response with `status()`, `headers()`, `body()`, `text()`, `json()`. |
| `route.request()` | The intercepted `Request`. |

```ts
// Patch a real response
await page.route('**/api/user', async (route) => {
  const response = await route.fetch();
  const user = await response.json();
  await route.fulfill({ response, json: { ...user, plan: 'pro' } });
});

// Add a header, then let other routes see the request
await page.route('**/api/**', (route) =>
  route.fallback({ headers: { ...route.request().headers(), 'x-test-run': '1' } }),
);
```

::: warning route.fetch() runs from the test process
`route.fetch()` sends the request with `fetch` from the test process (Bun or Node), not from the browser. It carries the intercepted request's headers, but not cookies that live only in the browser's jar.
:::

`page.unroute(url, handler?)`, `context.unroute(...)` and `unrouteAll()` remove routes. The `behavior` option of `unrouteAll` is ignored.

Requests served by `--static` (see [Serving a build](/e2e/static)) still pass through routes.

## Waiting for requests and responses

```ts
const saved = page.waitForResponse(
  (r) => r.url().endsWith('/api/todos') && r.request().method() === 'POST' && r.status() === 201,
);
await page.getByRole('button', { name: 'Add' }).click();
await saved;
```

`page.waitForRequest(urlOrPredicate)` and `page.waitForResponse(urlOrPredicate)` take a glob, a RegExp or a predicate (which may be async). Start the wait before the action, as in Playwright. The default timeout is the page's action timeout (30 s unless set).

Because actions settle before they return, a request an action starts has usually finished by the time `click()` returns. The promise you created before the click resolves either way.

## Network events

| Event | On | Payload |
| --- | --- | --- |
| `request` | page, context | `Request` |
| `response` | page, context | `Response` |
| `requestfinished` | page, context | `Request` |
| `requestfailed` | page, context | `Request` (`failure().errorText`) |
| `websocket` | page | `WebSocket` |

Network tracking starts when you first listen, wait for, or record. Requests that were already in flight are not reported.

`Request` has `url`, `method`, `headers`, `allHeaders`, `headerValue`, `postData`, `postDataBuffer`, `postDataJSON`, `resourceType`, `isNavigationRequest`, `response`, `failure`, `frame`, `redirectedFrom`, `redirectedTo` and `timing`. Limits: `frame()` is always the main frame, and every `timing()` field is -1.

`Response` has `url`, `status`, `statusText`, `ok`, `headers`, `allHeaders`, `headerValue`, `request`, `frame`, `finished`, `body`, `text` and `json`. `body()` works for responses seen through network events. The response that `page.goto()` returns carries the URL and status only: its `headers()` is empty and `body()` throws. Read headers with `page.waitForResponse` or `page.request` instead.

## API requests

The `request` fixture, `page.request`, `context.request` and `playwright.request.newContext()` give an `APIRequestContext`:

```ts
test('seeds then checks', async ({ request, page }) => {
  const created = await request.post('/api/todos', { data: { title: 'Milk' } });
  await expect(created).toBeOK();
  const { id } = await created.json();
  await page.goto(`/todos/${id}`);
});
```

| Method | Options |
| --- | --- |
| `get`, `post`, `put`, `patch`, `delete`, `head`, `fetch(urlOrRequest)` | `params`, `headers`, `data` (string, Buffer or JSON), `form`, `multipart` (values, `{ name, mimeType, buffer }` files or `FormData`), `timeout` (default 30 s), `failOnStatusCode`, `ignoreHTTPSErrors`, `maxRedirects` (0 disables redirects; other values follow redirects) |
| `storageState({ path })` | Cookies only. |
| `dispose()` | |

`newContext` options: `baseURL`, `extraHTTPHeaders`, `storageState` (its cookies), `timeout`, `failOnStatusCode`, `ignoreHTTPSErrors`, `userAgent`, `httpCredentials` (always sent as Basic auth). `maxRetries`, `proxy` and client certificates are not supported.

The context keeps its own cookie jar from `storageState` and `Set-Cookie` responses. Unlike Playwright, `context.request` and `page.request` do **not** share cookies with the browser context: cookies the page sets are not sent by `page.request`, and the other way around.

The requests run with `fetch` in the test process, not in the browser.

## HAR replay and recording

Replay a HAR file as mocks:

```ts
await page.routeFromHAR('e2e/hars/catalog.har', { url: '**/api/**', notFound: 'fallback' });
```

A request matches an entry by method and URL; when several match, by request body. `notFound: 'abort'` (the default) aborts unmatched requests; `'fallback'` passes them on. Bodies may be inline (`text`, base64 when `encoding` is `base64`) or in a file beside the HAR (`_file`).

Record one by passing `update: true`:

```ts
await page.routeFromHAR('e2e/hars/catalog.har', { url: '**/api/**', update: true });
// ... drive the page ...
await page.context().close(); // the file is written now
```

- The file is written when the context closes.
- `updateContent: 'attach'` (the default) writes bodies beside the HAR, named by hash; `'embed'` puts them inline; `'omit'` leaves them out.
- `updateMode: 'minimal'` leaves request headers out.
- The output is a HAR 1.2 JSON file. `.zip` archives are not produced.

`context.routeFromHAR` does the same for every page of the context. The `recordHar` context option records too:

```ts
const context = await browser.newContext({
  recordHar: { path: 'out/session.har', content: 'embed', urlFilter: '**/api/**' },
});
```

## WebSockets

```ts
page.on('websocket', (ws) => {
  ws.on('framereceived', ({ payload }) => console.log('<', payload));
  ws.on('framesent', ({ payload }) => console.log('>', payload));
  ws.on('close', () => console.log('closed', ws.url()));
});
```

`WebSocket` has `url()`, `isClosed()`, the events `framesent`, `framereceived` (payload is a string for text frames, a Buffer for binary ones; empty frames are not reported), `socketerror` and `close`, and `waitForEvent`, which rejects when the socket closes or errors first. An open socket does not hold up an action's settling.

`page.routeWebSocket()` and `context.routeWebSocket()` (mocking WebSocket servers) are not implemented.

## exposeFunction and exposeBinding

```ts
import { createHash } from 'node:crypto';

await page.exposeFunction('sha256', (text: string) =>
  createHash('sha256').update(text).digest('hex'),
);
const hash = await page.evaluate(() => (window as any).sha256('abc'));
```

`page.exposeFunction`, `page.exposeBinding`, `context.exposeFunction` and `context.exposeBinding` define `window[name]` in the current document and every later one. The call returns a promise of the function's result, which must be JSON-serializable. The binding source is `{ page, frame, context }`, where `frame` is always the main frame. Registering a name twice throws.

## Headers, credentials, proxy, offline

| Option or method | Status |
| --- | --- |
| `extraHTTPHeaders` (option), `context.setExtraHTTPHeaders()`, `page.setExtraHTTPHeaders()` | Supported. |
| `userAgent` (option) | Supported. |
| `httpCredentials` (option) | Used by `request` only. Page requests do not send them. Put credentials in `extraHTTPHeaders` (`authorization: 'Basic ...'`) for pages. |
| `proxy` (option) | No effect. Pass the browser flag instead: `use: { pyro: { args: ['--proxy', 'http://user:pass@host:3128'] } }`. HTTP proxies only, `CONNECT` for HTTPS, no SOCKS. `--proxy-bypass` takes a bypass list. See [serve options](/reference/serve). |
| `offline`, `context.setOffline()` | No effect. Use a route that aborts: `context.route('**/*', (r) => r.abort('internetdisconnected'))`. |
| `ignoreHTTPSErrors` (option) | Used by `request` only. |

## storageState and cookies

```ts
// Save after signing in (for example in globalSetup)
const context = await browser.newContext();
const page = await context.newPage();
await page.goto('/login');
// ... sign in ...
await context.storageState({ path: 'e2e/.auth/user.json' });

// Use it
export default defineConfig({ use: { storageState: 'e2e/.auth/user.json' } });
```

- `storageState` (a path or an object) seeds cookies and `localStorage`. Local storage entries are written once per context, by an init script, the first time a document of that origin loads. A marker key, `__lpStorageState`, stays in `localStorage` so later documents do not overwrite the app's changes. Code that lists every key of `localStorage` sees it.
- `context.storageState({ path })` returns cookies and the `localStorage` of the origins of the context's open pages. Origins with no open page are not included. `indexedDB` is not saved.
- `context.addCookies()`, `context.cookies(urls?)` and `context.clearCookies()` work. A cookie needs `url` or `domain`. Before the context's first page exists, cookies are held and set when the page gets its target. `clearCookies` options are ignored; it clears all cookies.

To set up storage once for the whole run, use `globalSetup`, since project `dependencies` do not run (see [Config and the test runner](/playwright/runner#accepted-and-ignored)). Pyro also has its own way to keep signed-in pages booted; see [Booting pages ahead](/playwright/boot-pools) and, for the native API, [Profiles and page pools](/e2e/profiles).
