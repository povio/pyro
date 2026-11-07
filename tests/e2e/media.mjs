// Real Playwright 1.59.1 (connectOverCDP) against `lightningpanda serve`:
// media elements as Chrome runs them, with small generated files. The
// resource is fetched as a Media request, its container headers give
// duration and video size before the window load event, play() (from
// page.evaluate, a user gesture) advances currentTime on the page clock
// until `ended`, garbage fails with MEDIA_ERR_SRC_NOT_SUPPORTED, and a
// screenshot paints the poster. Real AV1 and H.264 files (lp-media's
// fixtures: frame N of ten is rgb(N*25, 255-N*25, 128)) paint their current
// frame in screenshots, draw on a canvas and run requestVideoFrameCallback;
// Web Audio renders offline and decodes an MP3.
//
// Usage: bun tests/e2e/media.mjs /abs/path/to/lightningpanda
// Requires `bun install` in bench/ for the pinned playwright-core.
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import http from 'node:http';
import net from 'node:net';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(here, '..', '..', 'bench', 'package.json'));
const { chromium } = require('playwright-core');

const bin = process.argv[2];
if (!bin) {
  console.error('usage: bun tests/e2e/media.mjs /abs/path/to/lightningpanda');
  process.exit(2);
}

const results = [];
const deadline = setTimeout(() => {
  console.error(`media: deadline exceeded; completed ${JSON.stringify(results)}`);
  process.exit(1);
}, 60_000);
const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer();
  s.on('error', reject);
  s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); });
});

// --- generated files ---------------------------------------------------------
const u32 = (n) => { const b = Buffer.alloc(4); b.writeUInt32BE(n >>> 0); return b; };
const box = (type, ...payload) => { const body = Buffer.concat(payload); return Buffer.concat([u32(body.length + 8), Buffer.from(type), body]); };
// An MP4 of `seconds` with one H.264 video track of `w`x`h`.
function mp4(seconds, w, h) {
  const mvhd = Buffer.concat([Buffer.alloc(12), u32(1000), u32(Math.round(seconds * 1000)), Buffer.alloc(80)]);
  const matrix = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000].map(u32);
  const tkhd = Buffer.concat([Buffer.alloc(40), ...matrix, u32(w * 65536), u32(h * 65536)]);
  const hdlr = Buffer.concat([Buffer.alloc(8), Buffer.from('vide'), Buffer.alloc(13)]);
  const stsd = Buffer.concat([Buffer.alloc(4), u32(1), box('avc1', Buffer.alloc(70))]);
  const trak = box('trak', box('tkhd', tkhd), box('mdia', box('hdlr', hdlr), box('minf', box('stbl', box('stsd', stsd)))));
  return Buffer.concat([box('ftyp', Buffer.from('isom\0\0\x02\0isomavc1')), box('moov', box('mvhd', mvhd), trak), box('mdat', Buffer.alloc(16))]);
}
// A 16-bit mono PCM WAV of `seconds` at 8 kHz.
function wav(seconds) {
  const data = Buffer.alloc(Math.round(seconds * 16000));
  const head = Buffer.alloc(44);
  head.write('RIFF', 0); head.writeUInt32LE(36 + data.length, 4); head.write('WAVEfmt ', 8);
  head.writeUInt32LE(16, 16); head.writeUInt16LE(1, 20); head.writeUInt16LE(1, 22);
  head.writeUInt32LE(8000, 24); head.writeUInt32LE(16000, 28); head.writeUInt16LE(2, 32); head.writeUInt16LE(16, 34);
  head.write('data', 36); head.writeUInt32LE(data.length, 40);
  return Buffer.concat([head, data]);
}
const crcTable = Array.from({ length: 256 }, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
const crc32 = (buf) => { let c = 0xffffffff; for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
const chunk = (type, data) => Buffer.concat([u32(data.length), Buffer.from(type), data, u32(crc32(Buffer.concat([Buffer.from(type), data])))]);
// A solid `w`x`h` RGB PNG.
function png(w, h, [r, g, b]) {
  const row = Buffer.concat([Buffer.from([0]), Buffer.from(Array.from({ length: w }, () => [r, g, b]).flat())]);
  const ihdr = Buffer.concat([u32(w), u32(h), Buffer.from([8, 2, 0, 0, 0])]);
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(Buffer.concat(Array(h).fill(row)))), chunk('IEND', Buffer.alloc(0))]);
}
// The RGB of a screenshot's first pixel (8-bit RGB or RGBA, filter 0 or 1 on row one).
function firstPixel(shot) {
  let at = 8; let ihdr; const idat = [];
  while (at < shot.length) {
    const len = shot.readUInt32BE(at); const type = shot.toString('latin1', at + 4, at + 8); const data = shot.subarray(at + 8, at + 8 + len);
    if (type === 'IHDR') ihdr = data; else if (type === 'IDAT') idat.push(data);
    at += 12 + len;
  }
  const raw = zlib.inflateSync(Buffer.concat(idat));
  assert.equal(ihdr[8], 8, 'bit depth');
  return [raw[1], raw[2], raw[3]];
}

