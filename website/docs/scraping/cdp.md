# Puppeteer and raw CDP

`lightningpanda serve` is a Chrome DevTools Protocol server. Anything that speaks CDP can drive it: Puppeteer, a WebSocket client in any language, or tools built on either. This page shows Puppeteer, then the protocol by hand, then Lightningpanda's own `Lightningpanda.*` and `LP.*` commands.

## The endpoints

```sh
lightningpanda serve --port 9222
```

| Endpoint | What it returns |
| --- | --- |
| `GET /json/version` | Browser info and `webSocketDebuggerUrl`, the WebSocket to connect to. |
| `GET /json`, `GET /json/list` | An empty list: a fresh connection has no targets to list. |
| `ws://host:port/` | The CDP WebSocket. Every connection gets its own browser. |

```sh
$ curl -s http://127.0.0.1:9222/json/version
{
  "Browser": "Chrome/124.0.6367.29",
  "Protocol-Version": "1.3",
  "User-Agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
  "V8-Version": "",
  "WebKit-Version": "",
  "webSocketDebuggerUrl": "ws://127.0.0.1:9222/"
}
```

The `Browser` and `User-Agent` fields name Chrome 124 so that clients take their Chrome code paths. Pages send Lightningpanda's own user agent; see [Proxies, user agents, headers](/scraping/identity#user-agent).

Two checks protect the server from web pages, as Chrome's do:

- **The `Host` header must be an IP address or `localhost`.** A request with `Host: browser.internal:9222` gets `403`. Connect by IP, or have a proxy in front rewrite the header. This matters in Docker and Kubernetes, where clients often use a service name; see [Running as a service](/scraping/service).
- **WebSocket upgrades with an `Origin` header are refused.** A script in a browser tab cannot connect. Node.js, Bun, Python and Puppeteer do not send one.

There is no authentication. Anyone who can reach the port controls the browser and can read every cookie in it.

## Puppeteer

`puppeteer-core` connects to a running server. Do not use the full `puppeteer` package's `launch()`; that starts Chrome.

```sh
bun add puppeteer-core
```

```ts
import puppeteer from 'puppeteer-core';

const browser = await puppeteer.connect({ browserURL: 'http://127.0.0.1:9222' });
// or: puppeteer.connect({ browserWSEndpoint: 'ws://127.0.0.1:9222/' })

const context = await browser.createBrowserContext(); // isolated cookies and storage
const page = await context.newPage();
await page.goto('http://127.0.0.1:4500/login');
await page.type('input[name=user]', 'alice');
await page.type('input[name=password]', process.env.PASSWORD ?? '');
await Promise.all([page.waitForNavigation(), page.click('button[type=submit]')]);
await page.waitForSelector('.order');
console.log(await page.$$eval('.order', (els) => els.map((e) => e.textContent)));

await context.close();
await browser.disconnect(); // leaves the server running
```

Tested with puppeteer-core 25.17.0 against Lightningpanda 0.0.33. These worked:

| Area | Calls |
| --- | --- |
| Navigation | `goto` (with status), `waitForNavigation`, `click` that navigates, `url`, `title`, `content` |
| Queries | `$`, `$$eval`, `$eval`, `waitForSelector`, `locator(...).map(...).wait()`, `aria/` selectors, `evaluate` |
| Input | `click`, `type` |
| Contexts | `createBrowserContext`, with `proxyServer` and `proxyBypassList` too, `browser.newPage()` on the default context |
| Identity | `setUserAgent`, `setExtraHTTPHeaders`, `authenticate` (server and proxy Basic auth) |
| Cookies | `page.setCookie`, `page.cookies`, `browser.cookies` |
| Network | `setRequestInterception` with `continue`/`abort`, `page.on('response')` with `response.json()` |
| Output | `screenshot`, `pdf` (with `format: 'A4'`, which Puppeteer converts to inches) |
| Accessibility | `page.accessibility.snapshot()` |

Things that differ from Chrome:

- `browser.pages()` is empty after connecting. Lightningpanda does not create a placeholder page; call `newPage()`.
- `browser.version()` and `browser.userAgent()` report the Chrome 124 strings from `/json/version`, not the page user agent.
- Closing the connection (`disconnect()`, or the process ending) disposes every context and page it made.
- Commands Lightningpanda does not implement fail with `... is not supported by lightningpanda` instead of succeeding silently. Among them: `DOM.getDocument`, `DOM.querySelector`, `DOM.getOuterHTML`, `Tracing.*`, `Page.startScreencast`, `HeapProfiler.collectGarbage`. Puppeteer features built on those (tracing, screencasts, some `ElementHandle` internals) do not work.

The [CDP coverage](/reference/cdp) page lists every method with its status.

## Raw CDP over a WebSocket

No library needed: CDP is JSON messages over a WebSocket. Each command has an `id`, a `method` and `params`; page commands also carry a `sessionId`. The reply has the same `id` and either `result` or `error`. Messages without an `id` are events.

This script runs on Bun (or Node.js 22+, which has a global `WebSocket`):

