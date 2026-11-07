// Real Playwright 1.59.1 (connectOverCDP) against `lightningpanda serve`:
// Resource Timing and Navigation Timing as a page sees them. Subresources of
// every kind get `PerformanceResourceTiming` entries with the phases lp-net
// measured; a cross-origin response without Timing-Allow-Origin hides its
// details; the buffer fills, fires `resourcetimingbufferfull` and counts
// dropped entries; `PerformanceObserver({type: 'resource', buffered: true})`
// sees them; the navigation entry has its document fetch's phases.
//
// Usage: bun tests/e2e/resource-timing.mjs /abs/path/to/lightningpanda
// Requires `bun install` in bench/ for the pinned playwright-core.
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import http from 'node:http';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(here, '..', '..', 'bench', 'package.json'));
const { chromium } = require('playwright-core');

const bin = process.argv[2];
if (!bin) {
  console.error('usage: bun tests/e2e/resource-timing.mjs /abs/path/to/lightningpanda');
  process.exit(2);
}

const results = [];
const deadline = setTimeout(() => {
  console.error(`resource-timing: deadline exceeded; completed ${JSON.stringify(results)}`);
  process.exit(1);
}, 60_000);
const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer();
  s.on('error', reject);
  s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); });
});

const PAGE = `<!doctype html><html><head>
<link rel="stylesheet" href="/style.css">
<script src="/script.js"></script>
</head><body><img src="/img.png"><p>page</p></body></html>`;

const serve = (handler) => new Promise((resolve, reject) => {
  const server = http.createServer(handler);
  server.once('error', reject);
  server.listen(0, '127.0.0.1', () => resolve(server));
});
const server = await serve((req, res) => {
  const [type, body] = {
    '/': ['text/html', PAGE],
    '/style.css': ['text/css', 'p { color: red }'],
    '/script.js': ['text/javascript', 'globalThis.scriptRan = true'],
  }[req.url] ?? ['application/json', '{"answer":42}'];
  res.writeHead(200, { 'content-type': type });
  res.end(body);
});
const other = await serve((req, res) => {
  const headers = { 'content-type': 'text/plain', 'access-control-allow-origin': '*' };
  if (req.url.startsWith('/tao')) headers['timing-allow-origin'] = '*';
  res.writeHead(200, headers);
  res.end('cross');
});
const origin = `http://127.0.0.1:${server.address().port}`;
const otherOrigin = `http://127.0.0.1:${other.address().port}`;

const port = await freePort();
const child = spawn(
  bin,
  ['serve', '--host', '127.0.0.1', '--port', String(port), '--log-level', 'error'],
  { stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' } },
);
let stderr = '';
child.stderr.on('data', (d) => { stderr = (stderr + d).slice(-4000); });
process.on('exit', () => { try { child.kill('SIGKILL'); } catch {} });

const started = performance.now();
for (;;) {
  try {
    const r = await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(500) });
    if (r.ok) break;
  } catch {}
  if (child.exitCode !== null) { console.error(`lightningpanda exited early (${child.exitCode}): ${stderr}`); process.exit(1); }
  if (performance.now() - started > 10_000) { console.error(`startup timed out: ${stderr}`); process.exit(1); }
  await new Promise((r) => setTimeout(r, 10));
}

async function check(name, fn) {
  const t = performance.now();
  try {
    await fn();
    results.push({ name, ok: true, ms: +(performance.now() - t).toFixed(1) });
  } catch (e) {
    results.push({ name, ok: false, error: String(e?.stack ?? e).split('\n').slice(0, 4).join('\n') });
  }
}

const browser = await chromium.connectOverCDP(`ws://127.0.0.1:${port}`);
const context = await browser.newContext();
const page = await context.newPage();
await page.goto(`${origin}/`, { waitUntil: 'load' });

await check('subresources of every kind have entries', async () => {
  const kinds = await page.evaluate(async (other) => {
    await (await fetch('/data')).text();
    await new Promise((resolve) => {
      const x = new XMLHttpRequest();
      x.open('GET', '/xhr');
      x.onloadend = resolve;
      x.send();
    });
    await (await fetch(other + '/plain')).text();
    await (await fetch(other + '/tao')).text();
    return performance.getEntriesByType('resource')
      .map((e) => `${e.name.replace(location.origin, '').replace(other, 'X')}=${e.initiatorType}`).sort();
  }, otherOrigin);
  assert.deepEqual(kinds, ['/data=fetch', '/img.png=img', '/script.js=script', '/style.css=link', '/xhr=xmlhttprequest', 'X/plain=fetch', 'X/tao=fetch']);
});

