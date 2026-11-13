// A new window whose first navigation fails (offline, connection refused)
// shows Chromium's error document, chrome-error://chromewebdata/, so
// Playwright's `popup` event (which waits for the popup's first commit)
// fires. What each test checks is what stock Playwright 1.59.1 gives on its
// Chromium 147 for the same pages. (Chromium's full build fills the error
// document with its "This site can't be reached" page and the host as title;
// chromium-headless-shell, Playwright's default headless browser, and pyro
// leave it empty.)
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, test } from './fixtures';
import type { BrowserContext, Page } from '../../src/wright/index';

const ERROR_PAGE = 'chrome-error://chromewebdata/';
/** Paths the site saw. */
const seen: string[] = [];
let site: Server;
let SITE = '';
/** A port nothing listens on. */
let REFUSED = '';

test.beforeAll(async () => {
  site = createServer((req, res) => {
    const url = new URL(req.url!, 'http://site');
    seen.push(url.pathname);
    const html = { 'content-type': 'text/html' };
    if (url.pathname === '/opener') {
      res.writeHead(200, html).end(`<!doctype html><title>opener</title><a id="link" target="_blank" href="${url.searchParams.get('to')}">link</a>`);
    } else res.writeHead(200, html).end(`<!doctype html><title>${url.pathname}</title>`);
  });
  await new Promise<void>((resolve) => site.listen(0, '127.0.0.1', resolve));
  SITE = `http://127.0.0.1:${(site.address() as AddressInfo).port}`;
  const closed = createServer();
  await new Promise<void>((resolve) => closed.listen(0, '127.0.0.1', resolve));
  REFUSED = `http://127.0.0.1:${(closed.address() as AddressInfo).port}/refused`;
  await new Promise((resolve) => closed.close(resolve));
});

test.afterAll(() => {
  site.closeAllConnections();
  site.close();
});

/** What the error document's own script sees. */
const shown = () => ({ href: location.href, origin: location.origin, readyState: document.readyState, history: history.length });
const SHOWN = { href: ERROR_PAGE, origin: 'null', readyState: 'complete', history: 1 };

/** Opens `url` from `page` (on `/opener?to=url`) `via` window.open or a link: the popup once announced. */
async function popup(page: Page, url: string, via: 'window.open' | 'target=_blank'): Promise<Page> {
  const announced = page.waitForEvent('popup');
  if (via === 'window.open') await page.evaluate((u) => void window.open(u), url);
  else await page.locator('#link').click();
  return announced;
}

/** The popup's frame as `Page.getFrameTree` reports it. */
async function frameOf(context: BrowserContext, page: Page) {
  const session = await context.newCDPSession(page);
  const { frameTree } = await session.send('Page.getFrameTree');
  await session.detach();
  return frameTree.frame;
}

for (const via of ['window.open', 'target=_blank'] as const) {
  test(`a popup ${via} opened whose navigation fails shows the error document`, async ({ browser }) => {
    // Connection refused.
    const context = await browser.newContext();
    const page = await context.newPage();
    await page.goto(`${SITE}/opener?to=${encodeURIComponent(REFUSED)}`);
    const refused = await popup(page, REFUSED, via);
    // Announced once the error document committed, as Playwright does.
    expect(refused.url()).toBe(ERROR_PAGE);
    await refused.waitForLoadState();
    expect(await refused.evaluate(shown)).toEqual(SHOWN);
    expect(await frameOf(context, refused)).toMatchObject({ url: ERROR_PAGE, unreachableUrl: REFUSED, securityOrigin: '://' });
    expect(page.url()).toBe(`${SITE}/opener?to=${encodeURIComponent(REFUSED)}`);

    // Offline: the popup's document is never requested.
    const target = `${SITE}/offline-${via === 'window.open' ? 'open' : 'link'}`;
    await page.goto(`${SITE}/opener?to=${encodeURIComponent(target)}`);
    await context.setOffline(true);
    const offline = await popup(page, target, via);
    expect(offline.url()).toBe(ERROR_PAGE);
    await offline.waitForLoadState();
    expect(await offline.evaluate(shown)).toEqual(SHOWN);
    expect(await frameOf(context, offline)).toMatchObject({ url: ERROR_PAGE, unreachableUrl: target });
    expect(seen).not.toContain(new URL(target).pathname);
    // Back online, the popup goes on (its reload fetches the URL it could not reach).
    await context.setOffline(false);
    await offline.reload();
    expect([offline.url(), await offline.title()]).toEqual([target, new URL(target).pathname]);
    await context.close();
  });
}

test("a new page's failed first navigation shows the error document; goto still fails", async ({ browser }) => {
  const context = await browser.newContext();
  const page = await context.newPage();
  await expect(page.goto(REFUSED)).rejects.toThrow();
  // Chromium commits the error document after Page.navigate's errorText.
  await expect.poll(() => page.url()).toBe(ERROR_PAGE);
  await page.waitForLoadState();
  // (Chromium's history keeps the about:blank Playwright's new page committed: 2 entries.)
  expect(await page.evaluate(shown)).toMatchObject({ href: ERROR_PAGE, origin: 'null', readyState: 'complete' });
  // A page with a document of its own: goto fails as well.
  await page.goto(`${SITE}/loaded`);
  await expect(page.goto(REFUSED)).rejects.toThrow();
  await context.setOffline(true);
  await expect(page.goto(`${SITE}/never`)).rejects.toThrow();
  expect(seen).not.toContain('/never');
  await context.setOffline(false);
  await page.goto(`${SITE}/again`);
  expect(await page.title()).toBe('/again');
  await context.close();
});
