// Pointer boundary events when a press opens an overlay beneath the pointer.
// Usage: bun tests/e2e/stationary-hover.mjs /absolute/path/to/pyro
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
  console.error('usage: bun tests/e2e/stationary-hover.mjs /abs/path/to/pyro');
  process.exit(2);
}

const results = [];
const deadline = setTimeout(() => {
  console.error(`stationary-hover: deadline exceeded; completed ${JSON.stringify(results)}`);
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
  res.end(`<!doctype html><style>body{margin:0} #target{position:absolute;left:20px;top:20px;width:150px;height:50px} #cover{position:fixed;inset:0;background:white}</style><button id=target>Next</button><script>window.events=[];for(const type of ['pointerleave','pointerenter','mouseleave','mouseenter','mousemove'])document.addEventListener(type,e=>events.push([type,e.target.id,e.relatedTarget?.id]),true);</script>`);
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
await page.goto(`${origin}/`);

await check('pointer release updates boundaries after an overlay opens on press',async()=>{
  await page.mouse.move(50,40);
  await page.evaluate(()=>{window.events=[];document.getElementById('target').onpointerdown=()=>{const cover=document.createElement('div');cover.id='cover';document.body.append(cover)}});
  await page.mouse.down();
  await page.mouse.up();
  assert.deepEqual(await page.evaluate(()=>window.events), [
    ['pointerleave','target','cover'],['pointerenter','cover','target'],
    ['mouseleave','target','cover'],['mouseenter','cover','target']
  ]);
});
await check('layout change after click updates stationary hover',async()=>{
  await page.goto(origin);
  await page.mouse.move(50,40);
  await page.evaluate(()=>{window.events=[];document.getElementById('target').onclick=()=>{const cover=document.createElement('div');cover.id='cover';document.body.append(cover)}});
  await page.mouse.down();await page.mouse.up();
  await page.waitForTimeout(250);
  assert.deepEqual(await page.evaluate(()=>window.events), [
    ['pointerleave','target','cover'],['pointerenter','cover','target'],
    ['mouseleave','target','cover'],['mouseenter','cover','target']
  ]);
  await page.evaluate(()=>{window.events=[];const cover=document.getElementById('cover');cover.onclick=()=>cover.style.display='none'});
  await page.mouse.down();await page.mouse.up();
  await page.waitForTimeout(200);
  assert.deepEqual(await page.evaluate(()=>window.events), [
    ['pointerleave','cover','target'],['pointerenter','target','cover'],
    ['mouseleave','cover','target'],['mouseenter','target','cover']
  ]);
});
await check('pointer capture defers boundary changes until release',async()=>{
  await page.goto(origin);
  await page.mouse.move(50,40);
  await page.evaluate(()=>{window.events=[];document.getElementById('target').onpointerdown=e=>{e.currentTarget.setPointerCapture(e.pointerId);const cover=document.createElement('div');cover.id='cover';document.body.append(cover)}});
  await page.mouse.down();
  await page.waitForTimeout(200);
  assert.deepEqual(await page.evaluate(()=>window.events), []);
  await page.mouse.up();
  await page.waitForTimeout(200);
  assert.deepEqual(await page.evaluate(()=>window.events), [
    ['pointerleave','target','cover'],['pointerenter','cover','target'],
    ['mouseleave','target','cover'],['mouseenter','cover','target']
  ]);
});
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
