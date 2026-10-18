// Real Playwright 1.59.1 against `lightningpanda serve` with the opt-in shared public-asset cache
// (LIGHTNINGPANDA_SHARED_ASSET_CACHE_MB). Checks the CDP view: a public asset fetched in one fresh
// context is a shared hit (fromDiskCache, no server request) in another, a private one is not, and
// Network.clearBrowserCache clears the shared store. Usage: node shared-asset-cache.mjs <binary>
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import http from 'node:http';
import net from 'node:net';
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(here, '..', '..', 'bench', 'package.json'));
const { chromium } = require('playwright-core');
const bin = process.argv[2];
if (!bin) { console.error('usage: node shared-asset-cache.mjs <lightningpanda binary>'); process.exit(2); }

const hits = [];
const server = http.createServer((req, res) => {
  hits.push(req.url);
  if (req.url.startsWith('/public')) res.writeHead(200, { 'content-type': 'text/javascript', 'cache-control': 'public, max-age=3600, immutable' });
  else if (req.url.startsWith('/private')) res.writeHead(200, { 'content-type': 'text/javascript', 'cache-control': 'private, max-age=3600' });
  else res.writeHead(200, { 'content-type': 'text/html' });
  res.end(req.url.startsWith('/p') ? '1;' : '<!doctype html><p>shared</p>');
});
await new Promise(r => server.listen(0, '127.0.0.1', r));
const origin = `http://127.0.0.1:${server.address().port}`;
const port = await new Promise(r => { const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => r(p)); }); });
const child = spawn(bin, ['serve', '--host', '127.0.0.1', '--port', String(port), '--log-level', 'error'],
  { stdio: ['ignore', 'ignore', 'inherit'], env: { ...process.env, LIGHTNINGPANDA_SHARED_ASSET_CACHE_MB: '4', LIGHTPANDA_DISABLE_TELEMETRY: 'true' } });
let ok = false;
try {
  for (let i = 0; i < 200; i++) { try { if ((await fetch(`http://127.0.0.1:${port}/json/version`)).ok) break; } catch {} await new Promise(r => setTimeout(r, 25)); }
  const browser = await chromium.connectOverCDP(`ws://127.0.0.1:${port}`);
  const open = async () => {
    const context = await browser.newContext();
    const page = await context.newPage();
    await page.goto(`${origin}/doc`);
    const cdp = await context.newCDPSession(page);
    await cdp.send('Network.enable');
    const fromDisk = [];
    cdp.on('Network.responseReceived', e => { if (e.response.url.includes('/p')) fromDisk.push(e.response.fromDiskCache); });
    return { context, page, cdp, fromDisk, load: p => page.evaluate(u => fetch(u).then(r => r.text()), `${origin}${p}`) };
  };
  const count = p => hits.filter(h => h === p).length;
  const a = await open();
  await a.load('/public'); await a.load('/private');
  const b = await open();
  await b.load('/public'); await b.load('/private');
  assert.equal(count('/public'), 1, 'public asset shared across fresh contexts');
  assert.equal(count('/private'), 2, 'private response not shared');
  assert.deepEqual(b.fromDisk, [true, false]);
  await b.cdp.send('Network.clearBrowserCache');
  const c = await open();
  await c.load('/public');
  assert.equal(count('/public'), 2, 'clearBrowserCache cleared the shared store');
  assert.deepEqual(c.fromDisk, [false]);
  for (const x of [a, b, c]) await x.context.close();
  await browser.close();
  ok = true;
} finally {
  child.kill('SIGTERM'); server.close();
  console.log(JSON.stringify({ ok }));
}
