// Real Playwright 1.59.1 (connectOverCDP) against `lightningpanda serve`:
// `:has()` follows element and form state the user changes. Mouse hover and
// press, clicks on checkboxes, typing then leaving a field (user validity),
// a submit attempt, selecting an option and keyboard focus each restyle the
// `:has()` anchors that can see the changed element, read back through
// getComputedStyle.
//
// Usage: bun tests/e2e/has-state-invalidation.mjs /abs/path/to/lightningpanda
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
  console.error('usage: bun tests/e2e/has-state-invalidation.mjs /abs/path/to/lightningpanda');
  process.exit(2);
}

const results = [];
const deadline = setTimeout(() => {
  console.error(`has-state-invalidation: deadline exceeded; completed ${JSON.stringify(results)}`);
  process.exit(1);
}, 60_000);
const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer();
  s.on('error', reject);
  s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); });
});

const PAGE = `<!doctype html><html><head><style>
  body { margin: 0 }
  .row { height: 30px }
  #hv:has(#target:hover) { color: rgb(0, 0, 255) }
  #hv:has(#target:hover) .other { color: rgb(0, 0, 128) }
  #hv:has(#target:hover:active) { color: rgb(135, 206, 235) }
  label:has(input:checked) { color: rgb(0, 128, 0) }
  #f:has(:user-invalid) { color: rgb(255, 0, 0) }
  #f:has(:user-valid) .status { color: rgb(0, 128, 0) }
  #sl:has(option[value=b]:checked) { color: rgb(128, 0, 128) }
  #fv:has(:focus-visible) { color: rgb(154, 205, 50) }
</style></head><body>
<div id=hv><div class=row id=away>away</div><div class=row id=target>hover me</div><p class=other id=other>o</p></div>
<label id=lab class=row><input type=checkbox id=cb> check</label>
<form id=f><input id=email type=email required><span class=status id=status>s</span><button id=go>go</button></form>
<div id=sl><select id=sel><option value=a>a</option><option value=b>b</option></select></div>
<div id=fv><button id=fvb>focus me</button></div>
</body></html>`;

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
  res.writeHead(200, { 'content-type': 'text/html' });
  res.end(PAGE);
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
await page.goto(`${origin}/`);

const color = (id) => page.evaluate((id) => getComputedStyle(document.getElementById(id)).color, id);
const BLACK = 'rgb(0, 0, 0)';

await check('hover and press inside the anchor', async () => {
  assert.equal(await color('hv'), BLACK);
  await page.hover('#target');
  assert.equal(await color('hv'), 'rgb(0, 0, 255)');
  assert.equal(await color('other'), 'rgb(0, 0, 128)');
  await page.mouse.down();
  assert.equal(await color('hv'), 'rgb(135, 206, 235)');
  await page.mouse.up();
  assert.equal(await color('hv'), 'rgb(0, 0, 255)');
  await page.hover('#away');
  assert.equal(await color('hv'), BLACK);
  assert.equal(await color('other'), BLACK);
});

await check('clicking a checkbox restyles its label', async () => {
  await page.click('#cb');
  assert.equal(await color('lab'), 'rgb(0, 128, 0)');
  await page.click('#cb');
  assert.equal(await color('lab'), BLACK);
});

await check(':user-invalid only after the user commits a value', async () => {
  assert.equal(await color('f'), BLACK, 'no interaction yet');
  await page.fill('#email', 'not an email');
  await page.evaluate(() => document.getElementById('email').blur());
  assert.equal(await color('f'), 'rgb(255, 0, 0)', 'typed an invalid email');
  await page.fill('#email', 'a@b.c');
  await page.evaluate(() => document.getElementById('email').blur());
  assert.equal(await color('f'), BLACK);
  assert.equal(await color('status'), 'rgb(0, 128, 0)', 'user-valid');
  await page.evaluate(() => document.getElementById('f').reset());
  assert.equal(await color('status'), BLACK, 'reset clears user validity');
  await page.evaluate(() => document.getElementById('f').addEventListener('submit', (e) => e.preventDefault()));
  await page.click('#go');
  assert.equal(await color('f'), 'rgb(255, 0, 0)', 'a submit attempt validates the form');
});

await check('selecting an option', async () => {
  assert.equal(await color('sl'), BLACK);
  await page.selectOption('#sel', 'b');
  assert.equal(await color('sl'), 'rgb(128, 0, 128)');
  await page.selectOption('#sel', 'a');
  assert.equal(await color('sl'), BLACK);
});

await check('keyboard focus is :focus-visible', async () => {
  await page.focus('#fvb');
  await page.keyboard.press('Shift');
  const fv = await color('fv');
  await page.evaluate(() => document.activeElement.blur());
  assert.equal(await color('fv'), BLACK);
  assert.equal(fv, 'rgb(154, 205, 50)');
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