const fixture = (name) => readFileSync(path.join(here, '..', '..', 'crates', 'lp-media', 'tests', 'fixtures', name));
const FILES = {
  '/frames-h264.mp4': ['video/mp4', fixture('frames-h264.mp4')],
  '/frames-av1.webm': ['video/webm', fixture('frames-av1.webm')],
  '/sine.mp3': ['audio/mpeg', fixture('sine.mp3')],
  '/movie.mp4': ['video/mp4', mp4(1.5, 320, 180)],
  '/short.mp4': ['video/mp4', mp4(0.4, 64, 48)],
  '/ding.wav': ['audio/wav', wav(0.25)],
  '/bad.mp4': ['video/mp4', Buffer.from('<!doctype html><p>not a movie')],
  '/poster.png': ['image/png', png(8, 8, [255, 0, 0])],
};
const PAGE = `<!doctype html><html><head><style>body { margin: 0 } video { display: block; width: 160px; height: 90px }</style><script>
globalThis.events = [];
for (const type of ['loadstart', 'durationchange', 'resize', 'loadedmetadata', 'loadeddata', 'canplay', 'canplaythrough', 'error', 'play', 'playing', 'pause', 'ended', 'seeking', 'seeked'])
  document.addEventListener(type, (e) => events.push(e.target.id + ':' + type), true);
addEventListener('load', () => events.push('window:load:' + document.getElementById('v').readyState));
</script></head><body><video id="v" poster="/poster.png" src="/movie.mp4"></video><audio id="a" src="/ding.wav"></audio></body></html>`;

// A 64x48 video at the top left and a canvas; `?` names the file.
const FRAMES_PAGE = (query) => `<!doctype html><html><head><style>body { margin: 0 } video { display: block; width: 64px; height: 48px }</style></head>
<body><video id="f" muted src="/${query.split('?')[1]}"></video><canvas id="c" width="64" height="48"></canvas></body></html>`;
// The frame index of an RGB pixel.
const frameOf = ([r, g]) => Math.round((r / 25 + (255 - g) / 25) / 2);

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
  const file = FILES[req.url];
  res.writeHead(200, { 'content-type': file ? file[0] : 'text/html' });
  res.end(file ? file[1] : req.url.startsWith('/frames') ? FRAMES_PAGE(req.url.slice(1)) : PAGE);
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
const context = await browser.newContext({ viewport: { width: 320, height: 200 } });
const page = await context.newPage();
const mediaRequests = [];
page.on('request', (r) => { if (r.resourceType() === 'media') mediaRequests.push(new URL(r.url()).pathname); });

await page.goto(`${origin}/`);

await check('metadata before the load event; Media requests', async () => {
  const state = await page.evaluate(() => {
    const v = document.getElementById('v'); const a = document.getElementById('a');
    return { events: events.filter((e) => e.startsWith('v:') || e.startsWith('window')), ready: v.readyState, duration: v.duration, size: [v.videoWidth, v.videoHeight], audio: a.duration };
  });
  assert.deepEqual(state.events.slice(0, 5), ['v:loadstart', 'v:durationchange', 'v:resize', 'v:loadedmetadata', 'v:loadeddata'], JSON.stringify(state.events));
  // The media elements delayed the load event until they had data.
  assert.ok(state.events.findIndex((e) => e.startsWith('window:load')) > state.events.indexOf('v:loadeddata'), JSON.stringify(state.events));
  assert.equal(state.ready, 4);
  assert.equal(state.duration, 1.5);
  assert.deepEqual(state.size, [320, 180]);
  assert.equal(state.audio, 0.25);
  assert.deepEqual(mediaRequests.sort(), ['/ding.wav', '/movie.mp4']);
});

await check('play() from page.evaluate (a user gesture) advances to ended', async () => {
  const before = await page.evaluate(async () => {
    const v = document.getElementById('v');
    await v.play();
    return [v.paused, v.muted];
  });
  assert.deepEqual(before, [false, false]);
  await page.waitForFunction(() => document.getElementById('v').currentTime > 0.2);
  const ended = await page.evaluate(() => new Promise((resolve) => {
    const v = document.getElementById('v');
    v.playbackRate = 4;
    v.addEventListener('ended', () => resolve([v.ended, v.paused, v.currentTime]), { once: true });
  }));
  assert.deepEqual(ended, [true, true, 1.5]);
});

