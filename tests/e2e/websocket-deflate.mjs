// Real Playwright 1.59.1 (connectOverCDP) against `pyro serve`:
// WebSocket permessage-deflate (RFC 7692) against Bun's WebSocket servers.
// One server requires the extension (it answers 400 to a handshake that does
// not offer it) and compresses everything it sends; the other refuses it.
// The page sees `extensions`, round-trips text and binary, and CDP reports
// the offer, the server's answer and the inflated payloads.
//
// Usage: bun tests/e2e/websocket-deflate.mjs /abs/path/to/pyro
// Requires Bun (Bun.serve) and `bun install` in bench/ for playwright-core.
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(here, '..', '..', 'bench', 'package.json'));
const { chromium } = require('playwright-core');

const bin = process.argv[2];
if (!bin) {
  console.error('usage: bun tests/e2e/websocket-deflate.mjs /abs/path/to/pyro');
  process.exit(2);
}
if (typeof Bun === 'undefined') {
  console.error('websocket-deflate: run with bun (it needs Bun.serve)');
  process.exit(2);
}

const results = [];
const deadline = setTimeout(() => {
  console.error(`websocket-deflate: deadline exceeded; completed ${JSON.stringify(results)}`);
  process.exit(1);
}, 60_000);
const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer();
  s.on('error', reject);
  s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); });
});

const PAGE = `<!doctype html><title>ws</title><script>
globalThis.roundTrip = (url, messages) => new Promise((resolve, reject) => {
  const ws = new WebSocket(url);
  ws.binaryType = 'arraybuffer';
  const received = [];
  ws.onerror = () => reject(new Error('error event'));
  ws.onopen = () => { for (const m of messages) ws.send(m.binary ? new TextEncoder().encode(m.binary) : m); };
  ws.onmessage = (e) => {
    received.push(typeof e.data === 'string' ? e.data : { binary: new TextDecoder().decode(e.data) });
    if (received.length === messages.length) {
      const extensions = ws.extensions;
      ws.onclose = (c) => resolve({ extensions, received, code: c.code, clean: c.wasClean });
      ws.close(1000);
    }
  };
});
</script>`;

// Handshake offers seen by each server.
const offers = { required: [], refused: [] };
const echo = { message(ws, msg) { ws.send(msg, true); } };
const required = Bun.serve({
  hostname: '127.0.0.1',
  port: 0,
  fetch(req, server) {
    const offer = req.headers.get('sec-websocket-extensions') ?? '';
    if (req.headers.get('upgrade') !== 'websocket') return new Response(PAGE, { headers: { 'content-type': 'text/html' } });
    offers.required.push(offer);
    if (!/permessage-deflate/.test(offer)) return new Response('deflate required', { status: 400 });
    return server.upgrade(req) ? undefined : new Response('upgrade failed', { status: 500 });
  },
  websocket: { ...echo, perMessageDeflate: true },
});
const refused = Bun.serve({
  hostname: '127.0.0.1',
  port: 0,
  fetch(req, server) {
    offers.refused.push(req.headers.get('sec-websocket-extensions') ?? '');
    return server.upgrade(req) ? undefined : new Response('upgrade failed', { status: 500 });
  },
  websocket: { ...echo, perMessageDeflate: false },
});
const origin = `http://127.0.0.1:${required.port}`;
const deflateUrl = `ws://127.0.0.1:${required.port}/`;
const plainUrl = `ws://127.0.0.1:${refused.port}/`;

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
await page.goto(`${origin}/`);

const big = 'deflate me please, '.repeat(5000);
const messages = ['hello', big, { binary: 'bytes '.repeat(300) }, '', big];

await check('a server that requires deflate: extensions set, messages round-trip', async () => {
  const r = await page.evaluate(([url, m]) => roundTrip(url, m), [deflateUrl, messages]);
  assert.match(r.extensions, /^permessage-deflate\b/);
  assert.equal(r.received.length, messages.length);
  assert.deepEqual(r.received, messages);
  assert.equal(r.code, 1000);
  assert.equal(r.clean, true);
  assert.deepEqual(offers.required, ['permessage-deflate; client_max_window_bits']);
});

await check('a server that refuses deflate: extensions empty, messages round-trip', async () => {
  const r = await page.evaluate(([url, m]) => roundTrip(url, m), [plainUrl, messages]);
  assert.equal(r.extensions, '');
  assert.deepEqual(r.received, messages);
  assert.equal(r.clean, true);
  assert.deepEqual(offers.refused, ['permessage-deflate; client_max_window_bits']);
});

await check('CDP shows the offer, the answer and inflated payloads', async () => {
  const cdp = await context.newCDPSession(page);
  const events = [];
  for (const name of ['webSocketWillSendHandshakeRequest', 'webSocketHandshakeResponseReceived', 'webSocketFrameSent', 'webSocketFrameReceived']) {
    cdp.on(`Network.${name}`, (p) => events.push({ name, p }));
  }
  await cdp.send('Network.enable');
  const sockets = [];
  page.on('websocket', (ws) => {
    const s = { url: ws.url(), sent: [], received: [] };
    ws.on('framesent', (f) => s.sent.push(f.payload.toString()));
    ws.on('framereceived', (f) => s.received.push(f.payload.toString()));
    sockets.push(s);
  });
  const r = await page.evaluate(([url, m]) => roundTrip(url, m), [deflateUrl, ['abc', big]]);
  assert.deepEqual(r.received, ['abc', big]);
  const request = events.find((e) => e.name === 'webSocketWillSendHandshakeRequest');
  const reqHeaders = Object.fromEntries(Object.entries(request.p.request.headers).map(([k, v]) => [k.toLowerCase(), v]));
  assert.equal(reqHeaders['sec-websocket-extensions'], 'permessage-deflate; client_max_window_bits');
  const response = events.find((e) => e.name === 'webSocketHandshakeResponseReceived');
  const resHeaders = Object.fromEntries(Object.entries(response.p.response.headers).map(([k, v]) => [k.toLowerCase(), v]));
  assert.equal(resHeaders['sec-websocket-extensions'], r.extensions);
  const sent = events.filter((e) => e.name === 'webSocketFrameSent').map((e) => e.p.response.payloadData);
  const received = events.filter((e) => e.name === 'webSocketFrameReceived').map((e) => e.p.response.payloadData);
  assert.deepEqual(sent, ['abc', big]);
  assert.deepEqual(received, ['abc', big]);
  assert.equal(sockets.length, 1);
  assert.deepEqual(sockets[0].sent, ['abc', big]);
  assert.deepEqual(sockets[0].received, ['abc', big]);
  await cdp.detach();
});

await check('close', async () => {
  await page.close();
  await context.close();
  await browser.close();
});

clearTimeout(deadline);
child.kill('SIGTERM');
required.stop(true);
refused.stop(true);
const failed = results.filter((r) => !r.ok);
console.log(JSON.stringify({ passed: results.length - failed.length, total: results.length, results }, null, 2));
process.exit(failed.length ? 1 : 0);
