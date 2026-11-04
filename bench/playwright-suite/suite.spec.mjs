// What a typical app E2E suite uses from Playwright. One test per feature, independent of each other.
import { test, expect } from './fixtures.mjs';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';

test('goto + waitForLoadState + title', async ({ page, server }) => {
  const resp = await page.goto(server + '/index.html');
  expect(resp.status()).toBe(200);
  await page.waitForLoadState('domcontentloaded');
  await page.waitForLoadState('load');
  await page.waitForLoadState('networkidle');
  await expect(page).toHaveTitle('Home');
});

test('redirect + toHaveURL', async ({ page, server }) => {
  await page.goto(server + '/redirect');
  await expect(page).toHaveURL(/form\.html$/);
});

test('locators getByRole/getByText/getByLabel/nth', async ({ page, server }) => {
  await page.goto(server + '/form.html');
  await expect(page.getByRole('button', { name: 'Save' })).toBeVisible();
  await expect(page.getByLabel('Name')).toBeVisible();
  await expect(page.getByLabel('Email')).toBeVisible();
  await expect(page.getByLabel('Notes')).toBeVisible();
  await expect(page.getByText('Banana')).toBeVisible();
  await expect(page.getByRole('listitem')).toHaveCount(3);
  await expect(page.getByRole('listitem').nth(2)).toHaveText('Cherry');
  await expect(page.getByRole('combobox', { name: 'Color' })).toHaveValue('r');
});

test('click/fill/press/check/selectOption + form submit', async ({ page, server }) => {
  await page.goto(server + '/form.html');
  await page.getByLabel('Name').fill('Ada');
  await page.getByLabel('Email').fill('ada@example.com');
  await page.getByLabel('Name').press('End');
  await page.getByLabel('I agree').check();
  await expect(page.getByLabel('I agree')).toBeChecked();
  await page.getByLabel('Color').selectOption('g');
  await page.getByLabel('Notes').pressSequentially('hi!');
  await page.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByRole('status')).toHaveText('Saved: name=Ada&email=ada%40example.com&agree=on&color=g&notes=hi%21');
});

test('link navigation via click', async ({ page, server }) => {
  await page.goto(server + '/index.html');
  await page.getByRole('link', { name: 'Go to form' }).click();
  await expect(page).toHaveURL(/form\.html/);
  await expect(page.getByRole('heading')).toHaveCount(0);
});