await check('a fetch entry has its network phases in order and its sizes', async () => {
  const e = await page.evaluate(() => performance.getEntriesByName(location.origin + '/data')[0].toJSON());
  const order = ['startTime', 'fetchStart', 'domainLookupStart', 'domainLookupEnd', 'connectStart', 'connectEnd',
    'requestStart', 'responseStart', 'responseEnd'];
  for (let i = 1; i < order.length; i++) assert.ok(e[order[i - 1]] <= e[order[i]], `${order[i - 1]} <= ${order[i]}: ${JSON.stringify(e)}`);
  assert.ok(e.startTime > 0 && e.responseEnd > e.startTime, JSON.stringify(e));
  assert.equal(e.entryType, 'resource');
  assert.equal(e.encodedBodySize, 13);
  assert.equal(e.decodedBodySize, 13);
  assert.equal(e.transferSize, 313);
  assert.equal(e.nextHopProtocol, 'http/1.1');
  assert.equal(e.responseStatus, 200);
  assert.equal(e.contentType, 'application/json');
});

await check('cross-origin without Timing-Allow-Origin hides the details', async () => {
  const [plain, tao] = await page.evaluate((other) => ['/plain', '/tao']
    .map((p) => performance.getEntriesByName(other + p)[0].toJSON()), otherOrigin);
  assert.ok(plain.fetchStart > 0 && plain.responseEnd >= plain.fetchStart, JSON.stringify(plain));
  for (const k of ['domainLookupStart', 'connectStart', 'requestStart', 'responseStart', 'transferSize', 'encodedBodySize']) {
    assert.equal(plain[k], 0, k);
  }
  assert.equal(plain.nextHopProtocol, '');
  assert.ok(tao.requestStart > 0 && tao.encodedBodySize === 5, JSON.stringify(tao));
});

await check('the navigation entry has the document fetch', async () => {
  const n = await page.evaluate(() => performance.getEntriesByType('navigation')[0].toJSON());
  assert.equal(n.startTime, 0);
  assert.equal(n.fetchStart, 0);
  assert.ok(n.requestStart <= n.responseStart && n.responseStart <= n.responseEnd && n.responseEnd <= n.domInteractive, JSON.stringify(n));
  assert.ok(n.responseEnd > 0);
  assert.equal(n.encodedBodySize, Buffer.byteLength(PAGE));
  assert.equal(n.nextHopProtocol, 'http/1.1');
  assert.equal(n.responseStatus, 200);
});

await check('observers, the buffer full event and dropped entries', async () => {
  const r = await page.evaluate(async () => {
    const buffered = await new Promise((resolve) => new PerformanceObserver((list) => resolve(list.getEntries().length))
      .observe({ type: 'resource', buffered: true }));
    const seen = [];
    new PerformanceObserver((list) => seen.push(...list.getEntries().map((e) => e.name.split('/').pop())))
      .observe({ type: 'resource' });
    performance.clearResourceTimings();
    performance.setResourceTimingBufferSize(1);
    let full = 0;
    performance.addEventListener('resourcetimingbufferfull', () => full++);
    await Promise.all(['n1', 'n2', 'n3'].map((n) => fetch('/' + n).then((r) => r.text())));
    await new Promise((resolve) => setTimeout(resolve, 50));
    const dropped = await new Promise((resolve) => new PerformanceObserver((list, obs, options) => resolve(options.droppedEntriesCount))
      .observe({ type: 'resource', buffered: true }));
    return { buffered, seen: seen.sort(), full, kept: performance.getEntriesByType('resource').length, dropped };
  });
  assert.ok(r.buffered >= 7, JSON.stringify(r));
  assert.deepEqual(r.seen, ['n1', 'n2', 'n3']);
  assert.ok(r.full >= 1, JSON.stringify(r));
  assert.equal(r.kept, 1);
  assert.equal(r.dropped, 2);
});

await check('close', async () => {
  await page.close();
  await context.close();
  await browser.close();
});

clearTimeout(deadline);
child.kill('SIGTERM');
for (const s of [server, other]) {
  s.closeAllConnections();
  await new Promise((resolve) => s.close(resolve));
}
const failed = results.filter((r) => !r.ok);
console.log(JSON.stringify({ passed: results.length - failed.length, total: results.length, results }, null, 2));
process.exit(failed.length ? 1 : 0);
