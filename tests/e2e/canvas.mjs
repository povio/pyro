// Real Playwright (connectOverCDP) against `lightningpanda serve`: canvas 2D
// rasterization through the real browser. Pixels drawn by script read back
// exactly (getImageData), a loaded same-origin <img> draws its decoded
// pixels, a cross-origin one taints the canvas (SecurityError on readback),
// toDataURL round-trips, and Page.captureScreenshot shows the canvas.
//
// Usage: bun tests/e2e/canvas.mjs /abs/path/to/lightningpanda
// Requires `bun install` in bench/ for the pinned playwright-core.
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import http from 'node:http';
import net from 'node:net';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(here, '..', '..', 'bench', 'package.json'));
const { chromium } = require('playwright-core');

const bin = process.argv[2];
if (!bin) {
  console.error('usage: bun tests/e2e/canvas.mjs /abs/path/to/lightningpanda');
  process.exit(2);
}

const results = [];
const deadline = setTimeout(() => {
  console.error(`canvas: deadline exceeded; completed ${JSON.stringify(results)}`);
  process.exit(1);
}, 60_000);
const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer();
  s.on('error', reject);
  s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); });
});

// 2x2 RGBA PNG: red, lime / blue, white.
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEklEQVR4nGP4z8DwHwyBNBgAAEnICff5q7YNAAAAAElFTkSuQmCC', 'base64');

/** RGBA rows of an 8-bit RGB/RGBA non-interlaced PNG (screenshots). */
function decodePng(buf) {
  let pos = 8, width = 0, height = 0, type = 0;
  const idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const name = buf.toString('latin1', pos + 4, pos + 8);
    const data = buf.subarray(pos + 8, pos + 8 + len);
    if (name === 'IHDR') { width = data.readUInt32BE(0); height = data.readUInt32BE(4); type = data[9]; }
    if (name === 'IDAT') idat.push(data);
    pos += 12 + len;
  }
  const channels = type === 6 ? 4 : 3;
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const out = Buffer.alloc(width * height * 4);
  let prev = Buffer.alloc(stride);
  for (let y = 0; y < height; y++) {
    const f = raw[y * (stride + 1)];
    const line = Buffer.from(raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1)));
    for (let i = 0; i < stride; i++) {
      const a = i >= channels ? line[i - channels] : 0, b = prev[i], c = i >= channels ? prev[i - channels] : 0;
      const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
      const pred = [0, a, b, (a + b) >> 1, pa <= pb && pa <= pc ? a : pb <= pc ? b : c][f];
      line[i] = (line[i] + pred) & 255;
    }
    for (let x = 0; x < width; x++) {
      for (let k = 0; k < 4; k++) out[(y * width + x) * 4 + k] = k < channels ? line[x * channels + k] : 255;
    }
    prev = line;
  }
  return { width, height, pixel: (x, y) => Array.from(out.subarray((y * width + x) * 4, (y * width + x) * 4 + 4)) };
}

