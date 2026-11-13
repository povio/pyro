// Real Playwright 1.59.1 (connectOverCDP) and real MSW 2.15.0 browser mode
// against `pyro serve`: the page loads msw's browser bundle and its
// unmodified mockServiceWorker.js, `worker.start()` registers and activates
// the service worker, and the page's fetch() and XHR get the handlers' mocked
// responses; unhandled requests go to the network through the worker, where
// Playwright's `page.route()` still sees them. `context.serviceWorkers()` lists
// the worker (a `service_worker` CDP target).
//
// Usage: bun tests/e2e/msw.mjs /abs/path/to/pyro
// Requires `bun install` in bench/ (pinned playwright-core and msw).
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(here, '..', '..', 'bench', 'package.json'));
const { chromium } = require('playwright-core');
const mswRoot = path.dirname(require.resolve('msw/package.json'));
const MSW_BUNDLE = fs.readFileSync(path.join(mswRoot, 'lib', 'iife', 'index.js'));
// The vendored copy the Rust tests serve; the same file as the package's.
const WORKER = fs.readFileSync(path.join(here, '..', '..', 'crates', 'pyro-browser', 'tests', 'fixtures', 'msw', 'mockServiceWorker.js'));

const bin = process.argv[2];
if (!bin) {
  console.error('usage: bun tests/e2e/msw.mjs /abs/path/to/pyro');
  process.exit(2);
}

const results = [];
const deadline = setTimeout(() => {
  console.error(`msw: deadline exceeded; completed ${JSON.stringify(results)}`);
  process.exit(1);
}, 60_000);
const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer();
  s.on('error', reject);
  s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); });
});

const PAGE = `<!doctype html><html><head><script src="/msw.js"></script><script>
const { setupWorker, http, HttpResponse } = MockServiceWorker;
globalThis.events = [];
const worker = setupWorker(
  http.get('/api/user', () => HttpResponse.json({ name: 'mocked user' }, { headers: { 'x-mocked': 'yes' } })),
  http.post('/api/echo', async ({ request }) => HttpResponse.json({ got: await request.json(), method: request.method })),
  http.get('/api/missing', () => new HttpResponse('nope', { status: 404, statusText: 'Not There' })),
);
worker.events.on('request:start', ({ request }) => events.push('start ' + new URL(request.url).pathname));
worker.events.on('response:mocked', ({ request }) => events.push('mocked ' + new URL(request.url).pathname));
globalThis.started = worker.start({ onUnhandledRequest: 'bypass', quiet: true }).then(
  () => 'started', (e) => 'failed: ' + e.message + ' ' + e.stack);
</script></head><body><p>msw</p></body></html>`;

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
  if (child.exitCode !== null) { console.error(`pyro exited early (${child.exitCode}): ${stderr}`); process.exit(1); }
  if (performance.now() - started > 10_000) { console.error(`startup timed out: ${stderr}`); process.exit(1); }
  await new Promise((r) => setTimeout(r, 10));
}

const hits = [];
const server = http.createServer((req, res) => {
  hits.push(req.url);
  const send = (type, body, status = 200) => { res.writeHead(status, { 'content-type': type }); res.end(body); };
  if (req.url === '/') return send('text/html', PAGE);
  if (req.url === '/msw.js') return send('text/javascript', MSW_BUNDLE);
  if (req.url === '/mockServiceWorker.js') return send('text/javascript', WORKER);
  if (req.url === '/real') return send('text/plain', 'from the network');
  send('text/plain', 'missing', 404);
});
await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
const origin = `http://127.0.0.1:${server.address().port}`;

async function check(name, fn) {
  const t = performance.now();
  try {
    await fn();
    results.push({ name, ok: true, ms: +(performance.now() - t).toFixed(1) });
  } catch (e) {
    results.push({ name, ok: false, error: String(e?.stack ?? e).split('\n').slice(0, 6).join('\n') });
  }
}

