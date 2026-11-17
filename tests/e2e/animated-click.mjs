// Playwright waits for moving CSS animations before clicking.
// Usage: bun tests/e2e/animated-click.mjs /absolute/path/to/pyro
// Pass chromium instead of a binary for the reference run.
// Requires bun install in bench/.
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
  console.error('usage: bun tests/e2e/animated-click.mjs /abs/path/to/pyro');
  process.exit(2);
}

const results = [];
const deadline = setTimeout(() => {
  console.error(`animated-click: deadline exceeded; completed ${JSON.stringify(results)}`);
  process.exit(1);
}, 60_000);
const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer();
  s.on('error', reject);
  s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); });
});

const port = await freePort();
const child = bin === 'chromium' ? null : spawn(
  bin,
  ['serve', '--host', '127.0.0.1', '--port', String(port), '--log-level', 'error'],
  { stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' } },
);
let stderr = '';
child?.stderr.on('data', (d) => { stderr = (stderr + d).slice(-4000); });
process.on('exit', () => { try { child?.kill('SIGKILL'); } catch {} });

const started = performance.now();
for (; child;) {
  try {
    const r = await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(500) });
    if (r.ok) break;
  } catch {}
  if (child.exitCode !== null) { console.error(`pyro exited early (${child.exitCode}): ${stderr}`); process.exit(1); }
  if (performance.now() - started > 10_000) { console.error(`startup timed out: ${stderr}`); process.exit(1); }
  await new Promise((r) => setTimeout(r, 10));
}

const server = http.createServer((req, res) => {
  res.writeHead(200, { 'content-type': 'text/html' });
  res.end(req.url === '/child' ? `<!doctype html>
    <style>
      @keyframes enter { from { transform:translateX(-400px) } to { transform:translateX(0) } }
      #toast { position:fixed;right:20px;top:80px;animation:enter .7s linear both }
      button { width:40px;height:40px }
    </style>
    <div id=toast><button id=target>Next</button></div>
    <script>
      window.hits = [];
      document.getElementById('target').onclick = (event) => hits.push({
        trusted: event.isTrusted,
        finished: document.getElementById('toast').getAnimations().every(a => a.playState === 'finished'),
      });
    </script>` : `<!doctype html>
    <style>body { margin:0 } iframe { border:0;width:1280px;height:950px }</style>
    <iframe src=/child></iframe>`);
});
await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
const origin = `http://127.0.0.1:${server.address().port}`;

async function check(name, fn) {
  const t = performance.now();
  try {
    await fn();
    results.push({ name, ok: true, ms: +(performance.now() - t).toFixed(1) });
  } catch (e) {
    results.push({ name, ok: false, error: String(e?.stack ?? e) });
  }
}

const browser = child ? await chromium.connectOverCDP(`ws://127.0.0.1:${port}`) : await chromium.launch({headless:true});
if (child) {
  const cdp = await browser.newBrowserCDPSession();
  assert.equal(typeof (await cdp.send('Pyro.capabilities')).fork, 'boolean');
  await cdp.detach();
}
const context = await browser.newContext();
const page = await context.newPage();
for (const scripted of [false, true]) {
  await page.goto(`${origin}/`);
  const frame = page.frameLocator('iframe');
  if (scripted) {
    await frame.locator('#toast').evaluate((toast) => {
      toast.style.animation = 'none';
      toast.animate([{ transform: 'translateX(-400px)' }, { transform: 'translateX(0)' }],
        { duration: 700, fill: 'both' });
    });
  }
  await check(`${scripted ? 'script' : 'CSS'} animation settles before locator click`, async () => {
    await frame.locator('#target').click({ timeout: 3000 });
    const hits = await frame.locator('body').evaluate(() => window.hits);
    assert.equal(hits.length, 1);
    assert.equal(hits[0].trusted, true);
    assert.equal(hits[0].finished, true, 'click waits until the moving animation finishes');
  });
}
await check('close', async () => {
  await page.close();
  await context.close();
  await browser.close();
});

clearTimeout(deadline);
child?.kill('SIGTERM');
server.closeAllConnections();
await new Promise((resolve) => server.close(resolve));
const failed = results.filter((r) => !r.ok);
console.log(JSON.stringify({ passed: results.length - failed.length, total: results.length, results }, null, 2));
process.exit(failed.length ? 1 : 0);