const PAGE = `<!doctype html><html><body style="margin:0;background:white">
<canvas id="c" width="120" height="60" style="display:block"></canvas>
<img id="same" src="/img.png"><img id="cross">
<script>document.getElementById('cross').src = location.protocol + '//localhost:' + CROSS_PORT + '/img.png';</script>
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

const serve = (page) => http.createServer((req, res) => {
  if (req.url === '/img.png') { res.writeHead(200, { 'content-type': 'image/png' }); res.end(PNG); return; }
  res.writeHead(200, { 'content-type': 'text/html' });
  res.end(page);
});
const cross = serve('');
await new Promise((resolve, reject) => { cross.once('error', reject); cross.listen(0, '127.0.0.1', resolve); });
const server = serve(PAGE.replace('CROSS_PORT', String(cross.address().port)));
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
await page.goto(`${origin}/`, { waitUntil: 'load' });

await check('fillRect, arc and globalCompositeOperation read back exactly', async () => {
  const px = await page.evaluate(() => {
    const x = document.getElementById('c').getContext('2d');
    x.fillStyle = 'red'; x.fillRect(0, 0, 40, 30);
    x.fillStyle = '#0000ff'; x.beginPath(); x.arc(80, 30, 20, 0, Math.PI * 2); x.fill();
    x.globalCompositeOperation = 'destination-out'; x.fillRect(0, 0, 10, 10); x.globalCompositeOperation = 'source-over';
    const p = (X, Y) => Array.from(x.getImageData(X, Y, 1, 1).data);
    return [p(20, 20), p(80, 30), p(5, 5), p(110, 5)];
  });
  assert.deepEqual(px, [[255, 0, 0, 255], [0, 0, 255, 255], [0, 0, 0, 0], [0, 0, 0, 0]]);
});

await check('drawImage of a loaded same-origin <img> draws its pixels', async () => {
  const px = await page.evaluate(() => {
    const img = document.getElementById('same');
    const y = document.createElement('canvas').getContext('2d');
    y.imageSmoothingEnabled = false;
    y.drawImage(img, 0, 0, 4, 4);
    const p = (X, Y) => Array.from(y.getImageData(X, Y, 1, 1).data);
    return [img.complete, img.naturalWidth, p(0, 0), p(3, 0), p(0, 3), p(3, 3)];
  });
  assert.deepEqual(px, [true, 2, [255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255]]);
});

await check('a cross-origin <img> taints the canvas', async () => {
  const out = await page.evaluate(async () => {
    const img = document.getElementById('cross');
    if (!img.complete) await new Promise((r) => { img.onload = img.onerror = r; });
    const y = document.createElement('canvas').getContext('2d');
    y.drawImage(img, 0, 0);
    const errors = [];
    try { y.getImageData(0, 0, 1, 1); } catch (e) { errors.push(e.name); }
    try { y.canvas.toDataURL(); } catch (e) { errors.push(e.name); }
    return [img.naturalWidth, errors];
  });
  assert.deepEqual(out, [2, ['SecurityError', 'SecurityError']]);
});

await check('toDataURL round-trips through an <img>', async () => {
  const px = await page.evaluate(async () => {
    const url = document.getElementById('c').toDataURL();
    const img = new Image();
    await new Promise((r) => { img.onload = r; img.src = url; });
    const y = document.createElement('canvas').getContext('2d');
    y.drawImage(img, 0, 0);
    return [url.slice(0, 22), img.naturalWidth, Array.from(y.getImageData(20, 20, 1, 1).data)];
  });
  assert.deepEqual(px, ['data:image/png;base64,', 120, [255, 0, 0, 255]]);
});

await check('Page.captureScreenshot shows the canvas', async () => {
  const shot = decodePng(await page.screenshot({ clip: { x: 0, y: 0, width: 120, height: 60 } }));
  assert.equal(shot.width, 120);
  assert.deepEqual(shot.pixel(20, 20).slice(0, 3), [255, 0, 0]);
  assert.deepEqual(shot.pixel(80, 30).slice(0, 3), [0, 0, 255]);
  // Erased and undrawn pixels show the page background.
  assert.deepEqual(shot.pixel(5, 5).slice(0, 3), [255, 255, 255]);
  // Drawing again shows in the next screenshot.
  await page.evaluate(() => { const x = document.getElementById('c').getContext('2d'); x.fillStyle = 'lime'; x.fillRect(100, 0, 20, 20); });
  const again = decodePng(await page.screenshot({ clip: { x: 0, y: 0, width: 120, height: 60 } }));
  assert.deepEqual(again.pixel(110, 10).slice(0, 3), [0, 255, 0]);
});

clearTimeout(deadline);
await browser.close().catch(() => {});
server.close();
cross.close();
child.kill('SIGKILL');
const failed = results.filter((r) => !r.ok);
console.log(JSON.stringify(results, null, 2));
if (failed.length) {
  console.error(`canvas: ${failed.length} of ${results.length} checks failed`);
  process.exit(1);
}
console.log(`canvas: ${results.length} checks passed`);
