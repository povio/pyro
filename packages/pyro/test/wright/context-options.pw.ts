// Context options as Playwright honours them (proxy, locale, timezoneId,
// geolocation and permissions, httpCredentials, offline, device emulation,
// javaScriptEnabled, ignoreHTTPSErrors), files written into directories that
// do not exist yet, and the response `page.goto()` returns. Local servers on
// free ports: a site, a second origin, and two HTTP proxies (one asks for
// credentials).
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createServer, request as forward, type IncomingHttpHeaders, type Server } from 'node:http';
import { createServer as createHttpsServer } from 'node:https';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, isStock, test } from './fixtures';

type Seen = { url: string; headers: IncomingHttpHeaders };
const seen: Record<'site' | 'other' | 'proxy', Seen[]> = { site: [], other: [], proxy: [] };
const basic = (user: string, password: string) => `Basic ${Buffer.from(`${user}:${password}`).toString('base64')}`;
const authorizations = (list: Seen[], url: string) => list.filter((s) => s.url === url).map((s) => s.headers.authorization ?? null);

const PAGES: Record<string, string> = {
  '/scripted': `<!doctype html><body onload="window.loaded = 1"><script>window.ran = 1</script>
<noscript><p id="ns">no script</p></noscript><button onclick="window.clicked = 1">Go</button>`,
};

let site: Server, other: Server, proxy: Server, authProxy: Server;
/** `http://127.0.0.1:port` of the site and the other origin; `127.0.0.1:port` of the proxies. */
let SITE = '', OTHER = '', PROXY = '', AUTH_PROXY = '';

async function listen(server: Server): Promise<string> {
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  return `127.0.0.1:${(server.address() as AddressInfo).port}`;
}

/** Forwards absolute-form requests (plain HTTP); with `credentials`, only requests that carry them. */
function forwardingProxy(credentials?: string): Server {
  return createServer((req, res) => {
    seen.proxy.push({ url: req.url!, headers: req.headers });
    if (credentials && req.headers['proxy-authorization'] !== credentials) {
      res.writeHead(407, { 'proxy-authenticate': 'Basic realm="proxy"', 'content-type': 'text/plain' }).end('proxy credentials needed');
      return;
    }
    let target: URL;
    try {
      target = new URL(req.url!);
    } catch {
      res.writeHead(400).end();
      return;
    }
    const headers = { ...req.headers };
    delete headers['proxy-authorization'];
    const upstream = forward({ host: target.hostname, port: target.port, path: target.pathname + target.search, method: req.method, headers }, (r) => {
      res.writeHead(r.statusCode!, { ...r.headers, via: 'test-proxy' });
      r.pipe(res);
    });
    upstream.on('error', () => res.writeHead(502).end());
    req.pipe(upstream);
  });
}

test.beforeAll(async () => {
  site = createServer((req, res) => {
    seen.site.push({ url: req.url!, headers: req.headers });
    const path = new URL(req.url!, 'http://site').pathname;
    if (path === '/secret') {
      if (req.headers.authorization === basic('u', 'p')) res.writeHead(200, { 'content-type': 'text/html' }).end('<p>secret ok</p>');
      else res.writeHead(401, { 'www-authenticate': 'Basic realm="site"', 'content-type': 'text/plain' }).end('denied');
      return;
    }
    if (path === '/redirect') return void res.writeHead(302, { location: '/final' }).end();
    if (path === '/json') return void res.writeHead(200, { 'content-type': 'application/json' }).end('{"ok":true}');
    if (path.endsWith('.js')) return void res.writeHead(200, { 'content-type': 'text/javascript' }).end(`function ${path.slice(1, -3)}() { return 1; } ${path.slice(1, -3)}();`);
    if (path === '/scripts') {
      res.writeHead(200, { 'content-type': 'text/html' }).end('<!doctype html><script src="/one.js"></script><script src="/two.js"></script><p>scripts</p>');
      return;
    }
    res.writeHead(200, { 'content-type': 'text/html', 'x-doc': path, 'set-cookie': 'sid=1; Path=/' }).end(PAGES[path] ?? `<!doctype html><title>${path}</title><p>${path}</p>`);
  });
  SITE = `http://${await listen(site)}`;
  other = createServer((req, res) => {
    seen.other.push({ url: req.url!, headers: req.headers });
    res.writeHead(401, { 'www-authenticate': 'Basic realm="other"', 'content-type': 'text/plain' }).end('other origin');
  });
  OTHER = `http://${await listen(other)}`;
  proxy = forwardingProxy();
  PROXY = await listen(proxy);
  authProxy = forwardingProxy(basic('pu', 'p@ss:w/rd'));
  AUTH_PROXY = await listen(authProxy);
});

