// Real Playwright 1.59.1 (connectOverCDP) against `lightningpanda serve` on JavaScriptCore.
// Covers about:blank plus real HTTP navigation, redirects, response bodies and explicit network failures.
//
// Usage: node tests/e2e/about-blank.mjs /abs/path/to/lightningpanda
// Requires `bun install` (or npm install) in bench/ for the pinned playwright-core.
// Exit status: 0 when every check passed, 1 on any failure, 2 for usage errors.
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import net from 'node:net';
import http from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(here, '..', '..', 'bench', 'package.json'));
const { chromium } = require('playwright-core');

const bin = process.argv[2];
if (!bin) {
  console.error('usage: node tests/e2e/about-blank.mjs /abs/path/to/lightningpanda');
  process.exit(2);
}

const deadline = setTimeout(() => {
  console.error(`about-blank: overall deadline exceeded; completed ${JSON.stringify(results)}`);
  process.exit(1);
}, 60_000);

const port = await new Promise((resolve, reject) => {
  const s = net.createServer();
  s.on('error', reject);
  s.listen(0, '127.0.0.1', () => {
    const { port } = s.address();
    s.close(() => resolve(port));
  });
});

// Exact flags TOS passes (apps/fe/e2e/fixtures/lightpanda.ts).
const child = spawn(
  bin,
  ['serve', '--host', '127.0.0.1', '--port', String(port), '--log-level', 'error',
    '--experimental-features', 'cors', '--load-resources', 'stylesheet', '--load-resources', 'iframe'],
  { stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' } },
);
let stderr = '';
child.stderr.on('data', (d) => { stderr = (stderr + d).slice(-4000); });
process.on('exit', () => { try { child.kill('SIGKILL'); } catch {} });

const results = [];
async function check(name, fn) {
  if (process.env.E2E_VERBOSE) console.error(`check: ${name}`);
  const t = performance.now();
  try {
    await fn();
    results.push({ name, ok: true, ms: +(performance.now() - t).toFixed(1) });
  } catch (e) {
    results.push({ name, ok: false, error: String(e?.stack ?? e).split('\n').slice(0, 4).join('\n') });
  }
}

const started = performance.now();
for (;;) {
  try {
    const r = await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(500) });
    if (r.ok) break;
  } catch {}
  if (child.exitCode !== null) {
    console.error(`lightningpanda exited early (${child.exitCode}): ${stderr}`);
    process.exit(1);
  }
  if (performance.now() - started > 10_000) {
    console.error(`startup timed out: ${stderr}`);
    process.exit(1);
  }
  await new Promise((r) => setTimeout(r, 10));
}
const startupMs = +(performance.now() - started).toFixed(1);

const browser = await chromium.connectOverCDP(`ws://127.0.0.1:${port}`);
const context = await browser.newContext({ locale: undefined });
const page = await context.newPage();
const consoleMessages = [];
const pageErrors = [];
page.on('console', (m) => consoleMessages.push(`${m.type()}:${m.text()}`));
page.on('pageerror', (e) => pageErrors.push(`${e.name}:${e.message}`));

await check('initial url', async () => assert.equal(page.url(), 'about:blank'));
await check('evaluate', async () => assert.equal(await page.evaluate(() => 1 + 2), 3));
await check('evaluate with argument', async () =>
  assert.deepEqual(await page.evaluate((x) => [x, x === null], null), [null, true]));
await check('awaited promise across a timer', async () =>
  assert.equal(await page.evaluate(() => new Promise((r) => setTimeout(() => r('late'), 10))), 'late'));
