# Extracting data

Once a page has loaded, there are several ways to get data out of it: run JavaScript in the page, read through locators, take a compact accessibility snapshot, or skip the DOM and capture the JSON the page fetched. This page shows each, with the waiting, pagination, scrolling and download patterns around them.

All examples run against a local test site at `http://127.0.0.1:4500`. Replace the URLs and selectors with your own.

## Run JavaScript in the page

The most flexible way: one function that walks the DOM and returns plain data. It runs in the page, so a list of 1,000 rows costs one round trip, not 1,000.

```ts
import { chromium } from '@povio/pyro/wright';

const browser = await chromium.launch();
const page = await browser.newPage();
await page.goto('http://127.0.0.1:4500/products?page=1');

const products = await page.$$eval('.product', (items) =>
  items.map((li) => ({
    id: Number((li as HTMLElement).dataset.id),
    name: li.querySelector('.name')?.textContent?.trim(),
    price: Number(li.querySelector('.price')?.textContent),
  })),
);
// [{ id: 1, name: "Item 1", price: 3 }, ...]
```

- `page.$$eval(selector, fn)` passes every match to `fn`; `page.$eval` the first one.
- `page.evaluate(fn, arg)` runs any function with one serializable argument: `page.evaluate((sel) => document.querySelectorAll(sel).length, '.product')`.
- `locator.evaluateAll(fn)` does the same for a locator's matches.
- Return JSON-serializable values. DOM nodes do not cross over; return their text or attributes.
- A returned promise is awaited, so `page.evaluate(() => fetch('/api/items').then((r) => r.json()))` works and uses the page's cookies.

## Read through locators

Locators wait for their element and read one value each:

```ts
await page.locator('#list').innerText(); // the rendered text, line breaks from layout
await page.locator('.product .name').allTextContents(); // ["Item 1", "Item 2", ...]
await page.locator('.product').allInnerTexts(); // ["Item 1 3", "Item 2 6", ...]
await page.getByRole('link', { name: 'Next' }).getAttribute('href'); // "/products?page=2"
await page.locator('.product').count();
```

`textContent` is the raw text of every descendant, hidden ones included. `innerText` follows CSS: `display: none` text is left out and block elements start new lines. Use `getByRole`, `getByText` and `getByLabel` when the markup has no stable classes; they find elements the way a user would. See [Locators and selectors](/playwright/locators).

A method that reads one element (`textContent`, `getAttribute`, `innerText`) on a locator that matches several fails with a strict mode error. Use `.first()`, `.nth(i)`, or the `all...` methods.

## Read with native steps

With the native SDK, reads are steps of a run, and all values come back in one reply:

```ts
import { Pyro, css, evaluate, goto, heading, link } from '@povio/pyro';

const pyro = await Pyro.launch();
await using page = await pyro.page();
const r = await page.run([
  goto('http://127.0.0.1:4500/products?page=1'),
  heading().read('title'), // text
  css('.product').read('count', 'count'), // number of matches
  link('Next').read('nextVisible', 'visible'), // true / false
  css('h1').read('color', 'css', 'color'), // a computed style
  evaluate(`[...document.querySelectorAll('.product .name')].map((e) => e.textContent)`, 'names'),
]);
console.log(r.values); // { title, count, nextVisible, color, names }
pyro.close();
```