```ts
// raw-cdp.ts
const HTTP = process.env.LP_HTTP ?? 'http://127.0.0.1:9222';
const { webSocketDebuggerUrl } = await (await fetch(`${HTTP}/json/version`)).json();

const ws = new WebSocket(webSocketDebuggerUrl);
await new Promise((resolve, reject) => ((ws.onopen = resolve), (ws.onerror = reject)));

let nextId = 0;
const pending = new Map<number, { resolve: (v: any) => void; reject: (e: Error) => void }>();
ws.onmessage = (event) => {
  const msg = JSON.parse(String(event.data));
  const p = msg.id !== undefined && pending.get(msg.id);
  if (!p) return; // an event: msg.method, msg.params, msg.sessionId
  pending.delete(msg.id);
  msg.error ? p.reject(new Error(msg.error.message)) : p.resolve(msg.result);
};
function send(method: string, params: object = {}, sessionId?: string): Promise<any> {
  const id = ++nextId;
  ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

// A context, a page in it, and a session attached to the page.
const { browserContextId } = await send('Target.createBrowserContext');
const { targetId } = await send('Target.createTarget', { url: 'about:blank', browserContextId });
const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });

await send('Network.setCookies', { cookies: [{ name: 'sid', value: 'abc', url: 'http://127.0.0.1:4500' }] }, sessionId);
await send('Page.enable', {}, sessionId);

// Navigate, wait for load, read: three round trips.
const loaded = new Promise<void>((resolve) => {
  const previous = ws.onmessage!;
  ws.onmessage = (event) => {
    previous.call(ws, event);
    const msg = JSON.parse(String(event.data));
    if (msg.method === 'Page.loadEventFired' && msg.sessionId === sessionId) resolve();
  };
});
await send('Page.navigate', { url: 'http://127.0.0.1:4500/products?page=1' }, sessionId);
await loaded;
const { result } = await send(
  'Runtime.evaluate',
  { expression: "[...document.querySelectorAll('.product .name')].map((e) => e.textContent)", returnByValue: true },
  sessionId,
);
console.log(result.value);

// The same with LP.run: one round trip that navigates, waits until the page is quiet, and reads.
const run = await send(
  'LP.run',
  {
    steps: [
      { goto: { url: 'http://127.0.0.1:4500/products?page=2', waitUntil: 'load' } },
      { read: { locator: { css: '.product' }, read: 'count', as: 'count' } },
      { eval: { expression: "[...document.querySelectorAll('.product .name')].map((e) => e.textContent)", as: 'names' } },
    ],
  },
  sessionId,
);
console.log(run.reads.count, run.reads.names, `${run.ms} ms`);

const { cookies } = await send('Storage.getCookies', { browserContextId });
console.log(cookies.map((c: { name: string }) => c.name));

await send('Target.disposeBrowserContext', { browserContextId });
ws.close();
```

Notes:

- Use flat sessions (`flatten: true`) and put the `sessionId` on each page command. Puppeteer and Playwright do the same.
- `Runtime.evaluate` with `returnByValue: true` returns JSON; with `awaitPromise: true` it awaits a returned promise.
- Cookies: `Network.setCookies` / `Network.getCookies` on a page session, or `Storage.setCookies` / `Storage.getCookies` / `Storage.clearCookies` with a `browserContextId` on the browser session. Each context has its own jar. Partitioned cookies and non-default priority are refused.
- `Target.createBrowserContext` accepts `proxyServer` and `proxyBypassList` for a per-context proxy.
- The browser-level session also takes `Browser.grantPermissions`, `Browser.setDownloadBehavior` and `Security.setIgnoreCertificateErrors` (for every context).

The same pattern works from any language with a WebSocket client: Python's `websockets`, Go's `gorilla/websocket`, and so on.

## The `LP` and `Lightningpanda` domains

Lightningpanda adds two domains of its own.

**`LP.run`** (on a page session) runs a list of steps in the browser and replies once: navigation, locating elements by CSS, text, role, label or test id, actions (click, fill, type, press, hover, wheel, drag, upload), checks that wait until they hold, reads, network mocks, request checks, downloads and screenshots. Each action settles before the next step. `LP.idle { timeout }` waits until the page is quiet on its own. The reply has `reads`, `results`, `requests` and `ms`. The step format is in [LP.run wire format](/reference/lp-run); the native SDK builds it for you.

**`Lightningpanda.*`** (on the browser session) manages pre-booted pages:

| Command | What it does |
| --- | --- |
| `Lightningpanda.capabilities` | `{ fork, sharedProfiles }`: whether the server forks connections. |
| `Lightningpanda.registerPreparedPage` | Declare a profile: `url`, `initScripts`, `viewport`, `pool`; with `--fork` also `setup`, `base` and `name`. Returns `profileId`. |
| `Lightningpanda.acquirePreparedPage` | Take a booted page of a profile (`profileId` or `url`), optionally attached in the same call (`attach: true`). |
| `Lightningpanda.preparedPages` | Pages booted and waiting, with `ready`. |
| `Lightningpanda.unregisterPreparedPage` | Remove a profile (`profileId`), or drop this connection's hold on a named one (`name`). |
| `Lightningpanda.adoptPreparedPage` | Hold a named profile another client registered (`--fork`). |
| `Lightningpanda.preparedProfiles` | Named profiles and their holders (`--fork`). |

In fork mode, a connection to `ws://host:port/?profile=<profileId>` is forked from that profile's snapshot. The details are in [Lightningpanda CDP domain](/reference/cdp-domain), and the uses in [Profiles booted once (fork)](/scraping/fork-profiles).

## Related

- [CDP coverage](/reference/cdp): every method and its status.
- [Scripting a browser](/scraping/library): the Playwright library and the native SDK, which wrap all of this.
- [Running as a service](/scraping/service): exposing the endpoint safely.