test('SPA hash routing + async data', async ({ page, server }) => {
  await page.goto(server + '/spa.html');
  await expect(page.getByRole('heading', { name: 'Home view' })).toBeVisible();
  await page.getByRole('button', { name: 'Load data' }).click();
  await expect(page.getByRole('listitem')).toHaveText('Item true');
  await page.getByRole('link', { name: 'About' }).click();
  await expect(page.getByRole('heading', { name: 'About view' })).toBeVisible();
  await expect(page).toHaveURL(/#about$/);
  await page.goBack();
  await expect(page.getByRole('heading', { name: 'Home view' })).toBeVisible();
});

test('route() fulfill + abort + continue with headers', async ({ page, server }) => {
  await page.route('**/api/slow', (r) => r.fulfill({ json: { slow: 'mocked' } }));
  await page.route('**/style.css', (r) => r.abort());
  let seen = '';
  await page.route('**/api/echo', async (r) => { seen = r.request().method(); await r.continue({ headers: { ...r.request().headers(), 'x-test': '1' } }); });
  await page.goto(server + '/spa.html');
  await page.getByRole('button', { name: 'Load data' }).click();
  await expect(page.getByRole('listitem')).toHaveText('Item mocked');
  const j = await page.evaluate(async () => (await fetch('/api/echo', { method: 'POST', body: 'x' })).json());
  expect(j.body).toBe('x');
  expect(seen).toBe('POST');
});

test('waitForResponse + waitForRequest + request events', async ({ page, server }) => {
  await page.goto(server + '/spa.html');
  await expect(page.getByRole('button', { name: 'Load data' })).toBeVisible();
  const reqs = [];
  page.on('request', (r) => reqs.push(r.url()));
  const [req, resp] = await Promise.all([
    page.waitForRequest('**/api/slow'),
    page.waitForResponse((r) => r.url().endsWith('/api/slow') && r.status() === 200),
    page.getByRole('button', { name: 'Load data' }).click(),
  ]);
  expect(req.method()).toBe('GET');
  expect(await resp.json()).toEqual({ slow: true });
  expect(reqs.some((u) => u.endsWith('/api/slow'))).toBe(true);
});

test('cookies: server Set-Cookie, addCookies, clearCookies', async ({ page, context, server }) => {
  await page.goto(server + '/set-cookie');
  const c = await context.cookies();
  expect(c.find((x) => x.name === 'srv')?.value).toBe('1');
  await context.addCookies([{ name: 'added', value: 'yes', url: server }]);
  const j = await page.evaluate(async () => (await fetch('/api/echo')).json());
  expect(j.cookie).toContain('added=yes');
  expect(await page.evaluate(() => document.cookie)).toContain('srv=1');
  await context.clearCookies();
  expect(await context.cookies()).toHaveLength(0);
});

test('storageState save + restore into new context', async ({ browser, server }) => {
  const ctx1 = await browser.newContext();
  const p1 = await ctx1.newPage();
  await p1.goto(server + '/set-cookie');
  await p1.evaluate(() => localStorage.setItem('token', 'abc'));
  const state = await ctx1.storageState();
  await ctx1.close();
  expect(state.cookies.some((c) => c.name === 'srv')).toBe(true);
  expect(state.origins[0]?.localStorage.find((x) => x.name === 'token')?.value).toBe('abc');
  const ctx2 = await browser.newContext({ storageState: state });
  const p2 = await ctx2.newPage();
  await p2.goto(server + '/index.html');
  expect(await p2.evaluate(() => localStorage.getItem('token'))).toBe('abc');
  expect(await p2.evaluate(() => document.cookie)).toContain('srv=1');
  await ctx2.close();
});

test('multiple isolated contexts', async ({ browser, server }) => {
  const a = await browser.newContext();
  const b = await browser.newContext();
  const pa = await a.newPage();
  const pb = await b.newPage();
  await pa.goto(server + '/index.html');
  await pb.goto(server + '/index.html');
  await pa.evaluate(() => localStorage.setItem('who', 'a'));
  expect(await pb.evaluate(() => localStorage.getItem('who'))).toBeNull();
  await a.close();
  await b.close();
});

test('popup via window.open + target=_blank', async ({ page, context, server }) => {
  await page.goto(server + '/popup.html');
  const [popup] = await Promise.all([page.waitForEvent('popup'), page.getByRole('button', { name: 'Popup' }).click()]);
  await popup.waitForLoadState();
  await expect(popup).toHaveTitle('Home');
  const [tab] = await Promise.all([context.waitForEvent('page'), page.getByRole('link', { name: 'Open new tab' }).click()]);
  await tab.waitForLoadState();
  await expect(tab).toHaveTitle('Form');
});

test('dialogs alert/confirm/prompt', async ({ page, server }) => {
  await page.goto(server + '/dialogs.html');
  const types = [];
  page.on('dialog', async (d) => { types.push(d.type()); d.type() === 'prompt' ? await d.accept('Bob') : d.type() === 'confirm' ? await d.dismiss() : await d.accept(); });
  await page.getByRole('button', { name: 'Alert' }).click();
  await page.getByRole('button', { name: 'Confirm' }).click();
  await expect(page.locator('#c')).toHaveText('false');
  await page.getByRole('button', { name: 'Prompt' }).click();
  await expect(page.locator('#p')).toHaveText('Bob');
  expect(types).toEqual(['alert', 'confirm', 'prompt']);
});

test('file upload setInputFiles + multipart post', async ({ page, server }) => {
  await page.goto(server + '/upload.html');
  await page.locator('#file').setInputFiles({ name: 'a.txt', mimeType: 'text/plain', buffer: Buffer.from('12345') });
  await expect(page.locator('#sel')).toHaveText('a.txt:5');
  await page.getByRole('button', { name: 'Upload' }).click();
  await expect(page.locator('#r')).toContainText('received');
});

test('download event + saveAs', async ({ page, server }) => {
  await page.goto(server + '/download.html');
  const [dl] = await Promise.all([page.waitForEvent('download'), page.getByRole('link', { name: 'Download file' }).click()]);
  expect(dl.suggestedFilename()).toBe('hello.txt');
  const p = path.join(os.tmpdir(), `dl-${Date.now()}.txt`);
  await dl.saveAs(p);
  expect(fs.readFileSync(p, 'utf8')).toBe('hello download');
});

test('iframes frameLocator + frame()', async ({ page, server }) => {
  await page.goto(server + '/frame.html');
  const f = page.frameLocator('iframe[title="inner"]');
  await f.getByLabel('Name').fill('in-frame');
  await expect(f.getByLabel('Name')).toHaveValue('in-frame');
  expect(page.frames().length).toBe(2);
  const fr = page.frame({ url: /form\.html/ });
  expect(await fr.evaluate(() => document.title)).toBe('Form');
});

test('keyboard + mouse low-level', async ({ page, server }) => {
  await page.goto(server + '/index.html');
  await page.locator('#kbd').focus();
  await page.keyboard.type('ab');
  await page.keyboard.press('Shift+KeyC');
  await expect(page.locator('#kbd')).toHaveText('abShiftC');
  await page.mouse.click(30, 30);
  expect(await page.evaluate(() => window.lastDown)).toEqual([30, 30]);
});

test('hover', async ({ page, server }) => {
  await page.goto(server + '/index.html');
  await page.getByRole('button', { name: 'Hover me' }).hover();
  await expect(page.locator('#hov')).toHaveText('hovered');
});

test('page.screenshot + element screenshot (PNG non-empty)', async ({ page, server }) => {
  await page.goto(server + '/visual.html');
  const buf = await page.screenshot();
  expect(buf.subarray(1, 4).toString()).toBe('PNG');
  const el = await page.locator('.box').screenshot();
  expect(el.length).toBeGreaterThan(100);
});

test('toHaveScreenshot visual match vs Chrome baseline', async ({ page, server }) => {
  await page.setViewportSize({ width: 320, height: 200 });
  await page.goto(server + '/visual.html');
  await expect(page).toHaveScreenshot('visual.png', { maxDiffPixelRatio: 0.02 });
});

test('page.pdf', async ({ page, server }) => {
  await page.goto(server + '/form.html');
  const pdf = await page.pdf({ format: 'A4' });
  expect(pdf.subarray(0, 4).toString()).toBe('%PDF');
});

test('tracing start/stop', async ({ context, page, server }) => {
  await context.tracing.start({ screenshots: true, snapshots: true });
  await page.goto(server + '/form.html');
  await page.getByLabel('Name').fill('t');
  const p = path.join(os.tmpdir(), `trace-${Date.now()}.zip`);
  await context.tracing.stop({ path: p });
  expect(fs.statSync(p).size).toBeGreaterThan(1000);
});

test('video recording', async ({ browser, server }) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'vid-'));
  const ctx = await browser.newContext({ recordVideo: { dir, size: { width: 320, height: 240 } } });
  const p = await ctx.newPage();
  await p.goto(server + '/spa.html');
  await p.waitForTimeout(500);
  const v = p.video();
  await ctx.close();
  const f = await v.path();
  expect(fs.statSync(f).size).toBeGreaterThan(1000);
});