await check('seeking fires seeking/seeked and moves currentTime', async () => {
  const seek = await page.evaluate(() => new Promise((resolve) => {
    const v = document.getElementById('v');
    v.addEventListener('seeked', () => resolve([v.currentTime, v.seeking]), { once: true });
    v.currentTime = 0.5;
  }));
  assert.deepEqual(seek, [0.5, false]);
});

await check('canPlayType answers like Chrome', async () => {
  const answers = await page.evaluate(() => ['video/mp4', 'video/mp4; codecs="avc1.42E01E, mp4a.40.2"', 'audio/mpeg', 'video/webm; codecs="vp9, opus"', 'audio/ogg; codecs=opus', 'video/ogg; codecs=theora', 'application/octet-stream'].map((t) => document.createElement('video').canPlayType(t)));
  // No VP9 decoder: Chrome's answers for the codecs this build decodes.
  assert.deepEqual(answers, ['maybe', 'probably', 'probably', '', 'probably', '', '']);
});

await check('garbage fails with MEDIA_ERR_SRC_NOT_SUPPORTED', async () => {
  const error = await page.evaluate(() => new Promise((resolve) => {
    const v = document.createElement('video');
    v.onerror = () => resolve([v.error.code, v.networkState, v.readyState]);
    v.src = '/bad.mp4';
  }));
  assert.deepEqual(error, [4, 3, 0]);
});

await check('muted autoplay plays; the short clip ends', async () => {
  const done = await page.evaluate(() => new Promise((resolve) => {
    const v = document.createElement('video');
    v.muted = true; v.autoplay = true;
    v.onended = () => resolve(events.filter((e) => e.startsWith('auto:')));
    v.id = 'auto'; v.src = '/short.mp4';
    document.body.append(v);
  }));
  assert.ok(done.includes('auto:playing') && done.at(-1) === 'auto:ended', JSON.stringify(done));
});

await check('screenshot paints the poster', async () => {
  const shot = await page.screenshot({ clip: { x: 80, y: 45, width: 1, height: 1 } });
  const [r, g, b] = firstPixel(shot);
  assert.ok(r > 200 && g < 50 && b < 50, `poster pixel ${[r, g, b]}`);
});

for (const file of ['frames-h264.mp4', 'frames-av1.webm']) {
  await check(`${file}: screenshots, drawImage and requestVideoFrameCallback show the current frame`, async () => {
    await page.goto(`${origin}/frames?${file}`);
    const shot = async () => frameOf(firstPixel(await page.screenshot({ clip: { x: 32, y: 24, width: 1, height: 1 } })));
    assert.equal(await shot(), 0);
    const seeked = await page.evaluate(() => new Promise((resolve) => {
      const v = document.getElementById('f');
      v.requestVideoFrameCallback((now, meta) => resolve([meta.mediaTime, meta.width, meta.height]));
      v.currentTime = 0.55;
    }));
    assert.deepEqual(seeked, [0.5, 64, 48]);
    assert.equal(await shot(), 5);
    const drawn = await page.evaluate(() => {
      const c = document.getElementById('c').getContext('2d');
      c.drawImage(document.getElementById('f'), 0, 0);
      return [...c.getImageData(32, 24, 1, 1).data];
    });
    assert.equal(frameOf(drawn), 5);
    // Playback presents later frames to the frame callbacks.
    const times = await page.evaluate(() => new Promise((resolve) => {
      const v = document.getElementById('f');
      const seen = [];
      const each = (now, meta) => { seen.push(meta.mediaTime); if (seen.length < 3) v.requestVideoFrameCallback(each); else resolve(seen); };
      v.requestVideoFrameCallback(each);
      v.play();
    }));
    assert.ok(times[0] < times[1] && times[1] < times[2], JSON.stringify(times));
  });
}

await check('Web Audio renders offline and decodes an MP3', async () => {
  const out = await page.evaluate(async () => {
    const ctx = new OfflineAudioContext(1, 4410, 44100);
    const osc = new OscillatorNode(ctx, { frequency: 441 });
    const gain = new GainNode(ctx, { gain: 0.5 });
    osc.connect(gain).connect(ctx.destination);
    osc.start();
    const rendered = await ctx.startRendering();
    const d = rendered.getChannelData(0);
    let up = 0;
    for (let i = 1; i < d.length; i++) if (d[i - 1] <= 0 && d[i] > 0) up++;
    const mp3 = await new AudioContext().decodeAudioData(await (await fetch('/sine.mp3')).arrayBuffer());
    return [ctx.state, up, Math.round(Math.max(...d) * 100) / 100, mp3.sampleRate, mp3.numberOfChannels, Math.round(mp3.duration * 10) / 10];
  });
  assert.deepEqual(out, ['closed', 44, 0.5, 48000, 1, 0.5]);
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