test.afterAll(() => {
  for (const server of [site, other, proxy, authProxy]) {
    server.closeAllConnections();
    server.close();
  }
});

test('locale: navigator.language, Accept-Language and Intl follow it', async ({ browser }) => {
  test.skip(isStock, 'Intl follows the locale through pyro/wright');
  const context = await browser.newContext({ locale: 'de-DE' });
  const page = await context.newPage();
  await page.goto(`${SITE}/locale`);
  expect(seen.site.find((s) => s.url === '/locale')?.headers['accept-language']).toMatch(/^de-DE/);
  expect(
    await page.evaluate(() => [
      navigator.language,
      navigator.languages[0],
      Intl.DateTimeFormat().resolvedOptions().locale,
      new Intl.NumberFormat().format(1234.5),
      (1234.5).toLocaleString(),
      new Date(Date.UTC(2024, 0, 15)).toLocaleDateString(undefined, { timeZone: 'UTC' }),
    ]),
  ).toEqual(['de-DE', 'de-DE', 'de-DE', '1.234,5', '1.234,5', '15.1.2024']);
  // An explicit locale wins, and to scripts the constructors are still the built-in ones.
  expect(
    await page.evaluate(() => {
      class Mine extends Intl.NumberFormat {}
      return [
        (1234.5).toLocaleString('en-US'),
        new Intl.DateTimeFormat() instanceof Intl.DateTimeFormat,
        Intl.DateTimeFormat.name,
        Intl.DateTimeFormat.length,
        typeof Intl.DateTimeFormat.supportedLocalesOf,
        Intl.NumberFormat.prototype.constructor === Intl.NumberFormat,
        new Mine().format(1234.5),
        new Mine() instanceof Intl.NumberFormat,
      ];
    }),
  ).toEqual(['1,234.5', true, 'DateTimeFormat', 0, 'function', true, '1.234,5', true]);
  await context.close();
});

test('timezoneId: contexts in different zones at the same time', async ({ browser }) => {
  test.skip(isStock, 'a zone per context needs a connection per zone (pyro/wright)');
  const read = () => {
    const noon = new Date(Date.UTC(2024, 0, 15, 12));
    return [noon.getTimezoneOffset(), noon.getHours(), Intl.DateTimeFormat().resolvedOptions().timeZone];
  };
  const host = await browser.newContext();
  const hostPage = await host.newPage();
  await hostPage.goto(`${SITE}/zone-host`);
  const hostZone = await hostPage.evaluate(read);
  const tokyo = await browser.newContext({ timezoneId: 'Asia/Tokyo' });
  const newYork = await browser.newContext({ timezoneId: 'America/New_York' });
  const [tokyoPage, newYorkPage] = await Promise.all([tokyo.newPage(), newYork.newPage()]);
  await Promise.all([tokyoPage.goto(`${SITE}/zone-tokyo`), newYorkPage.goto(`${SITE}/zone-new-york`)]);
  expect(await tokyoPage.evaluate(read)).toEqual([-540, 21, 'Asia/Tokyo']);
  expect(await newYorkPage.evaluate(read)).toEqual([300, 7, 'America/New_York']);
  // A context without one keeps the host's zone meanwhile.
  await hostPage.reload();
  expect(await hostPage.evaluate(read)).toEqual(hostZone);
  // A second context in an open zone runs in it too.
  const tokyo2 = await browser.newContext({ timezoneId: 'Asia/Tokyo' });
  const tokyo2Page = await tokyo2.newPage();
  await tokyo2Page.goto(`${SITE}/zone-tokyo-2`);
  expect(await tokyo2Page.evaluate(read)).toEqual([-540, 21, 'Asia/Tokyo']);
  for (const context of [host, tokyo, newYork, tokyo2]) await context.close();
  // An unknown zone fails the page's first call.
  const nowhere = await browser.newContext({ timezoneId: 'Mars/Olympus_Mons' });
  await expect((await nowhere.newPage()).goto(`${SITE}/zone-mars`)).rejects.toThrow(/Invalid timezone id: Mars\/Olympus_Mons/);
  await nowhere.close();
});

test.describe('use options', () => {
  test.use({ locale: 'fr-FR', timezoneId: 'Europe/Paris', geolocation: { latitude: 1.5, longitude: 2.5 }, permissions: ['geolocation'] });
  test('reach the page fixture', async ({ page }) => {
    test.skip(isStock, 'pyro/wright runner');
    await page.goto(`${SITE}/use-options`);
    const read = () =>
      new Promise((resolve) =>
        navigator.geolocation.getCurrentPosition(
          (p) => resolve([navigator.language, Intl.DateTimeFormat().resolvedOptions().timeZone, p.coords.latitude, p.coords.longitude]),
          (e) => resolve(e.code),
        ),
      );
    expect(await page.evaluate(read)).toEqual(['fr-FR', 'Europe/Paris', 1.5, 2.5]);
  });
});