test('emulation viewport/locale/timezone/colorScheme/userAgent', async ({ browser, server }) => {
  const ctx = await browser.newContext({ viewport: { width: 400, height: 300 }, locale: 'de-DE', timezoneId: 'Asia/Tokyo', colorScheme: 'dark', userAgent: 'MyUA/1.0' });
  const p = await ctx.newPage();
  await p.goto(server + '/env.html');
  const e = JSON.parse(await p.locator('#e').textContent());
  const errs = [];
  if (e.w !== 400 || e.h !== 300) errs.push(`viewport ${e.w}x${e.h}`);
  if (e.lang !== 'de-DE') errs.push(`locale ${e.lang}`);
  if (e.tz !== 'Asia/Tokyo') errs.push(`tz ${e.tz}`);
  if (!e.dark) errs.push('colorScheme not dark');
  if (e.ua !== 'MyUA/1.0') errs.push(`ua ${e.ua}`);
  await ctx.close();
  expect(errs).toEqual([]);
});

test('emulation mobile device (isMobile/hasTouch/dpr)', async ({ browser, server }) => {
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true });
  const p = await ctx.newPage();
  await p.goto(server + '/env.html');
  const e = JSON.parse(await p.locator('#e').textContent());
  await ctx.close();
  expect({ w: e.w, dpr: e.dpr, touch: e.touch }).toEqual({ w: 390, dpr: 3, touch: true });
});