const browser = await chromium.connectOverCDP(`ws://127.0.0.1:${port}`);
const context = await browser.newContext();
const page = await context.newPage();
const pageErrors = [];
page.on('pageerror', (e) => pageErrors.push(`${e.name}: ${e.message}`));
const serviceWorkerEvent = context.waitForEvent('serviceworker', { timeout: 10_000 }).catch((e) => e);

await page.goto(`${origin}/`);

await check('worker.start() registers and activates mockServiceWorker.js', async () => {
  assert.equal(await page.evaluate(() => started), 'started');
  const state = await page.evaluate(() => [navigator.serviceWorker.controller?.state, navigator.serviceWorker.controller?.scriptURL]);
  assert.deepEqual(state, ['activated', `${origin}/mockServiceWorker.js`]);
  assert.equal(hits.filter((h) => h === '/mockServiceWorker.js').length, 1);
});

await check('fetch() gets the handler\'s mocked JSON response', async () => {
  const got = await page.evaluate(async () => {
    const r = await fetch('/api/user');
    return { status: r.status, mocked: r.headers.get('x-mocked'), body: await r.json() };
  });
  assert.deepEqual(got, { status: 200, mocked: 'yes', body: { name: 'mocked user' } });
  assert.ok(!hits.includes('/api/user'), JSON.stringify(hits));
});

await check('a POST body reaches the handler', async () => {
  const got = await page.evaluate(async () => {
    const r = await fetch('/api/echo', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ a: 1 }) });
    return r.json();
  });
  assert.deepEqual(got, { got: { a: 1 }, method: 'POST' });
});

await check('XHR is mocked too, with the handler\'s status', async () => {
  const got = await page.evaluate(() => new Promise((resolve) => {
    const xhr = new XMLHttpRequest();
    xhr.open('GET', '/api/missing');
    xhr.onload = () => resolve([xhr.status, xhr.statusText, xhr.responseText]);
    xhr.send();
  }));
  assert.deepEqual(got, [404, 'Not There', 'nope']);
});

await check('msw life-cycle events fire', async () => {
  const events = await page.evaluate(() => events.slice());
  assert.ok(events.includes('start /api/user'), JSON.stringify(events));
  assert.ok(events.includes('mocked /api/user'), JSON.stringify(events));
});

await check('unhandled requests go through the worker to the network', async () => {
  const text = await page.evaluate(() => fetch('/real').then((r) => r.text()));
  assert.equal(text, 'from the network');
  assert.ok(hits.includes('/real'), JSON.stringify(hits));
});

await check('page.route() sees the worker\'s passthrough request', async () => {
  let routed = 0;
  await page.route('**/real', (route) => { routed++; return route.fulfill({ status: 200, contentType: 'text/plain', body: 'routed' }); });
  const text = await page.evaluate(() => fetch('/real').then((r) => r.text()));
  await page.unroute('**/real');
  assert.equal(text, 'routed');
  assert.equal(routed, 1);
});

await check('context.serviceWorkers() lists the worker', async () => {
  const event = await serviceWorkerEvent;
  assert.ok(!(event instanceof Error), String(event));
  assert.equal(event.url(), `${origin}/mockServiceWorker.js`);
  const workers = context.serviceWorkers();
  assert.equal(workers.length, 1);
  assert.equal(workers[0].url(), `${origin}/mockServiceWorker.js`);
  assert.equal(await workers[0].evaluate(() => typeof self.clients.matchAll), 'function');
});

await check('no page errors', async () => {
  assert.deepEqual(pageErrors, []);
});

await check('close', async () => {
  await page.close();
  await context.close();
  await browser.close();
});

clearTimeout(deadline);
child.kill('SIGTERM');
server.closeAllConnections();
await new Promise((resolve) => server.close(resolve));
const failed = results.filter((r) => !r.ok);
console.log(JSON.stringify({ passed: results.length - failed.length, total: results.length, results }, null, 2));
process.exit(failed.length ? 1 : 0);
