// Playwright (connectOverCDP) against `lightningpanda serve --prepare URL`:
// a page booted before the test exists is handed over by
// Lightningpanda.acquirePreparedPage, replaced right away, and profiles can
// be registered at run time with init scripts and a viewport.
//
// Usage: node tests/e2e/prepared-page.mjs /abs/path/to/lightningpanda
// Requires `bun install` (or npm install) in bench/ for the pinned playwright-core.
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import net from 'node:net';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(here, '..', '..', 'bench', 'package.json'));
const { chromium } = require('playwright-core');

const bin = process.argv[2];
if (!bin) {
  console.error('usage: node tests/e2e/prepared-page.mjs /abs/path/to/lightningpanda');
  process.exit(2);
}
const results = [];
const deadline = setTimeout(() => {
  console.error(`prepared-page: overall deadline exceeded; completed ${JSON.stringify(results)}`);
  process.exit(1);
}, 60_000);

const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer();
  s.on('error', reject);
  s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); });
});

const appPort = await freePort();
const origin = `http://127.0.0.1:${appPort}`;
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'lp-prepared-'));
const dist = path.join(tmp, 'dist');
fs.mkdirSync(path.join(dist, 'assets'), { recursive: true });
fs.writeFileSync(path.join(dist, 'index.html'),
  '<!doctype html><html><head><script type="module" src="/assets/app.js"></script></head><body><div id="app">shell</div></body></html>');
fs.writeFileSync(path.join(dist, 'assets', 'app.js'),
  'document.getElementById("app").textContent = "token=" + localStorage.getItem("token") + " path=" + location.pathname + " w=" + innerWidth; globalThis.appLoaded = true; globalThis.bootedAt = performance.now();');
const seedPath = path.join(tmp, 'seed.json');
fs.writeFileSync(seedPath, JSON.stringify({ [origin]: { token: 'preloaded' } }));

const port = await freePort();
const child = spawn(bin, ['serve', '--host', '127.0.0.1', '--port', String(port), '--log-level', 'error',
  '--experimental-features', 'cors', '--static', `${origin}=${dist}`, '--local-storage', seedPath,
  '--prepare', `${origin}/dashboard`], {
  stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' },
});
let stderr = '';
child.stderr.on('data', (d) => { stderr = (stderr + d).slice(-4000); });
process.on('exit', () => { try { child.kill('SIGKILL'); } catch {} try { fs.rmSync(tmp, { recursive: true, force: true }); } catch {} });

async function check(name, fn) {
  const t = performance.now();
  try { await fn(); results.push({ name, ok: true, ms: +(performance.now() - t).toFixed(1) }); }
  catch (e) { results.push({ name, ok: false, error: String(e?.stack ?? e).split('\n').slice(0, 4).join('\n') }); }
}

const started = performance.now();
for (;;) {
  try { if ((await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(500) })).ok) break; } catch {}
  if (child.exitCode !== null) { console.error(`lightningpanda exited early (${child.exitCode}): ${stderr}`); process.exit(1); }
  if (performance.now() - started > 10_000) { console.error(`startup timed out: ${stderr}`); process.exit(1); }
  await new Promise((r) => setTimeout(r, 10));
}

const browser = await chromium.connectOverCDP(`ws://127.0.0.1:${port}`);
const cdp = await browser.newBrowserCDPSession();
const defaultContext = browser.contexts()[0];

async function waitReady(url, timeoutMs = 5000) {
  const t = performance.now();
  for (;;) {
    const { pages } = await cdp.send('Lightningpanda.preparedPages');
    const mine = pages.filter((p) => p.url === url);
    if (mine.some((p) => p.ready)) return pages;
    if (performance.now() - t > timeoutMs) throw new Error(`not ready: ${JSON.stringify(pages)}`);
    await new Promise((r) => setTimeout(r, 5));
  }
}
async function acquire(params) {
  const [page, result] = await Promise.all([
    defaultContext.waitForEvent('page'),
    cdp.send('Lightningpanda.acquirePreparedPage', params),
  ]);
  return { page, result };
}