test('geolocation + permissions', async ({ browser, server }) => {
  const ctx = await browser.newContext({ geolocation: { latitude: 46.05, longitude: 14.5 }, permissions: ['geolocation'] });
  const p = await ctx.newPage();
  await p.goto(server + '/index.html');
  const pos = await p.evaluate(() => new Promise((res, rej) => navigator.geolocation.getCurrentPosition((x) => res([x.coords.latitude, x.coords.longitude]), (e) => rej(new Error(e.message)), { timeout: 5000 })));
  await ctx.close();
  expect(pos).toEqual([46.05, 14.5]);
});

test('addInitScript', async ({ page, server }) => {
  await page.addInitScript(() => { window.__injected = 42; });
  await page.goto(server + '/index.html');
  expect(await page.evaluate(() => window.__injected)).toBe(42);
});

test('exposeFunction', async ({ page, server }) => {
  await page.exposeFunction('sha', (s) => s.split('').reverse().join(''));
  await page.goto(server + '/index.html');
  expect(await page.evaluate(() => window.sha('abc'))).toBe('cba');
});

test('console + pageerror events', async ({ page, server }) => {
  const logs = [];
  const errors = [];
  page.on('console', (m) => logs.push(`${m.type()}:${m.text()}`));
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto(server + '/error.html');
  await expect.poll(() => errors.length).toBeGreaterThan(0);
  expect(logs).toContain('error:boom-log');
  expect(errors[0]).toContain('boom-uncaught');
});

test('web workers (page.workers + worker message)', async ({ page, server }) => {
  const wp = page.waitForEvent('worker', { timeout: 5000 }).catch(() => null);
  await page.goto(server + '/worker.html');
  await expect(page.locator('#w')).toHaveText('worker:42');
  const w = await wp;
  expect(w, 'worker event').not.toBeNull();
  expect(w.url()).toContain('worker.js');
});

test('evaluate / evaluateHandle / $$eval', async ({ page, server }) => {
  await page.goto(server + '/form.html');
  expect(await page.evaluate(() => 1 + 1)).toBe(2);
  const h = await page.evaluateHandle(() => document.body);
  expect(await h.evaluate((b) => b.tagName)).toBe('BODY');
  expect(await page.$$eval('li', (els) => els.map((e) => e.textContent))).toEqual(['Apple', 'Banana', 'Cherry']);
});

test('setExtraHTTPHeaders + request API', async ({ page, context, server }) => {
  await page.setExtraHTTPHeaders({ 'accept-language': 'fr-FR' });
  await page.goto(server + '/index.html');
  const j = await page.evaluate(async () => (await fetch('/api/echo')).json());
  expect(j.lang).toBe('fr-FR');
  const r = await context.request.post(server + '/api/echo', { data: 'q' });
  expect((await r.json()).body).toBe('q');
});

test('big page: locate + click in 2000-row table', async ({ page, server }) => {
  await page.goto(server + '/big.html');
  await page.getByRole('button', { name: 'btn 1999' }).click();
  await expect(page.getByRole('row')).toHaveCount(2000);
});

test('reload + goBack/goForward', async ({ page, server }) => {
  await page.goto(server + '/index.html');
  await page.goto(server + '/form.html');
  await page.goBack();
  await expect(page).toHaveTitle('Home');
  await page.goForward();
  await expect(page).toHaveTitle('Form');
  await page.reload();
  await expect(page).toHaveTitle('Form');
});

test('inline on* handler attributes (onclick="...")', async ({ page, server }) => {
  await page.goto(server + '/inline.html');
  await page.getByRole('button', { name: 'Inline' }).click();
  await expect(page.locator('#o')).toHaveText('inline');
});

test('named element access (window[id])', async ({ page, server }) => {
  await page.goto(server + '/inline.html');
  expect(await page.evaluate(() => window.named)).toBe('yes');
});

test('click (addEventListener)', async ({ page, server }) => {
  await page.goto(server + '/index.html');
  await page.getByRole('button', { name: 'Click me' }).click();
  await expect(page.locator('#out')).toHaveText('clicked');
});

test('page.content() on a page with a doctype', async ({ page, server }) => {
  await page.goto(server + '/form.html');
  const html = await page.content();
  expect(html.startsWith('<!DOCTYPE html>')).toBe(true);
  expect(html).toContain('<title>Form</title>');
});
