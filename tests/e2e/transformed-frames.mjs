// CDP geometry and pointer coordinates through transformed iframe owners.
// Usage: bun tests/e2e/transformed-frames.mjs /absolute/path/to/pyro
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
  console.error('usage: bun tests/e2e/transformed-frames.mjs /abs/path/to/pyro');
  process.exit(2);
}

const results = [];
const deadline = setTimeout(() => {
  console.error(`transformed-frames: deadline exceeded; completed ${JSON.stringify(results)}`);
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
  res.end(req.url === '/nested' ? `<!doctype html><style>body{margin:0} iframe{position:absolute;left:20px;top:10px;width:400px;height:200px;border:5px solid;padding:3px;transform:scale(.5);transform-origin:0 0}</style><iframe src=/child></iframe>` : req.url === '/child' ? `<!doctype html><style>body { margin:0 } button { position:absolute; left:300px; top:100px; width:60px; height:30px }</style><button id=target>Next</button><script>window.hits=[]; document.getElementById('target').onclick=e=>hits.push([Math.round(e.clientX),Math.round(e.clientY),e.isTrusted]);</script>` : `<!doctype html><style>body { margin:0 } #wrap { position:absolute; left:40px; top:30px; transform-origin:0 0; transform:scale(.8,.6) } iframe { width:400px;height:200px;border:5px solid;padding:3px }</style><div id=wrap><iframe src=/child></iframe></div>`);
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

function assertCenterClick(hits) {
  assert.equal(hits.length, 1);
  assert.equal(hits[0][2], true);
  // Playwright rounds root coordinates to integer pixels; inverse scaling
  // magnifies that rounding by up to two child CSS pixels in these fixtures.
  assert.ok(Math.abs(hits[0][0]-330)<=2, JSON.stringify(hits));
  assert.ok(Math.abs(hits[0][1]-115)<=2, JSON.stringify(hits));
}
const frame = page.frameLocator('iframe');
await frame.locator('#target').waitFor();
await check('child layout viewport is not scaled', async () => {
  assert.deepEqual(await frame.locator('body').evaluate(()=>[innerWidth,innerHeight]), [400,200]);
});
await check('scaled frame geometry is in the root viewport', async () => {
  const box = await frame.locator('#target').boundingBox();
  for (const [key, expected] of Object.entries({x:286.4,y:94.8,width:48,height:18})) {
    assert.ok(Math.abs(box[key]-expected)<.05, `${key}: ${box[key]} != ${expected}`);
  }
});
await check('root input is inverted into the scaled child viewport', async () => {
  await page.mouse.click(310.4,103.8);
  assertCenterClick(await frame.locator('body').evaluate(()=>window.hits));
});
await check('Playwright locator clicks reach the scaled child', async () => {
  await frame.locator('body').evaluate(()=>window.hits=[]);
  await frame.locator('#target').click({timeout:2000});
  assertCenterClick(await frame.locator('body').evaluate(()=>window.hits));
});
await check('touch points and moves use the inverse frame transform', async () => {
  await frame.locator('body').evaluate(() => {
    window.touches=[];
    for (const type of ['touchstart','touchmove']) document.addEventListener(type, e => {
      e.preventDefault();
      window.touches.push([...e.touches].map(t=>[Math.round(t.clientX),Math.round(t.clientY)]));
    }, {passive:false});
  });
  const cdp = await context.newCDPSession(page);
  await cdp.send('Input.dispatchTouchEvent', {type:'touchStart',touchPoints:[{id:1,x:298.4,y:103.8},{id:2,x:322.4,y:103.8}]});
  await cdp.send('Input.dispatchTouchEvent', {type:'touchMove',touchPoints:[{id:1,x:306.4,y:103.8},{id:2,x:330.4,y:103.8}]});
  await cdp.send('Input.dispatchTouchEvent', {type:'touchEnd',touchPoints:[]});
  assert.deepEqual(await frame.locator('body').evaluate(()=>window.touches), [[[315,115]],[[315,115],[345,115]],[[325,115],[355,115]]]);
  await cdp.detach();
});
await check('nested frame transforms compose in order', async () => {
  await page.locator('iframe').evaluate(f => f.src='/nested');
  const inner = page.frameLocator('iframe').frameLocator('iframe');
  await inner.locator('#target').waitFor();
  const box = await inner.locator('#target').boundingBox();
  for (const [key, expected] of Object.entries({x:185.6,y:73.2,width:24,height:9})) {
    assert.ok(Math.abs(box[key]-expected)<.05, `${key}: ${box[key]} != ${expected}`);
  }
  await page.mouse.click(197.6,77.7);
  assertCenterClick(await inner.locator('body').evaluate(()=>window.hits));
  await inner.locator('#target').click({timeout:2000});
  assert.equal(await inner.locator('body').evaluate(()=>window.hits.length), 2);
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