let first;
await check('a page is booted before any command', async () => {
  const pages = await waitReady(`${origin}/dashboard`);
  assert.equal(pages.length, 1);
});
await check('acquire hands over the booted page', async () => {
  const t = performance.now();
  const { page, result } = await acquire({ url: `${origin}/dashboard` });
  const tAcquired = performance.now();
  assert.equal(result.ready, true);
  assert.equal(page.url(), `${origin}/dashboard`);
  await page.waitForLoadState('load');
  const tLoaded = performance.now();
  assert.equal(await page.textContent('#app'), 'token=preloaded path=/dashboard w=1920', 'engine default viewport');
  assert.equal(await page.evaluate(() => globalThis.appLoaded), true);
  results.push({ name: `handover (ms): acquire ${(tAcquired - t).toFixed(1)}, load state ${(tLoaded - tAcquired).toFixed(1)}, assert ${(performance.now() - tLoaded).toFixed(1)}`, ok: true, ms: +(performance.now() - t).toFixed(1) });
  first = page;
});
await check('the pool refills and the next page is independent', async () => {
  await waitReady(`${origin}/dashboard`);
  const { page } = await acquire({ url: `${origin}/dashboard` });
  assert.notEqual(page, first);
  await page.waitForLoadState('load');
  await first.evaluate(() => localStorage.setItem('token', 'changed'));
  assert.equal(await page.evaluate(() => localStorage.getItem('token')), 'preloaded', 'separate contexts');
  assert.equal(await first.evaluate(() => 1 + 1), 2, 'the first page still works');
});
await check('registerPreparedPage: init scripts and viewport apply before boot', async () => {
  const { profileId } = await cdp.send('Lightningpanda.registerPreparedPage', {
    url: `${origin}/settings`, initScripts: ['globalThis.__features = ["a", "b"]'], viewport: { width: 1440, height: 900 },
  });
  assert.equal(profileId, 1);
  await waitReady(`${origin}/settings`);
  const { page, result } = await acquire({ profileId });
  assert.equal(result.url, `${origin}/settings`);
  await page.waitForLoadState('load');
  assert.deepEqual(await page.evaluate(() => globalThis.__features), ['a', 'b']);
  assert.equal(await page.textContent('#app'), 'token=preloaded path=/settings w=1440');
});
await check('refillAfterClose: no replacement until the acquired page closes', async () => {
  const url = `${origin}/after-close`;
  const { profileId } = await cdp.send('Lightningpanda.registerPreparedPage', { url, refillAfterClose: true });
  await waitReady(url);
  const { page } = await acquire({ profileId });
  await page.waitForLoadState('load');
  await new Promise((r) => setTimeout(r, 80));
  let { pages } = await cdp.send('Lightningpanda.preparedPages');
  assert.equal(pages.filter((p) => p.url === url).length, 0, 'nothing boots while the page is in use');
  await page.close();
  await waitReady(url, 5000);
});
await check('unknown url is an error; ordinary pages still work', async () => {
  await assert.rejects(cdp.send('Lightningpanda.acquirePreparedPage', { url: `${origin}/nope` }), /No prepared page/);
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto(`${origin}/plain`);
  assert.equal(await page.textContent('#app'), 'token=preloaded path=/plain w=1280', 'Playwright default viewport');
  await context.close();
});
await check('Target.getTargets lists only acquired pages', async () => {
  const { targetInfos } = await cdp.send('Target.getTargets');
  const urls = targetInfos.map((t) => t.url).sort();
  assert.deepEqual(urls, [`${origin}/dashboard`, `${origin}/dashboard`, `${origin}/settings`], JSON.stringify(targetInfos));
  // Replacements boot once the thread has been quiet for a moment.
  await waitReady(`${origin}/dashboard`);
  await waitReady(`${origin}/settings`);
  const pages = await waitReady(`${origin}/after-close`);
  assert.deepEqual(pages.map((p) => p.url).sort(), [`${origin}/after-close`, `${origin}/dashboard`, `${origin}/settings`], 'one waiting per profile');
});

await browser.close();
clearTimeout(deadline);
const failed = results.filter((r) => !r.ok);
console.log(JSON.stringify({ ok: failed.length === 0, results }, null, 2));
if (failed.length) console.error(`stderr tail:\n${stderr}`);
process.exit(failed.length ? 1 : 0);
