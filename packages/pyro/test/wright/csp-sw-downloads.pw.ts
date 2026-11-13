// `bypassCSP`, `serviceWorkers` and `acceptDownloads` against a local site
// (and a second origin for a cross-origin script). Each test notes what stock
// Playwright 1.59.1 with its Chromium (147) gives on the same pages.
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, isStock, test } from './fixtures';

const NOT_ACCEPTED = 'Pass { acceptDownloads: true } when you are creating your browser context.';
/** Requests the site saw, by path. */
const seen: string[] = [];
let site: Server, other: Server;
let SITE = '', OTHER = '';

const SW = `self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (e) => e.waitUntil(clients.claim()));
self.addEventListener('fetch', (e) => { if (new URL(e.request.url).pathname === '/sw-hello') e.respondWith(new Response('from sw')); });`;

/** A policy that allows the nonced script and the site's own scripts only. */
const CSP_PAGE = `<!doctype html><body><script nonce="abc">
window.nonced = 1;
try { eval('window.evaled = 1'); } catch (e) { window.evalError = e.name; }
const d = document.createElement('div'); d.id = 'inserted'; d.setAttribute('nonce', 'abc'); document.body.append(d);
</script>
<script>window.inline = 1</script>
<script src="OTHER/other.js"></script>
<button onclick="window.clicked = 1">Go</button>`;