await check('evaluateHandle + getProperties + jsonValue', async () => {
  const h = await page.evaluateHandle(() => ({ a: 1, b: [1, 2] }));
  assert.deepEqual([...(await h.getProperties()).keys()].sort(), ['a', 'b']);
  assert.deepEqual(await h.jsonValue(), { a: 1, b: [1, 2] });
  await h.dispose();
});
await check('console event', async () => {
  await page.evaluate(() => console.log('hello', 42));
  await page.waitForTimeout(20);
  assert.ok(consoleMessages.includes('log:hello 42'), JSON.stringify(consoleMessages));
});
await check('pageerror from a timer', async () => {
  await page.evaluate(() => { setTimeout(() => { throw new TypeError('boom'); }, 0); });
  await page.waitForTimeout(50);
  assert.ok(pageErrors.includes('TypeError:boom'), JSON.stringify(pageErrors));
});
await check('evaluate exception', async () => {
  await assert.rejects(page.evaluate(() => { throw new RangeError('bad range'); }), /RangeError: bad range/);
});
await check('goto about:blank', async () => {
  await page.goto('about:blank');
  assert.equal(await page.evaluate(() => typeof window), 'object');
});
await check('goto http fails explicitly', async () => {
  await assert.rejects(page.goto('http://127.0.0.1:1/x', { timeout: 5000 }), /ERR_CONNECTION_REFUSED/);
});
await check('Fetch fulfill and abort', async () => {
  const body = '<script>globalThis.fulfilledFromRoute=43</script>';
  await page.route('**/fulfilled', (route) => route.fulfill({ status: 202, contentType: 'text/html', body }));
  try {
    const response = await page.goto('http://127.0.0.1:1/fulfilled', { timeout: 5000 });
    assert.equal(response.status(), 202);
    assert.equal(await response.text(), body);
    assert.equal(await page.evaluate(() => globalThis.fulfilledFromRoute), 43);
  } finally {
    await page.unroute('**/fulfilled');
  }
  await page.route('**/aborted', (route) => route.abort('aborted'));
  try {
    await assert.rejects(page.goto('http://127.0.0.1:1/aborted', { timeout: 5000 }), /ERR_ABORTED/);
  } finally {
    await page.unroute('**/aborted');
  }
});
await check('HTTP redirect, response body and inline script', async () => {
  const body = '<!doctype html><script>globalThis.loadedFromHttp=42</script>';
  let continuedHeader = null;
  const server = http.createServer((req, res) => {
    if (req.url === '/redirect') {
      res.writeHead(302, { location: '/document' });
      res.end();
    } else {
      continuedHeader = req.headers['x-route-check'];
      res.writeHead(201, { 'content-type': 'text/html' });
      res.end(body);
    }
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  try {
    const origin = `http://127.0.0.1:${server.address().port}`;
    await page.route('**/redirect', (route) => route.continue({ headers: { ...route.request().headers(), 'x-route-check': 'continued' } }));
    const response = await page.goto(`${origin}/redirect`, { timeout: 5000 });
    assert.ok(response, 'navigation exposes a Network response');
    assert.equal(response.status(), 201);
    assert.equal(response.url(), `${origin}/document`);
    assert.equal(await response.text(), body);
    assert.equal(response.request().redirectedFrom().url(), `${origin}/redirect`);
    assert.equal(await page.evaluate(() => globalThis.loadedFromHttp), 42);
    assert.equal(continuedHeader, 'continued');
  } finally {
    await page.unroute('**/redirect');
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
});
await check('Fetch Basic authentication challenge', async () => {
  let requests = 0;
  const authorization = `Basic ${Buffer.from('fixture:local-test').toString('base64')}`;
  const server = http.createServer((req, res) => {
    requests++;
    if (req.headers.authorization !== authorization) {
      res.writeHead(401, { 'www-authenticate': 'Basic realm="fixture"', 'content-length': '0' });
      res.end();
    } else {
      res.writeHead(200, { 'content-type': 'text/html' });
      res.end('<script>globalThis.authenticatedFixture=44</script>');
    }
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  let authenticated;
  try {
    authenticated = await browser.newContext({ httpCredentials: { username: 'fixture', password: 'local-test' } });
    const authPage = await authenticated.newPage();
    const response = await authPage.goto(`http://127.0.0.1:${server.address().port}/auth`, { timeout: 5000 });
    assert.equal(response.status(), 200);
    assert.equal(await authPage.evaluate(() => globalThis.authenticatedFixture), 44);
    assert.equal(requests, 2, 'one challenge followed by one credentialed retry');
  } finally {
    if (authenticated) await authenticated.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
});
await check('context cookies reach HTTP and stay isolated', async () => {
  const received = [];
  const server = http.createServer((req, res) => {
    received.push(req.headers.cookie ?? '');
    res.writeHead(200, { 'content-type': 'text/html', 'set-cookie': 'fromServer=2; Path=/; HttpOnly; SameSite=Lax' });
    res.end('<script>globalThis.cookieFixture=45</script>');
  });
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const url = `http://127.0.0.1:${server.address().port}/cookie`;
  let first, second;
  try {
    first = await browser.newContext(); second = await browser.newContext();
    await first.addCookies([{ name: 'injected', value: '1', url, httpOnly: true, sameSite: 'Lax' }]);
    assert.deepEqual(await second.cookies(), []);
    await (await first.newPage()).goto(url, { timeout: 5000 });
    assert.equal(received[0], 'injected=1');
    const cookies = await first.cookies(url);
    assert.deepEqual(cookies.map(c => c.name).sort(), ['fromServer', 'injected']);
    assert.ok(cookies.every(c => c.httpOnly && c.sameSite === 'Lax' && c.expires === -1));
    await (await second.newPage()).goto(url, { timeout: 5000 });
    assert.equal(received[1], '');
    await first.clearCookies();
    assert.deepEqual(await first.cookies(), []);
    assert.equal((await second.cookies())[0].name, 'fromServer');
  } finally {
    if (first) await first.close(); if (second) await second.close();
    server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
  }
});
await check('second context is independent', async () => {
  const other = await browser.newContext();
  const p2 = await other.newPage();
  await p2.evaluate(() => { globalThis.marker = 'second'; });
  assert.equal(await page.evaluate(() => typeof globalThis.marker), 'undefined');
  await other.close();
});
await check('close', async () => {
  await page.close();
  await context.close();
  await browser.close();
});

clearTimeout(deadline);
child.kill('SIGTERM');
const failed = results.filter((r) => !r.ok);
console.log(JSON.stringify({ startupMs, passed: results.length - failed.length, total: results.length, results }, null, 2));
process.exit(failed.length ? 1 : 0);
