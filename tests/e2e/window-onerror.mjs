// Real Playwright 1.59.1 (connectOverCDP) against `lightningpanda serve`: the
// HTML "report an exception" path. Uncaught exceptions of listeners, timers,
// microtasks and XHR callbacks fire `error` at the window; `window.onerror`
// gets (message, source, lineno, colno, error) in that order; returning true
// (or `preventDefault()` on the event) cancels the console report, so
// Playwright's `pageerror` sees only the exceptions nobody handled.
//
// Usage: bun tests/e2e/window-onerror.mjs /abs/path/to/lightningpanda
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
  console.error('usage: bun tests/e2e/window-onerror.mjs /abs/path/to/lightningpanda');
  process.exit(2);
}

const results = [];
const deadline = setTimeout(() => {
  console.error(`window-onerror: deadline exceeded; completed ${JSON.stringify(results)}`);
  process.exit(1);
}, 60_000);
const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer();
  s.on('error', reject);
  s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); });
});

const PAGE = `<!doctype html><html><head><script>
globalThis.calls = [];
window.onerror = function (message, source, lineno, colno, error) {
  calls.push({ n: arguments.length, self: this === window, message, source, lineno, colno, error: error && error.message });
  return /cancel/.test(message);
};
addEventListener('error', (ev) => {
  calls.push({ listener: ev.message, type: ev.type, trusted: ev.isTrusted, bubbles: ev.bubbles, cancelable: ev.cancelable, isErrorEvent: ev instanceof ErrorEvent });
  if (/prevent/.test(ev.message)) ev.preventDefault();
});
</script></head><body><button id="b">go</button><script>
document.getElementById('b').addEventListener('click', () => { throw new Error('click cancel'); });
</script></body></html>`;

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

const server = http.createServer((req, res) => {
  res.writeHead(200, { 'content-type': req.url === '/data' ? 'text/plain' : 'text/html' });
  res.end(req.url === '/data' ? 'payload' : PAGE);
});
await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
const origin = `http://127.0.0.1:${server.address().port}`;

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
const pageErrors = [];
page.on('pageerror', (e) => pageErrors.push(`${e.name}: ${e.message}`));

const settle = () => page.waitForTimeout(50);
const takeCalls = () => page.evaluate(() => calls.splice(0));
const takeErrors = () => pageErrors.splice(0);

await page.goto(`${origin}/`);

await check('listener exception: onerror argument order, event shape, true cancels', async () => {
  await page.click('#b');
  await settle();
  const calls = await takeCalls();
  assert.equal(calls.length, 2, JSON.stringify(calls));
  const [handler, listener] = calls;
  assert.equal(handler.n, 5);
  assert.equal(handler.self, true);
  assert.equal(handler.message, 'Uncaught Error: click cancel');
  assert.equal(handler.source, `${origin}/`);
  assert.ok(handler.lineno > 0 && handler.colno > 0, JSON.stringify(handler));
  assert.equal(handler.error, 'click cancel');
  assert.deepEqual(listener, { listener: 'Uncaught Error: click cancel', type: 'error', trusted: true, bubbles: false, cancelable: true, isErrorEvent: true });
  assert.deepEqual(takeErrors(), []);
});

await check('timer exception: onerror returning false keeps the console report', async () => {
  await page.evaluate(() => { setTimeout(() => { throw new TypeError('timer reported'); }, 0); });
  await settle();
  const calls = await takeCalls();
  assert.equal(calls[0]?.message, 'Uncaught TypeError: timer reported', JSON.stringify(calls));
  assert.equal(calls[0].error, 'timer reported');
  assert.deepEqual(takeErrors(), ['TypeError: timer reported']);
});

await check('preventDefault on the error event cancels the report', async () => {
  await page.evaluate(() => { queueMicrotask(() => { throw new Error('microtask prevent'); }); });
  await settle();
  const calls = await takeCalls();
  assert.equal(calls[0]?.message, 'Uncaught Error: microtask prevent', JSON.stringify(calls));
  assert.deepEqual(takeErrors(), []);
});

await check('XHR load callback exception reaches onerror with the thrown error', async () => {
  await page.evaluate(() => new Promise((resolve) => {
    const xhr = new XMLHttpRequest();
    xhr.onload = () => { setTimeout(resolve, 0); throw new RangeError('xhr cancel'); };
    xhr.open('GET', '/data');
    xhr.send();
  }));
  await settle();
  const calls = await takeCalls();
  assert.equal(calls[0]?.message, 'Uncaught RangeError: xhr cancel', JSON.stringify(calls));
  assert.equal(calls[0].error, 'xhr cancel');
  assert.deepEqual(takeErrors(), []);
});

await check('thrown primitive: error is the value, source falls back to the document', async () => {
  await page.evaluate(() => { setTimeout(() => { throw 'plain cancel'; }, 0); });
  await settle();
  const calls = await takeCalls();
  assert.equal(calls[0]?.message, 'Uncaught plain cancel', JSON.stringify(calls));
  assert.equal(calls[0].source, `${origin}/`);
  assert.equal(calls[0].error, undefined);
  assert.deepEqual(takeErrors(), []);
});

await check('exception inside the error listener is reported without a second event', async () => {
  await page.evaluate(() => {
    const inner = () => { removeEventListener('error', inner); throw new Error('inner reported'); };
    addEventListener('error', inner);
    setTimeout(() => { throw new Error('outer cancel'); }, 0);
  });
  await settle();
  const calls = await takeCalls();
  assert.deepEqual(calls.map((c) => c.message ?? c.listener), ['Uncaught Error: outer cancel', 'Uncaught Error: outer cancel']);
  assert.deepEqual(takeErrors(), ['Error: inner reported']);
});

await check('page.evaluate exceptions are the caller\'s, not the window\'s', async () => {
  await assert.rejects(page.evaluate(() => { throw new Error('evaluate'); }), /evaluate/);
  await settle();
  assert.deepEqual(await takeCalls(), []);
  assert.deepEqual(takeErrors(), []);
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