test('geolocation: with the permission granted, else denied', async ({ browser }) => {
  const position = () =>
    new Promise((resolve) =>
      navigator.geolocation.getCurrentPosition(
        (p) => resolve([p.coords.latitude, p.coords.longitude, p.coords.accuracy]),
        (e) => resolve(e.code),
        { timeout: 5000 },
      ),
    );
  const granted = await browser.newContext({ geolocation: { latitude: 46.05, longitude: 14.51 }, permissions: ['geolocation'] });
  const page = await granted.newPage();
  await page.goto(`${SITE}/geo`);
  expect(await page.evaluate(position)).toEqual([46.05, 14.51, 0]);
  expect(await page.evaluate(() => navigator.permissions.query({ name: 'geolocation' }).then((s) => s.state))).toBe('granted');
  await granted.setGeolocation({ latitude: 48.2, longitude: 16.37, accuracy: 10 });
  expect(await page.evaluate(position)).toEqual([48.2, 16.37, 10]);
  await granted.clearPermissions();
  expect(await page.evaluate(position)).toBe(1); // PERMISSION_DENIED

  const denied = await browser.newContext({ geolocation: { latitude: 46.05, longitude: 14.51 } });
  const deniedPage = await denied.newPage();
  await deniedPage.goto(`${SITE}/geo-denied`);
  expect(await deniedPage.evaluate(position)).toBe(1);
  await denied.grantPermissions(['geolocation'], { origin: SITE });
  expect(await deniedPage.evaluate(position)).toEqual([46.05, 14.51, 0]);
  await expect(denied.grantPermissions(['teleportation'])).rejects.toThrow('Unknown permission: teleportation');
  await expect(browser.newContext({ geolocation: { latitude: 100, longitude: 0 } })).rejects.toThrow('geolocation.latitude: precondition -90 <= LATITUDE <= 90 failed.');
  await granted.close();
  await denied.close();
});

test('proxy: requests go through it, with its credentials, except bypassed hosts', async ({ browser }) => {
  // `host:port`, loopback included (as Playwright, which lists `<-loopback>`).
  const context = await browser.newContext({ proxy: { server: PROXY } });
  const res = await (await context.newPage()).goto(`${SITE}/via-proxy`);
  expect(res!.headers()['via']).toBe('test-proxy');
  expect(seen.proxy.map((s) => s.url)).toContain(`${SITE}/via-proxy`);
  await context.close();

  const authed = await browser.newContext({ proxy: { server: `http://${AUTH_PROXY}`, username: 'pu', password: 'p@ss:w/rd' } });
  const authedPage = await authed.newPage();
  expect((await authedPage.goto(`${SITE}/via-auth-proxy`))!.status()).toBe(200);
  await expect(authedPage.locator('p')).toHaveText('/via-auth-proxy');
  // pyro sends them from the first request; Chrome after the proxy's 407.
  const hops = seen.proxy.filter((s) => s.url === `${SITE}/via-auth-proxy`).map((s) => s.headers['proxy-authorization']);
  expect(hops).toContain(basic('pu', 'p@ss:w/rd'));
  // The proxy's credentials are the proxy's: the site gets none.
  expect(authorizations(seen.site, '/via-auth-proxy')).toEqual([null]);
  await authed.close();

  // Without credentials the proxy turns the request away.
  const anonymous = await browser.newContext({ proxy: { server: `http://${AUTH_PROXY}` } });
  const turnedAway = await (await anonymous.newPage()).goto(`${SITE}/via-auth-proxy-anonymous`);
  expect(turnedAway!.status()).toBe(407);
  expect(seen.site.some((s) => s.url === '/via-auth-proxy-anonymous')).toBe(false);
  await anonymous.close();

  const bypassing = await browser.newContext({ proxy: { server: PROXY, bypass: '127.0.0.1' } });
  await (await bypassing.newPage()).goto(`${SITE}/bypassed`);
  expect(seen.proxy.some((s) => s.url.endsWith('/bypassed'))).toBe(false);
  expect(seen.site.some((s) => s.url === '/bypassed')).toBe(true);
  await bypassing.close();
});

