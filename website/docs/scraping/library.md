# Scripting a browser

Most scraping jobs are scripts: open a page, wait for the data, read it, move on. The package gives you two APIs for that, and both drive the same browser.

- **`@povio/pyro/wright`** is Playwright's library API: `chromium`, `Browser`, `BrowserContext`, `Page`, `Locator`. Playwright itself is not needed. Use it when you know Playwright, when you want `storageState` profiles, or when you port an existing script.
- **`@povio/pyro`** is the native SDK. A job is a list of steps sent as one `Pyro.run` command. The browser runs every step, waits in place, and replies once with everything you read. Use it when you scrape many pages and the round trips add up, or with fork mode.

Install the package in your project:

```sh
bun add @povio/pyro
# or: npm install @povio/pyro
```

The examples run on Bun (`bun script.ts`) and on Node.js 22+ (compile TypeScript first, or write `.mjs`).

## Launch or connect

There are two ways to get a browser.

**Launch** starts `pyro serve` on a free local port as a child process and connects to it. The server exits with your script, however the script ends.

```ts
import { chromium } from '@povio/pyro/wright';

const browser = await chromium.launch();
// Extra serve flags: chromium.launch({ args: ['--proxy', 'http://proxy.internal:3128'] })
```

**Connect** attaches to a server that is already running, on this machine or another:

```sh
pyro serve --port 9222
```

```ts
const browser = await chromium.connectOverCDP('http://127.0.0.1:9222');
// or the WebSocket URL directly: chromium.connectOverCDP('ws://127.0.0.1:9222/')
```

An `http://` endpoint is looked up through `/json/version`. Connecting is the better choice for long-running jobs, for several scripts sharing one server, and for a browser on another machine (see [Running as a service](/scraping/service)).

Each `connectOverCDP` or `launch` opens one CDP connection, and each connection is its own browser thread with its own JavaScript engine on the server. Pages in one connection share that thread. To use several CPU cores, open several connections. See [Concurrency at scale](/scraping/concurrency).

`browser.close()` closes every context this script opened. On a launched browser it also stops the server; on a connected one it only disconnects.

## Contexts and pages

A **browser context** is an isolated session: its own cookies, `localStorage`, `sessionStorage`, IndexedDB and cache. Two contexts never see each other's state. Use one context per identity or per job.

```ts
const context = await browser.newContext({
  userAgent: 'MyCrawler/1.0 (+https://example.com/bot)',
  viewport: { width: 1366, height: 768 },
  extraHTTPHeaders: { 'accept-language': 'en-GB,en;q=0.9' },
});
const page = await context.newPage();
// ...
await context.close(); // frees the context's pages and their memory on the server
```

`browser.newPage()` makes a context with a single page, closed together with the page.

Context options that take effect: `storageState`, `userAgent`, `viewport`, `extraHTTPHeaders`, `baseURL`, `colorScheme`, `reducedMotion`, `recordHar`. Downloads are always accepted. Others are accepted and ignored; [Proxies, user agents, headers](/scraping/identity) lists them and shows the CDP calls that work instead.

## Navigate, wait, read

```ts
import { chromium } from '@povio/pyro/wright';

const BASE = 'http://127.0.0.1:4500';
const browser = await chromium.launch();
const page = await browser.newPage();

const rows: { id: number; name: string; price: number }[] = [];
let url: string | null = `${BASE}/products?page=1`;
while (url) {
  const response = await page.goto(url); // resolves after the page loaded and went quiet
  if (!response?.ok()) throw new Error(`${url}: ${response?.status()}`);
  rows.push(
    ...(await page.$$eval('.product', (items) =>
      items.map((li) => ({
        id: Number((li as HTMLElement).dataset.id),
        name: li.querySelector('.name')!.textContent!,
        price: Number(li.querySelector('.price')!.textContent),
      })),
    )),
  );
  const next = page.getByRole('link', { name: 'Next' });
  url = (await next.count()) ? new URL((await next.getAttribute('href'))!, url).href : null;
}
console.log(rows.length, 'rows');
await browser.close();
```

Things to know:

- **`goto` returns at `load`.** It does not wait for requests the page's scripts start after that. On a test page that fetched its data from an inline script, `page.locator('.o').count()` right after `goto` was `0` six times out of six, even with a 0 ms API. Wait for the element you need, or for its response; `goto(url, { waitUntil: 'networkidle' })` also waits for requests in flight, but not for ones a timer starts later. [Data from single-page apps](/recipes/scraping-spa-data) shows each way. Clicks and other actions do settle: they return once the requests they started have finished (see [Settling and auto-waiting](/guide/settling)).
- **Locators auto-wait.** `page.locator('.order').first().textContent()` waits for the element, up to the timeout (`page.setDefaultTimeout(ms)`). `count()`, `allTextContents()` and `allInnerTexts()` do not wait, as in Playwright: they read what is there now, so call `.first().waitFor()` before them.
- **`page.evaluate(fn, arg)`** runs a function in the page and returns its JSON result. A returned promise is awaited.
- **`page.content()`** returns the page's HTML.

The locator API is Playwright's: `getByRole`, `getByText`, `getByLabel`, `getByTestId`, CSS, XPath, `filter`, `nth`, `and`, `or`. See [Locators and selectors](/playwright/locators). For extraction patterns (APIs, scrolling, downloads) see [Extracting data](/scraping/extracting).