`read` waits for its element like any step, and fails the run if it never appears. `read` kinds: `text`, `value`, `count`, `visible`, `box`, `css`, `name`, `focused`, `enabled`. See [Scripting a browser](/scraping/library#the-native-sdk-one-round-trip-per-job).

## A compact view of the page: `ariaSnapshot`

`ariaSnapshot()` returns the page's accessibility tree as indented YAML: roles, accessible names and states, with layout markup left out. It is much smaller than the HTML and keeps what a reader of the page would notice, which makes it a good input for a language model that has to understand or act on a page.

```ts
console.log(await page.ariaSnapshot());
```

```yaml
- heading "Products, page 1" [level=1]
- list:
  - listitem: Item 1 3
  - listitem: Item 2 6
  ...
- link "Next":
  - /url: /products?page=2
```

`ariaSnapshot({ mode: 'ai' })` adds a reference to each element, the active element, and pointer hints, as Playwright's AI mode does:

```yaml
- generic [active] [ref=e1]:
  - heading "Products, page 1" [level=1] [ref=e2]
  - list [ref=e3]:
    - listitem [ref=e4]: Item 1 3
```

An agent can then act on an element by its reference: `page.locator('aria-ref=e4').click()` finds the element that ref named in the last AI snapshot of its frame. `locator.ariaSnapshot()` takes the snapshot of one part of the page. Iframes appear nested in AI mode, with refs like `f1e5`.

The tree is built in the browser from the same role and name code `getByRole` uses. Over raw CDP, `Accessibility.getFullAXTree` returns Chrome's node format instead. For writing agents against Pyro, see [Writing tests with AI agents](/e2e/ai-agents).

## Waiting for data

Pyro settles after each action: `goto`, `click`, `fill` and the rest return once the page is quiet. Quiet means no queued task, no request the action started still in flight, no timer due within 50 ms, no pending animation frame, and no navigation short of `DOMContentLoaded`. Data a page fetches right after it loads is usually in the DOM when `goto` returns. See [Settling and auto-waiting](/guide/settling).

Settling does not wait for everything. A timer set for 2 seconds, a poll, a WebSocket message or a request started by a later timer can come after it. Wait for what you need, in order of preference:

1. **The element.** `await page.locator('.order').first().waitFor()`, or any locator read, which waits by itself. In the native SDK, a `read` or `check` step waits until it holds.
2. **The response.** `await page.waitForResponse((r) => r.url().includes('/api/orders'))`; see below.
3. **A condition.** `await page.waitForFunction(() => window.__APP_READY__ === true)`.
4. **Network quiet.** `await page.waitForLoadState('networkidle')`, or an `idle()` step in the native SDK, which waits until the page is quiet again and reports what was still pending if it gives up.
5. **A fixed delay**, `page.waitForTimeout(ms)`, only as a last resort.

## Intercepting JSON APIs

Many sites render from a JSON API. Reading the JSON is more reliable than parsing the HTML built from it, and it often holds fields the page does not show.

**Wait for one response:**

```ts
const response = page.waitForResponse((r) => r.url().includes('/api/items') && r.status() === 200);
await page.goto('http://127.0.0.1:4500/late');
const data = await (await response).json();
```

Start waiting before the action that triggers the request, as above, or you can miss it.

**Collect every response:**

```ts
const captured: Promise<{ url: string; body: unknown }>[] = [];
page.on('response', (response) => {
  if (!response.url().includes('/api/') || !response.ok()) return;
  captured.push(response.json().catch(() => null).then((body) => ({ url: response.url(), body })));
});
await page.goto('http://127.0.0.1:4500/late');
await page.waitForLoadState('networkidle');
const bodies = await Promise.all(captured); // [{ url, body }, ...]
```

Keep the body promises and await them at the end. Reading the body takes another round trip to the browser, so a handler that pushes only after `await response.json()` can still be running when `waitForLoadState` returns, and the list is empty at that moment. `context.on('response')` does the same for every page of a context. Response bodies are kept in the browser within a budget (8 MiB per body, 64 MiB for the whole server); read them promptly.

**Call the API yourself.** Once you know the endpoint, `page.evaluate(() => fetch(url).then((r) => r.json()))` calls it with the page's cookies and origin. `context.request.get(url)` calls it from Node.js, outside the browser, with the cookies from the context's `storageState`.

**Change or block requests** with `page.route` (or `context.route`):

```ts
// Answer with your own JSON
await page.route('**/api/items*', (route) => route.fulfill({ json: { items: [], next: null } }));
// Fetch the real response and change it
await page.route('**/api/config', async (route) => {
  const response = await route.fetch();
  const json = await response.json();
  await route.fulfill({ response, json: { ...json, featureFlag: true } });
});
```

In the native SDK, `mock(url, options)` steps do the same inside a run: `json`, `body`, `status`, `abort`, `passthrough`, `patch` (rewrite the real response) and `hold`/`release`. The run's `requests` list holds every request the page made with its status. See [Requests and mocks](/e2e/network) and [Network, routes and HAR](/playwright/network).

## Cut what you do not need

Images, fonts and media cost bandwidth and time, for you and for the site. Block them when you only want text:

```ts
await context.route(/\.(png|jpe?g|gif|webp|avif|svg|woff2?|ttf|mp4|webm)(\?|$)/, (route) => route.abort());
```

The browser fetches what the page requests, so blocking at the route is what saves the transfer. Blocking analytics and ad hosts the same way (`route.abort()` on their URLs) also makes pages settle sooner, because their requests no longer hold up settling.

## Pagination

**Follow the "next" link** until there is none:

```ts
const BASE = 'http://127.0.0.1:4500';
const rows: string[] = [];
let url: string | null = `${BASE}/products?page=1`;
while (url) {
  await page.goto(url);
  rows.push(...(await page.locator('.product .name').allTextContents()));
  const next = page.locator('a[rel=next]');
  url = (await next.count()) ? new URL((await next.getAttribute('href'))!, page.url()).href : null;
}
```

**Click through** when there are no URLs, for example a "Load more" button or a pager driven by JavaScript:

```ts
while (await page.getByRole('button', { name: 'Load more' }).isVisible()) {
  await page.getByRole('button', { name: 'Load more' }).click(); // settles on the request it starts
}
```

**Read the API** when the pager calls one: find the request (`page.on('request')`), then call it directly with increasing `page` or `offset` parameters.

Keep a cap on the number of pages, and a delay between them for sites you do not own.

## Infinite scroll

Infinite lists load more when you scroll near the end, usually through an `IntersectionObserver` on a sentinel element. Scroll with the mouse wheel until the item count stops growing:

```ts
await page.goto('http://127.0.0.1:4500/feed');
let before = -1;
for (let i = 0; i < 50; i++) {
  const count = await page.locator('.row').count();
  if (count === before) break; // nothing new arrived: the end
  before = count;
  await page.mouse.wheel(0, 2000);
  await page.waitForLoadState('networkidle');
}
const rows = await page.locator('.row').allTextContents();
```

Scrolling the sentinel into view works too: `await page.locator('#sentinel').scrollIntoViewIfNeeded()`.

In the native SDK the whole scroll is one run. Each `mouse('wheel')` step settles on the requests it triggers:

```ts
import { css, goto, mouse } from '@povio/pyro';

const r = await page.run([
  goto('http://127.0.0.1:4500/feed'),
  ...Array.from({ length: 8 }, () => mouse('wheel', { x: 100, y: 100, deltaY: 3000 })),
  css('.row').read('rows', 'count'),
]);
// { rows: 47 }: 8 wheel steps loaded the whole feed in 58 ms
```

For a list of unknown length, run a few wheel steps at a time and repeat until the count stops changing.

## Downloads

Downloads are always accepted. Playwright library:

```ts
await page.goto('http://127.0.0.1:4500/downloads');
const [download] = await Promise.all([page.waitForEvent('download'), page.getByText('Export CSV').click()]);
console.log(download.suggestedFilename()); // items.csv
await download.saveAs('out/items.csv');
```

Native SDK, where the file's text also comes back in the reply:

```ts
import { css, download, goto } from '@povio/pyro';

const r = await page.run([
  goto('http://127.0.0.1:4500/downloads'),
  css('#dl').click(),
  download({ saveTo: 'out/dl', as: 'file' }),
]);
// r.values.file: { filename: "items.csv", path: "out/dl/items.csv", size: 650, text: "1,Item 1,3\n...", url: "..." }
```

`saveTo` is a directory on the machine running the browser. If the browser runs on another host, the file lands there; read it from the reply's `text` instead.

When a link points straight at the file, you do not need the browser: `context.request.get(url)` downloads it with the context's cookies.

## Related

- [Scripting a browser](/scraping/library): the two APIs.
- [Screenshots and PDFs](/scraping/pdf): visual output.
- [Concurrency at scale](/scraping/concurrency): running these jobs in parallel.
