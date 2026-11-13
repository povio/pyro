// A popup has all of its context's settings from its first document on,
// whether `window.open` or a `target=_blank` link opened it, and an iframe
// has its page's. What the first inline script of the popup (or frame) saw,
// and what the server saw of its document request, is what stock Playwright
// 1.59.1 gives on its Chromium (147) for the same pages.
import { createServer, type IncomingHttpHeaders, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, isStock, test } from './fixtures';
import type { BrowserContext, Page } from '../../src/wright/index';

/** Requests the site saw, by path (with the query). */
const seen: { path: string; headers: IncomingHttpHeaders }[] = [];
const headersOf = (path: string) => seen.find((s) => s.path === path)?.headers;
let site: Server;
let SITE = '';

/** The first script of a document: what it sees of the settings. */
const FIRST = `<script nonce="abc">
window.first = {
  init: window.__init ?? null,
  binding: typeof window.hello,
  language: navigator.language,
  timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
  userAgent: navigator.userAgent,
  width: innerWidth,
  height: innerHeight,
  dpr: devicePixelRatio,
  orientation: screen.orientation.type,
  touch: navigator.maxTouchPoints,
  dark: matchMedia('(prefers-color-scheme: dark)').matches,
  reducedMotion: matchMedia('(prefers-reduced-motion: reduce)').matches,
  nonce: (() => { const d = document.createElement('div'); d.setAttribute('nonce', 'abc'); document.documentElement.append(d); return d.getAttribute('nonce'); })(),
};
navigator.geolocation.getCurrentPosition((p) => { window.position = [p.coords.latitude, p.coords.longitude]; }, (e) => { window.position = e.code; });
</script>`;

test.beforeAll(async () => {
  site = createServer((req, res) => {
    const url = new URL(req.url!, 'http://site');
    seen.push({ path: url.pathname + url.search, headers: req.headers });
    const html = { 'content-type': 'text/html' };
    if (url.pathname === '/opener') {
      // A link to `to`, and a frame unless `frame` is empty.
      const frame = url.searchParams.get('frame') ?? '/frame';
      res.writeHead(200, html).end(`<!doctype html><a id="link" href="${url.searchParams.get('to') ?? '/popup'}" target="_blank">open</a>${frame ? `<iframe src="${frame}"></iframe>` : ''}`);
    } else if (url.pathname === '/popup' || url.pathname === '/frame') {
      // Nonces of a header-delivered policy are hidden unless bypassCSP. (The
      // viewport meta keeps Chromium's isMobile layout at the viewport's width.)
      res
        .writeHead(200, { ...html, 'content-security-policy': "script-src 'self' 'nonce-abc'" })
        .end(`<!doctype html><meta name="viewport" content="width=device-width"><title>${url.pathname}</title>${FIRST}`);
    } else if (url.pathname === '/secret') {
      if (req.headers.authorization === `Basic ${Buffer.from('u:p').toString('base64')}`) res.writeHead(200, html).end('<title>secret ok</title>');
      else res.writeHead(401, { ...html, 'www-authenticate': 'Basic realm="site"' }).end('<title>denied</title>');
    } else if (url.pathname === '/scripted') {
      res.writeHead(200, html).end('<!doctype html><title>no script</title><script>document.title = "script ran"</script>');
    } else res.writeHead(200, html).end(`<!doctype html><title>${url.pathname}</title>`);
  });
  await new Promise<void>((resolve) => site.listen(0, '127.0.0.1', resolve));
  SITE = `http://127.0.0.1:${(site.address() as AddressInfo).port}`;
});

test.afterAll(() => {
  site.closeAllConnections();
  site.close();
});

/** Opens `path` from `page` (on `/opener?to=path`) as a popup, `via` `window.open` or a link; the popup once announced. */
async function popup(page: Page, path: string, via: 'window.open' | 'target=_blank'): Promise<Page> {
  const announced = page.waitForEvent('popup');
  if (via === 'window.open') await page.evaluate((p) => void window.open(p), path);
  else await page.locator('#link').click();
  return announced;
}

const SETTINGS = {
  locale: 'de-DE',
  timezoneId: 'Asia/Tokyo',
  userAgent: 'PopupAgent/1.0',
  extraHTTPHeaders: { 'x-extra': 'yes' },
  viewport: { width: 400, height: 500 },
  deviceScaleFactor: 2,
  isMobile: true,
  hasTouch: true,
  colorScheme: 'dark' as const,
  reducedMotion: 'reduce' as const,
  bypassCSP: true,
  geolocation: { latitude: 46, longitude: 14 },
  permissions: ['geolocation'],
};
const SEEN = {
  init: 'yes',
  binding: 'function',
  language: 'de-DE',
  timeZone: 'Asia/Tokyo',
  userAgent: 'PopupAgent/1.0',
  width: 400,
  height: 500,
  dpr: 2,
  // isMobile with a portrait screen (a desktop one is landscape).
  orientation: 'portrait-primary',
  touch: 1,
  dark: true,
  reducedMotion: true,
  nonce: 'abc',
};