async function listen(server: Server): Promise<string> {
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  return `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
}

test.beforeAll(async () => {
  site = createServer((req, res) => {
    const path = new URL(req.url!, 'http://site').pathname;
    seen.push(path);
    const html = { 'content-type': 'text/html' };
    if (path === '/csp') {
      res.writeHead(200, { ...html, 'content-security-policy': "script-src 'self' 'nonce-abc'" }).end(CSP_PAGE.replace('OTHER', OTHER));
    } else if (path === '/own.js') res.writeHead(200, { 'content-type': 'text/javascript' }).end('window.own = 1');
    else if (path === '/sw.js') res.writeHead(200, { 'content-type': 'text/javascript' }).end(SW);
    else if (path === '/sw-hello') res.writeHead(200, { 'content-type': 'text/plain' }).end('from network');
    else if (path === '/dl') res.writeHead(200, { 'content-type': 'application/octet-stream', 'content-disposition': 'attachment; filename=hello.txt' }).end('hello download');
    else if (path === '/dl-page') res.writeHead(200, html).end('<!doctype html><a href="/dl">Download</a>');
    else res.writeHead(200, html).end(`<!doctype html><title>${path}</title><p>${path}</p>`);
  });
  SITE = await listen(site);
  other = createServer((_req, res) => res.writeHead(200, { 'content-type': 'text/javascript' }).end('window.other = 1'));
  OTHER = await listen(other);
});

test.afterAll(() => {
  for (const server of [site, other]) {
    server.closeAllConnections();
    server.close();
  }
});

/** What the CSP page's own script did. */
const ran = () => {
  const w = window as any;
  const inserted = document.getElementById('inserted')!;
  return {
    nonced: w.nonced ?? null,
    evaled: w.evaled ?? w.evalError,
    inline: w.inline ?? null,
    other: w.other ?? null,
    clicked: w.clicked ?? null,
    added: w.added ?? null,
    own: w.own ?? null,
    nonceAttribute: inserted.getAttribute('nonce'),
    nonceProperty: inserted.nonce,
  };
};

test('bypassCSP: a page policy lifted, nonce hiding included', async ({ browser }) => {
  // pyro enforces no CSP directive: without bypassCSP the page's inline
  // script, eval, cross-origin script, inline handler and an added inline
  // script all run, where Chromium blocks every one of them ({ nonced: 1,
  // evaled: 'EvalError', inline: null, other: null, clicked: null, and
  // addScriptTag({ content }) throws "...violates the following Content
  // Security Policy directive..."). The one effect a header-delivered policy
  // has is nonce hiding, as in Chromium: the attribute of an element script
  // inserts reads back empty, the property keeps the value.
  const strict = await browser.newContext();
  const page = await strict.newPage();
  await page.goto(`${SITE}/csp`);
  await page.locator('button').click();
  await page.addScriptTag({ content: 'window.added = 1' });
  await page.addScriptTag({ url: '/own.js' });
  expect(await page.evaluate(ran)).toEqual({ nonced: 1, evaled: 1, inline: 1, other: 1, clicked: 1, added: 1, own: 1, nonceAttribute: '', nonceProperty: 'abc' });
  await strict.close();

  // With it, as in Chromium (Page.setBypassCSP): everything runs and the
  // nonce attribute stays.
  const lenient = await browser.newContext({ bypassCSP: true });
  const bypassing = await lenient.newPage();
  await bypassing.goto(`${SITE}/csp`);
  await bypassing.locator('button').click();
  await bypassing.addScriptTag({ content: 'window.added = 1' });
  await bypassing.addScriptTag({ url: '/own.js' });
  expect(await bypassing.evaluate(ran)).toEqual({ nonced: 1, evaled: 1, inline: 1, other: 1, clicked: 1, added: 1, own: 1, nonceAttribute: 'abc', nonceProperty: 'abc' });
  await lenient.close();
});

test("serviceWorkers: 'block' registers none, 'allow' keeps registration", async ({ browser }) => {
  // Chromium, both: as below.
  const allowing = await browser.newContext();
  const page = await allowing.newPage();
  await page.goto(`${SITE}/sw-page`);
  expect(await page.evaluate(() => navigator.serviceWorker.register('/sw.js').then((r) => r.scope))).toBe(`${SITE}/`);
  await page.waitForFunction(() => navigator.serviceWorker.controller !== null);
  expect(await page.evaluate(() => fetch('/sw-hello').then((r) => r.text()))).toBe('from sw');
  await allowing.close();

  const blocking = await browser.newContext({ serviceWorkers: 'block' });
  const blocked = await blocking.newPage();
  const warnings: string[] = [];
  blocked.on('console', (m) => m.type() === 'warning' && warnings.push(m.text()));
  await blocked.goto(`${SITE}/sw-page-blocked`);
  const fetched = seen.filter((p) => p === '/sw.js').length;
  expect(
    await blocked.evaluate(async () => [
      String(await navigator.serviceWorker.register('/sw.js')),
      (await navigator.serviceWorker.getRegistrations()).length,
      navigator.serviceWorker.controller,
      await fetch('/sw-hello').then((r) => r.text()),
    ]),
  ).toEqual(['undefined', 0, null, 'from network']);
  await expect.poll(() => warnings).toEqual(['Service Worker registration blocked by Playwright']);
  expect(seen.filter((p) => p === '/sw.js').length).toBe(fetched);
  if (!isStock) {
    // Playwright's block is an init script that replaces `register` on the
    // container: in Chromium the prototype's method still registers. pyro
    // blocks registration in the context, so it does not.
    expect(await blocked.evaluate(() => ServiceWorkerContainer.prototype.register.call(navigator.serviceWorker, '/sw.js').then(String))).toBe('undefined');
    expect(seen.filter((p) => p === '/sw.js').length).toBe(fetched);
  }
  await blocking.close();
});

test('acceptDownloads: false still has the download event, without the file', async ({ browser }) => {
  // Chromium gives the same: the event, the URL and name, `failure()` with
  // Playwright's message, which `path()`, `saveAs()` and `createReadStream()`
  // throw; `cancel()` and `delete()` resolve (in that order: Playwright
  // disposes of a deleted download), and the page stays.
  const denying = await browser.newContext({ acceptDownloads: false });
  const page = await denying.newPage();
  await page.goto(`${SITE}/dl-page`);
  const [download] = await Promise.all([page.waitForEvent('download'), page.locator('a').click()]);
  expect([download.url(), download.suggestedFilename()]).toEqual([`${SITE}/dl`, 'hello.txt']);
  expect(await download.failure()).toBe(NOT_ACCEPTED);
  await expect(download.path()).rejects.toThrow(NOT_ACCEPTED);
  await expect(download.saveAs(join(tmpdir(), 'pyro-never-saved.txt'))).rejects.toThrow(NOT_ACCEPTED);
  await expect(download.createReadStream()).rejects.toThrow(NOT_ACCEPTED);
  await download.cancel();
  await download.delete();
  expect(page.url()).toBe(`${SITE}/dl-page`);
  await denying.close();

  const accepting = await browser.newContext();
  const acceptingPage = await accepting.newPage();
  await acceptingPage.goto(`${SITE}/dl-page`);
  const [saved] = await Promise.all([acceptingPage.waitForEvent('download'), acceptingPage.locator('a').click()]);
  const dir = mkdtempSync(join(tmpdir(), 'pyro-accepted-'));
  try {
    expect(await saved.failure()).toBeNull();
    expect(existsSync(await saved.path())).toBe(true);
    await saved.saveAs(join(dir, 'saved.txt'));
    expect(readFileSync(join(dir, 'saved.txt'), 'utf8')).toBe('hello download');
    let streamed = '';
    for await (const chunk of await saved.createReadStream()) streamed += chunk;
    expect(streamed).toBe('hello download');
    await saved.cancel();
    expect(await saved.failure()).toBeNull();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  await accepting.close();
});
