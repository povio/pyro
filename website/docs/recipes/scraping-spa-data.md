# Data from single-page apps

A single-page app sends an almost empty HTML page. Its scripts start after `load`, call a JSON API with `fetch`, and build the DOM from the answer. If you read the page as soon as it has loaded, you get the empty shell. This recipe shows the ways to wait for the data, how to read the API's JSON instead of the HTML, and how to page through and scroll such apps.

## The trap

`page.goto()` returns when the document fires `load`. In the Playwright library and in the native SDK's `goto` step, it does not wait for requests that the page's scripts start after that. Neither does `pyro fetch --wait-ms`. On the test site below:

```
$ pyro fetch --wait-ms 5000 http://127.0.0.1:4602/products | grep -o '<div id="app">.\{0,40\}'
<div id="app">Loading...</div><script>
```

and the first line of the script further down prints `right after goto: 0 products`. Always wait for the data itself.

## Files

```
spa/
  site.ts      test site: a products app with a client-side pager, an infinite feed
  spa.ts       the Playwright library version
  native.ts    the native SDK version
```

```sh
mkdir spa && cd spa
bun init -y
bun add @povio/pyro
```

Until `@povio/pyro` is published under that name, install it as [Installation](/guide/installation#try-today) describes.

## The test site

`/products` loads its data 200 ms after boot from `/api/products?page=N` (which answers after 150 ms) and has a **Next** button that loads the next page without changing the URL. `/feed` is an infinite list: a sentinel element at the end loads 10 more rows from `/api/feed` when it scrolls into view.

::: details site.ts
```ts
// site.ts: a single-page app that loads everything through fetch after `load`.
// bun site.ts 4602
const PORT = Number(process.argv[2] ?? 4602);
const ITEMS = Array.from({ length: 47 }, (_, i) => ({ id: i + 1, name: `Item ${i + 1}`, price: (i + 1) * 3 }));
const PAGE_SIZE = 10;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const page = (title: string, script: string) =>
  new Response(`<!doctype html><meta charset="utf-8"><title>${title}</title><div id="app">Loading...</div><script>${script}</script>`, {
    headers: { 'content-type': 'text/html' },
  });

// /products: an empty shell; the app fetches a page of JSON and has a client-side pager.
const productsApp = `
  let page = 1;
  async function show() {
    const r = await fetch('/api/products?page=' + page);
    const { items, pages } = await r.json();
    document.querySelector('#app').innerHTML =
      '<h1>Products, page ' + page + ' of ' + pages + '</h1><ul>' +
      items.map((p) => '<li class="product" data-id="' + p.id + '"><span class="name">' + p.name + '</span> <span class="price">' + p.price + '</span></li>').join('') +
      '</ul>' + (page < pages ? '<button id="next">Next</button>' : '');
    document.querySelector('#next')?.addEventListener('click', () => { page++; show(); });
  }
  setTimeout(show, 200); // the app boots, then loads its data
`;

// /feed: infinite scroll; a sentinel at the end loads the next 10 rows.
const feedApp = `
  document.querySelector('#app').innerHTML = '<ul id="rows"></ul><div id="sentinel" style="height:1px"></div>';
  let offset = 0, done = false, busy = false;
  new IntersectionObserver(async ([e]) => {
    if (!e.isIntersecting || done || busy) return;
    busy = true;
    const { rows, next } = await (await fetch('/api/feed?offset=' + offset)).json();
    for (const r of rows) document.querySelector('#rows').insertAdjacentHTML('beforeend', '<li class="row" style="height:150px">' + r + '</li>');
    offset = next ?? offset; done = next === null; busy = false;
  }).observe(document.querySelector('#sentinel'));
`;

Bun.serve({
  port: PORT,
  hostname: '127.0.0.1',
  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === '/products') return page('Products', productsApp);
    if (url.pathname === '/feed') return page('Feed', feedApp);
    if (url.pathname === '/api/products') {
      await sleep(150); // a slow API
      const p = Number(url.searchParams.get('page') ?? 1);
      return Response.json({ page: p, pages: Math.ceil(ITEMS.length / PAGE_SIZE), items: ITEMS.slice((p - 1) * PAGE_SIZE, p * PAGE_SIZE) });
    }
    if (url.pathname === '/api/feed') {
      await sleep(50);
      const offset = Number(url.searchParams.get('offset') ?? 0);
      const rows = ITEMS.slice(offset, offset + 10).map((i) => i.name);
      return Response.json({ rows, next: offset + 10 < ITEMS.length ? offset + 10 : null });
    }
    return new Response('not found', { status: 404 });
  },
});
console.log(`site on http://127.0.0.1:${PORT}`);
```
:::

```sh
bun site.ts 4602
```

## With the Playwright library

Five ways to get the data, from the simplest to the most robust, plus the pager and the feed:

```ts
// spa.ts: bun spa.ts
import { chromium } from '@povio/pyro/wright';

const BASE = process.env.BASE ?? 'http://127.0.0.1:4602';
type Product = { id: number; name: string; price: number };

const browser = await chromium.launch();
try {
  const page = await browser.newPage();

  // 0. What goes wrong: goto returns at `load`, before the app fetched its data.
  await page.goto(`${BASE}/products`);
  console.log('right after goto:', await page.locator('.product').count(), 'products');

  // 1. Wait for the element the data turns into.
  await page.goto(`${BASE}/products`);
  await page.locator('.product').first().waitFor();
  console.log('1. waitFor:', (await page.locator('.product .name').allTextContents()).join(', '));

  // 2. Wait for the response and read its JSON: no HTML parsing.
  const response = page.waitForResponse((r) => r.url().includes('/api/products') && r.ok());
  await page.goto(`${BASE}/products`);
  const first = (await (await response).json()) as { page: number; pages: number; items: Product[] };
  console.log('2. response:', first.pages, 'pages,', first.items.length, 'items on page 1');

  // 3. Call the API yourself, from the page: its cookies, origin and headers.
  const all: Product[] = [...first.items];
  for (let p = 2; p <= first.pages; p++) {
    const body = await page.evaluate((u) => fetch(u).then((r) => r.json()), `/api/products?page=${p}`);
    all.push(...body.items);
  }
  console.log('3. API loop:', all.length, 'items, last', JSON.stringify(all.at(-1)));

  // 4. Click through a client-side pager: wait for the heading of the next page each time.
  await page.goto(`${BASE}/products`);
  const names: string[] = [];
  for (let n = 1; ; n++) {
    await page.getByRole('heading', { name: new RegExp(`^Products, page ${n} of`) }).waitFor();
    names.push(...(await page.locator('.product .name').allTextContents()));
    const next = page.getByRole('button', { name: 'Next' });
    if (!(await next.count())) break;
    await next.click();
  }
  console.log('4. pager:', names.length, 'items');

  // 5. Infinite scroll: wheel until the row count stops growing.
  await page.goto(`${BASE}/feed`);
  await page.locator('.row').first().waitFor();
  let before = 0;
  for (let i = 0; i < 50; i++) {
    const count = await page.locator('.row').count();
    if (count === before) break;
    before = count;
    await page.mouse.wheel(0, 5000);
    await page.waitForLoadState('networkidle');
  }
  console.log('5. feed:', before, 'rows');
} finally {
  await browser.close();
}
```

```
$ bun spa.ts
pyro: CDP server listening on ws://127.0.0.1:54338/
right after goto: 0 products
1. waitFor: Item 1, Item 2, Item 3, Item 4, Item 5, Item 6, Item 7, Item 8, Item 9, Item 10
2. response: 5 pages, 10 items on page 1
3. API loop: 47 items, last {"id":47,"name":"Item 47","price":141}
4. pager: 47 items
5. feed: 47 rows
```

Which one to use:

| Way | Use it when |
| --- | --- |
| 1. Wait for the element | You want what the page shows. A locator read or `waitFor()` waits up to the timeout (`page.setDefaultTimeout`). |
| 2. Wait for the response | The API's JSON has what you need, often more fields than the page shows. Start `waitForResponse` **before** `goto` or the click, or you can miss the response. |
| 3. Call the API yourself | You know the endpoint and its parameters. `page.evaluate(() => fetch(...))` runs in the page, so it sends the page's cookies and passes CORS. It is the fastest way through many pages. |
| 4. Click through | The pager has no URLs. After each click, wait for something that proves the new page is there (here the heading with the page number), not just for any `.product`. |
| 5. Scroll | The list loads on scroll. Scroll, wait until the network is quiet, and stop when the count stops growing. |

`page.goto(url, { waitUntil: 'networkidle' })` and `page.waitForLoadState('networkidle')` also wait for the requests the page made, which covers many apps. They do not help when the app starts its request from a timer: on the test site's `/products`, which fetches 200 ms after boot, both returned with 0 products. Waiting for the element or the response does.

When the data comes from an API that needs the session, the third way is the most reliable: you see the API's status code (`401` when the session expired) instead of an empty list. The [identity pool](/recipes/scraping-identity-pool) uses that to detect expired sessions.

## With the native SDK

In the native SDK, `read` and `check` steps wait for their element, so a job loads the page and reads its data in one round trip. Clicks wait for the requests they start (they settle), so the pager is one `run`:

```ts
// native.ts: bun native.ts
import { Pyro, button, check, css, evaluate, goto, heading, idle, mouse } from '@povio/pyro';

const BASE = process.env.BASE ?? 'http://127.0.0.1:4602';
const pyro = await Pyro.launch();
try {
  await using page = await pyro.page();

  // A read step waits for its element: one round trip loads the page and reads the data.
  let r = await page.run([
    goto(`${BASE}/products`),
    heading(/^Products, page 1 of/).read('title'),
    evaluate(`[...document.querySelectorAll('.product .name')].map((e) => e.textContent)`, 'names'),
  ]);
  console.log(r.values.title, (r.values.names as string[]).length, `${r.ms.toFixed(0)} ms in the browser`);

  // The pager: each click settles on the request it starts; the check waits for the new page.
  const steps = [goto(`${BASE}/products`), heading(/^Products, page 1 of/).read('p1')];
  for (let n = 2; n <= 5; n++) steps.push(button('Next').click(), check(heading(new RegExp(`^Products, page ${n} of`))).visible());
  steps.push(css('.product').read('lastPageCount', 'count'));
  r = await page.run(steps);
  console.log('pager:', r.values.lastPageCount, 'items on the last page,', r.requests.filter((q) => q.url.includes('/api/')).length, 'API calls');

  // Infinite scroll: a few wheel steps per run until the count stops growing.
  r = await page.run([goto(`${BASE}/feed`), idle(), css('.row').read('rows', 'count')]);
  let rows = r.values.rows as number;
  for (;;) {
    r = await page.run([...Array.from({ length: 3 }, () => mouse('wheel', { x: 100, y: 100, deltaY: 5000 })), idle(), css('.row').read('rows', 'count')]);
    if (r.values.rows === rows) break;
    rows = r.values.rows as number;
  }
  console.log('feed:', rows, 'rows');
} finally {
  pyro.close();
}
```

```
$ bun native.ts
pyro: CDP server listening on ws://127.0.0.1:54435/
Products, page 1 of 5 10 414 ms in the browser
pager: 7 items on the last page, 5 API calls
feed: 47 rows
```

The first run takes about 400 ms in the browser because the `heading` read waits for the app's timer and its API call. `idle()` waits until the page is quiet, with no request in flight; use it after `goto` when there is no single element to wait for. The run's `requests` list shows every request the page made, with its status.

## Things to keep

- **Cap every loop.** A pager or feed that never ends (or a bug in your stop condition) should not run forever. The scroll loop above stops after 50 rounds.
- **Wait for proof of the new state**, not for any element: after **Next**, the old `.product` elements are still there until the app replaces them.
- **Rate-limit.** Calling an API in a tight loop is fast for you and heavy for the site. Add a delay for sites you do not own; see [Scraping responsibly](/scraping/overview#scraping-responsibly).

## Next

- More extraction patterns (locators, `ariaSnapshot`, downloads, blocking images): [Extracting data](/scraping/extracting).
- The waiting rules in full: [Settling and auto-waiting](/guide/settling).