## The native SDK: one round trip per job

The Playwright library sends one command per awaited call. A job that navigates, waits, reads five things and checks a link costs seven round trips. The native SDK sends the whole job as one list of steps:

```ts
import { Pyro, css, evaluate, goto, link } from '@povio/pyro';

const BASE = 'http://127.0.0.1:4500';
const pyro = await Pyro.launch(); // or: await Pyro.connect('ws://127.0.0.1:9222/')
try {
  await using page = await pyro.page(); // a blank page in its own context, closed at the end of the block
  const rows: unknown[] = [];
  let url: string | null = `${BASE}/products?page=1`;
  while (url) {
    const r = await page.run([
      goto(url),
      css('h1').read('heading'), // text of the single <h1>
      css('.product').read('count', 'count'), // how many match
      evaluate(
        `[...document.querySelectorAll('.product')].map((li) => ({
           id: Number(li.dataset.id),
           name: li.querySelector('.name').textContent,
           price: Number(li.querySelector('.price').textContent),
         }))`,
        'rows',
      ),
      evaluate(`document.querySelector('a[rel=next]')?.href ?? null`, 'next'),
    ]);
    console.log(r.values.heading, r.values.count, `${r.ms.toFixed(1)} ms in the browser`);
    rows.push(...(r.values.rows as unknown[]));
    url = r.values.next as string | null;
  }
  console.log('total', rows.length);

  // Click through instead of building URLs: the click settles on the navigation it starts.
  const r = await page.run([goto(`${BASE}/products?page=1`), link('Next').click(), css('h1').read('h')]);
  console.log(r.values.h, r.requests.map((q) => `${q.method} ${q.url} ${q.status}`));
} finally {
  pyro.close();
}
```

Output against the local test site:

```
Products, page 1 10 3.2 ms in the browser
Products, page 2 10 5.7 ms in the browser
...
total 47
Products, page 2 [ "GET http://127.0.0.1:4500/products?page=1 200", "GET http://127.0.0.1:4500/products?page=2 200" ]
```

What a run gives back:

| Field | Contents |
| --- | --- |
| `values` | Results of `read(as)` and `evaluate(expr, as)` steps, by name. |
| `results` | One entry per step: what a check saw, settle time. |
| `requests` | Every request the page made during the run: method, URL, status, error, time. |
| `ms` | Time spent in the browser. |

A step that cannot complete (an element that never appears, an expression that throws) fails the run with a `RunError` that names the step and what it was waiting for. Each step has a 5 s timeout by default; pass `page.run(steps, { timeout: 15_000 })` or a per-step `timeout`.

### Reading with steps

- `locator.read(as, what)` reads from one element. `what` is `'text'` (default), `'value'`, `'count'`, `'visible'`, `'box'`, `'css'` (with a property name), `'name'` (accessible name), `'focused'` or `'enabled'`. Reading text from a locator that matches several elements fails with a strict mode error, as in Playwright; use `.first()`, `.nth(i)` or `'count'`.
- `evaluate(expression, as)` runs any expression and stores its JSON value. Use it to read lists.

Locators are data: `css`, `text`, `role` and its shortcuts (`button`, `link`, `heading`, `row`, `cell` and more), `label`, `placeholder`, `testId`, chained (`css('#list').text('Item 3')`) and filtered (`.hasText()`, `.visible()`, `.first()`). See [SDK: locators and steps](/reference/steps).

### Connections

`Pyro.connect(ws, { connections: 4 })` and `Pyro.launch({ connections: 4 })` open four browser threads; `pyro.page()` takes a page on the least busy one. Against a server started with `--fork`, every `pyro.page()` gets its own forked process instead, and `connections` makes no difference. See [Profiles booted once (fork)](/scraping/fork-profiles).

### Raw CDP from the SDK

`page.send(method, params)` sends any CDP command to the page's session, and `pyro.cdp.send(method, params)` sends browser-level commands. For example, to override the timezone before a run:

```ts
await page.send('Emulation.setTimezoneOverride', { timezoneId: 'America/New_York' });
```

## Which one to use

| | Playwright library | Native SDK |
| --- | --- | --- |
| Round trips per job | one per awaited call | one per `page.run` |
| Stored `storageState` profiles | yes (`newContext({ storageState })`) | no; `localStorage` seeds and sign-in `setup` steps instead |
| Fork mode and pre-booted signed-in pages | not used | yes |
| Network events, `route`, HAR | yes | `mock` steps and the run's `requests` list |
| Per-identity proxy | one server per proxy (`launch({ args: ['--proxy', url] })`) | one server per proxy, or a context with `proxyServer` over CDP |
| Known API | Playwright's | its own, Playwright-shaped |

You can mix them: use the Playwright library to sign in and save profiles, and the native SDK for the bulk of the crawl. For the choice in general, see [Choosing an API](/guide/choosing-an-api).

## Next

- Signed-in identities stored on disk, each with its own proxy and user agent: [Stored profiles](/scraping/profiles), then the complete [identity pool recipe](/recipes/scraping-identity-pool).
- Many short signed-in jobs on a server: [Fork-mode signed-in jobs](/recipes/scraping-fork-signed-in-jobs).
- Pages that load their data after `load`: [Data from single-page apps](/recipes/scraping-spa-data).