test('httpCredentials answer a 401 challenge, and only their origin gets them', async ({ browser }) => {
  const context = await browser.newContext({ httpCredentials: { username: 'u', password: 'p', origin: SITE } });
  const page = await context.newPage();
  expect((await page.goto(`${SITE}/secret?page`))!.status()).toBe(200);
  await expect(page.locator('p')).toHaveText('secret ok');
  // Sent in answer to the challenge, not before it.
  expect(authorizations(seen.site, '/secret?page')).toEqual([null, basic('u', 'p')]);
  // Another origin's challenge gets nothing.
  expect((await page.goto(`${OTHER}/x?page`))!.status()).toBe(401);
  expect(authorizations(seen.other, '/x?page')).toEqual([null]);
  // context.request: after the 401 too, and never to the other origin.
  expect((await context.request.get(`${SITE}/secret?api`)).status()).toBe(200);
  expect(authorizations(seen.site, '/secret?api')).toEqual([null, basic('u', 'p')]);
  expect((await context.request.get(`${OTHER}/x?api`)).status()).toBe(401);
  expect(authorizations(seen.other, '/x?api')).toEqual([null]);
  await context.close();

  // Wrong credentials: one try, then the 401 stands.
  const wrong = await browser.newContext({ httpCredentials: { username: 'u', password: 'wrong' } });
  expect((await (await wrong.newPage()).goto(`${SITE}/secret?wrong`))!.status()).toBe(401);
  expect(authorizations(seen.site, '/secret?wrong')).toEqual([null, basic('u', 'wrong')]);
  await wrong.close();

  // send: 'always' sends them with the API request itself.
  const always = await browser.newContext({ httpCredentials: { username: 'u', password: 'p', send: 'always' } });
  expect((await always.request.get(`${SITE}/secret?always`)).status()).toBe(200);
  expect(authorizations(seen.site, '/secret?always')).toEqual([basic('u', 'p')]);
  await always.close();
});

test('offline: requests fail until setOffline(false)', async ({ browser }) => {
  const context = await browser.newContext({ offline: true });
  const page = await context.newPage();
  await expect(page.goto(`${SITE}/offline`)).rejects.toThrow();
  expect(seen.site.some((s) => s.url === '/offline')).toBe(false);
  await context.setOffline(false);
  await page.goto(`${SITE}/online`);
  expect(await page.evaluate(() => navigator.onLine)).toBe(true);
  await context.setOffline(true);
  expect(await page.evaluate(async () => [navigator.onLine, await fetch('/json').then(() => 'loaded', () => 'failed')])).toEqual([false, 'failed']);
  await context.close();
});