/** A context with every setting a page gets on its own, an init script, a binding, a route and request events. */
async function configured(browser: { newContext(o: object): Promise<BrowserContext> }) {
  const context = await browser.newContext(SETTINGS);
  await context.addInitScript(() => void ((window as any).__init = 'yes'));
  await context.exposeFunction('hello', () => 'hi');
  await context.route('**/routed*', (route) => route.fulfill({ contentType: 'text/html', body: '<!doctype html><script>window.first = "routed"</script>' }));
  const requested: string[] = [];
  context.on('request', (r) => requested.push(r.url()));
  return { context, requested };
}

for (const via of ['window.open', 'target=_blank'] as const) {
  test(`a popup ${via} opened has its context's settings from its first document`, async ({ browser }) => {
    const { context, requested } = await configured(browser);
    const page = await context.newPage();
    const target = `/popup?via=${encodeURIComponent(via)}`;
    await page.goto(`${SITE}/opener?frame=&to=${encodeURIComponent(target)}`);
    const opened = await popup(page, target, via);
    // Announced once its document committed, as in Playwright.
    expect(opened.url()).toBe(SITE + target);
    await opened.waitForLoadState();
    expect(await opened.evaluate(() => (window as any).first)).toEqual(SEEN);
    await expect.poll(() => opened.evaluate(() => (window as any).position)).toEqual([46, 14]);
    // Its document request: the user agent, the extra header, the locale; seen by the context's listeners.
    expect(headersOf(target)).toMatchObject({ 'user-agent': 'PopupAgent/1.0', 'x-extra': 'yes', 'accept-language': 'de-DE' });
    expect(requested).toContain(SITE + target);

    // A route answers the popup's own document.
    const routedPath = `/routed?via=${encodeURIComponent(via)}`;
    await page.goto(`${SITE}/opener?frame=&to=${encodeURIComponent(routedPath)}`);
    const routed = await popup(page, routedPath, via);
    await routed.waitForLoadState();
    expect(await routed.evaluate(() => (window as any).first)).toBe('routed');
    expect(seen.some((s) => s.path === routedPath)).toBe(false);
    await context.close();
  });
}

test('an iframe has its page settings, its document request and routes included', async ({ browser }) => {
  const { context, requested } = await configured(browser);
  const page = await context.newPage();
  await page.goto(`${SITE}/opener?frame=/frame`);
  const frame = page.frames().find((f) => f.url() === `${SITE}/frame`)!;
  // The frame's own viewport is its box (Chromium: 300x150 too).
  expect(await frame.evaluate(() => (window as any).first)).toEqual({ ...SEEN, width: 300, height: 150 });
  expect(headersOf('/frame')).toMatchObject({ 'user-agent': 'PopupAgent/1.0', 'x-extra': 'yes', 'accept-language': 'de-DE' });
  expect(requested).toContain(`${SITE}/frame`);
  // (Encoded, so that the route does not match the opener's own URL.)
  await page.goto(`${SITE}/opener?frame=${encodeURIComponent('/routed-frame')}`);
  const routed = page.frames().find((f) => f.url() === `${SITE}/routed-frame`)!;
  expect(await routed.evaluate(() => (window as any).first)).toBe('routed');
  expect(seen.some((s) => s.path === '/routed-frame')).toBe(false);
  await context.close();
});

test('httpCredentials, javaScriptEnabled and offline reach a popup first document', async ({ browser }) => {
  // Its document's 401 challenge is answered (before, the popup showed "denied").
  const credentials = await browser.newContext({ httpCredentials: { username: 'u', password: 'p' } });
  const page = await credentials.newPage();
  await page.goto(`${SITE}/opener?frame=&to=/secret`);
  for (const via of ['window.open', 'target=_blank'] as const) {
    const opened = await popup(page, '/secret', via);
    await opened.waitForLoadState();
    expect(await opened.title()).toBe('secret ok');
  }
  await credentials.close();

  // No script of its own runs (`evaluate` still does, as in Playwright; the
  // link is clicked by an event, the page's scripts being off).
  const noScript = await browser.newContext({ javaScriptEnabled: false });
  const quiet = await noScript.newPage();
  await quiet.goto(`${SITE}/opener?frame=&to=/scripted`);
  for (const via of ['window.open', 'target=_blank'] as const) {
    const announced = quiet.waitForEvent('popup');
    if (via === 'window.open') await quiet.evaluate(() => void window.open('/scripted'));
    else await quiet.locator('#link').dispatchEvent('click');
    const scripted = await announced;
    await scripted.waitForLoadState();
    expect(await scripted.title()).toBe('no script');
  }
  await noScript.close();

  // Offline: its document is never requested. Chromium commits an error
  // page for the failed navigation, which Playwright's `popup` event waits
  // for; pyro commits none, so stock Playwright over CDP never announces
  // this popup (wright does once the navigation ended).
  if (isStock) return;
  const offline = await browser.newContext();
  const page2 = await offline.newPage();
  await page2.goto(`${SITE}/opener?frame=&to=/offline-link`);
  await offline.setOffline(true);
  for (const [via, path] of [['window.open', '/offline-open'], ['target=_blank', '/offline-link']] as const) {
    const opened = await popup(page2, path, via);
    await opened.waitForLoadState().catch(() => {});
    expect(seen.some((s) => s.path === path)).toBe(false);
  }
  await offline.close();
});