test('deviceScaleFactor, isMobile, hasTouch and screen', async ({ browser }) => {
  const device = () => [devicePixelRatio, innerWidth, innerHeight, screen.width, screen.height, screen.orientation.type, navigator.maxTouchPoints, matchMedia('(pointer: coarse)').matches];
  const phone = await browser.newContext({ viewport: { width: 300, height: 400 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true });
  const page = await phone.newPage();
  await page.goto(`${SITE}/phone`);
  // The page has no `<meta name="viewport">`: a 980 px layout viewport, as in Chromium.
  expect(await page.evaluate(device)).toEqual([2, 980, 1307, 300, 400, 'portrait-primary', 1, true]);
  // Screenshots are in device pixels (Playwright's default `scale: 'device'`).
  const png = await page.screenshot();
  expect([png.readUInt32BE(16), png.readUInt32BE(20)]).toEqual([600, 800]);
  await phone.close();

  const desk = await browser.newContext({ viewport: { width: 300, height: 400 }, screen: { width: 1000, height: 800 } });
  const deskPage = await desk.newPage();
  await deskPage.goto(`${SITE}/desk`);
  expect(await deskPage.evaluate(device)).toEqual([1, 300, 400, 1000, 800, 'landscape-primary', 0, false]);
  await desk.close();
  await expect(browser.newContext({ viewport: null, deviceScaleFactor: 2 })).rejects.toThrow('"deviceScaleFactor" option is not supported with null "viewport"');
});

test('javaScriptEnabled: false runs none of the page script', async ({ browser }) => {
  // Stock Playwright waits for animation frames in its own script world, which pyro stops too.
  test.skip(isStock, 'pyro/wright acts natively');
  const context = await browser.newContext({ javaScriptEnabled: false });
  const page = await context.newPage();
  await page.goto(`${SITE}/scripted`);
  await page.locator('button').click();
  // page.evaluate still runs, as in Playwright: the page's own script, handlers included, did not.
  expect(await page.evaluate(() => [typeof (window as any).ran, typeof (window as any).loaded, typeof (window as any).clicked])).toEqual(['undefined', 'undefined', 'undefined']);
  await expect(page.locator('#ns')).toHaveText('no script');
  await context.close();

  const scripted = await browser.newContext();
  const scriptedPage = await scripted.newPage();
  await scriptedPage.goto(`${SITE}/scripted`);
  await scriptedPage.locator('button').click();
  expect(await scriptedPage.evaluate(() => [typeof (window as any).ran, typeof (window as any).loaded, typeof (window as any).clicked])).toEqual(['number', 'number', 'number']);
  await expect(scriptedPage.locator('#ns')).toHaveCount(0);
  await scripted.close();
});

test('ignoreHTTPSErrors: a self-signed certificate loads', async ({ browser }) => {
  const dir = mkdtempSync(join(tmpdir(), 'pyro-tls-'));
  try {
    try {
      execFileSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', join(dir, 'key.pem'), '-out', join(dir, 'cert.pem'), '-days', '1', '-subj', '/CN=127.0.0.1'], { stdio: 'ignore' });
    } catch {
      test.skip(true, 'openssl makes the certificate');
    }
    const server = createHttpsServer({ key: readFileSync(join(dir, 'key.pem')), cert: readFileSync(join(dir, 'cert.pem')) }, (_req, res) => {
      res.writeHead(200, { 'content-type': 'text/html' }).end('<p>secure</p>');
    });
    const url = `https://${await listen(server as unknown as Server)}/`;
    try {
      const strict = await browser.newContext();
      await expect((await strict.newPage()).goto(url)).rejects.toThrow();
      await strict.close();
      const lenient = await browser.newContext({ ignoreHTTPSErrors: true });
      const page = await lenient.newPage();
      expect((await page.goto(url))!.status()).toBe(200);
      await expect(page.locator('p')).toHaveText('secure');
      await lenient.close();
    } finally {
      server.closeAllConnections();
      server.close();
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('paths in directories that do not exist yet are made', async ({ page }) => {
  const dir = mkdtempSync(join(tmpdir(), 'pyro-paths-'));
  try {
    await page.goto(`${SITE}/paths`);
    await page.context().storageState({ path: join(dir, 'a/b/state.json') });
    await page.screenshot({ path: join(dir, 'c/d/page.png') });
    await page.locator('p').screenshot({ path: join(dir, 'e/f/element.png') });
    await page.pdf({ path: join(dir, 'g/h/page.pdf') });
    await page.request.storageState({ path: join(dir, 'i/j/api.json') });
    for (const file of ['a/b/state.json', 'c/d/page.png', 'e/f/element.png', 'g/h/page.pdf', 'i/j/api.json']) expect(existsSync(join(dir, file)), file).toBe(true);
    expect(JSON.parse(readFileSync(join(dir, 'a/b/state.json'), 'utf8')).cookies.map((c: { name: string }) => c.name)).toContain('sid');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('goto returns the document response: status, headers, body, redirects', async ({ page }) => {
  // Without Network events (nothing listens): from the navigation's own request log.
  const final = (await page.goto(`${SITE}/redirect`))!;
  expect([final.url(), final.status(), final.headers()['x-doc']]).toEqual([`${SITE}/final`, 200, '/final']);
  expect(final.request().redirectedFrom()?.url()).toBe(`${SITE}/redirect`);
  expect(final.request().redirectedFrom()?.redirectedTo()).toBe(final.request());

  const res = (await page.goto(`${SITE}/doc`))!;
  expect([res.url(), res.status(), res.statusText(), res.ok()]).toEqual([`${SITE}/doc`, 200, 'OK', true]);
  expect(res.headers()['x-doc']).toBe('/doc');
  expect(res.headers()['content-type']).toBe('text/html');
  expect(await res.headerValue('X-Doc')).toBe('/doc');
  expect(res.request().method()).toBe('GET');
  expect(res.request().isNavigationRequest()).toBe(true);
  expect(res.request().headers()['user-agent']).toBeTruthy();
  expect(res.frame()).toBe(page.mainFrame());
  expect(await res.text()).toContain('<p>/doc</p>');

  expect(await (await page.goto(`${SITE}/json`))!.json()).toEqual({ ok: true });
});

test('goto returns the response Network events reported, with page.on("response")', async ({ page }) => {
  const responses: string[] = [];
  page.on('response', (r) => responses.push(r.url()));
  const res = (await page.goto(`${SITE}/doc-events`))!;
  expect(responses).toContain(`${SITE}/doc-events`);
  expect(res.headers()['x-doc']).toBe('/doc-events');
  expect(await res.text()).toContain('<p>/doc-events</p>');
});
