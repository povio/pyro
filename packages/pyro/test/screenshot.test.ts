// matchScreenshot end to end: a real pyro binary built with the
// `render` feature (PYRO_BIN, else target/{release,debug} of this
// checkout), a page served by Bun, baselines in a temporary directory.
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { FrameFifo, Pyro, ScreenshotMismatchError, css, evaluate, goto, label, matchScreenshot, screenshotMatchers } from '../src/index';

const root = join(import.meta.dir, '../../..');
const bin =
  process.env.PYRO_BIN ??
  [join(root, 'target/release/pyro'), join(root, 'target/debug/pyro')].find(existsSync);

// A 2x2 red PNG, served as an image the page shows.
const RED = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4z8AARAwQCgAf7gP9i18U1AAAAABJRU5ErkJggg==',
  'base64',
);
const PAGE = `<!doctype html><html><head><style>
  body { margin: 0; background: #f3f4f6; font-family: sans-serif; }
  #card { width: 300px; height: 120px; margin: 40px; background: #6d28d9; border-radius: 12px; }
  img { width: 40px; height: 40px; display: block; margin-left: 40px; }
</style></head><body>
  <div id="card"></div><img src="/red.png"><label>Name <input id="name"></label>
</body></html>`;

let server: ReturnType<typeof Bun.serve>;
let lp: Pyro;
let dir: string;

beforeAll(async () => {
  server = Bun.serve({
    port: 0,
    fetch: (req) =>
      new URL(req.url).pathname === '/red.png'
        ? new Response(RED, { headers: { 'content-type': 'image/png' } })
        : new Response(PAGE, { headers: { 'content-type': 'text/html' } }),
  });
  dir = mkdtempSync(join(tmpdir(), 'pyro-shots-'));
  if (bin) lp = await Pyro.launch({ bin });
});

afterAll(() => {
  lp?.close();
  server?.stop(true);
  rmSync(dir, { recursive: true, force: true });
});

test.skipIf(!bin)('records, matches, hits the hash, then reports a change with images', async () => {
  await using page = await lp.page({ url: `http://127.0.0.1:${server.port}/` });
  await page.send('Emulation.setDeviceMetricsOverride', { width: 400, height: 300, deviceScaleFactor: 1, mobile: false });

  // No baseline yet: the render is written and the step fails, as Playwright does.
  const missing = page.run([matchScreenshot('card', { dir })]);
  expect(missing).rejects.toBeInstanceOf(ScreenshotMismatchError);
  await missing.catch(() => {});
  expect(existsSync(join(dir, 'card.png'))).toBe(true);
  const meta = JSON.parse(readFileSync(join(dir, 'card.json'), 'utf8'));
  expect(meta.sceneHash).toMatch(/^[0-9a-f]{16}$/);

  // The image came through the page's network: its red pixels are in the baseline.
  const first = await page.matchScreenshot('card', { dir });
  expect(first.pass).toBe(true);
  expect(first.hashHit).toBe(true); // the sidecar's scene hash
  const again = await page.matchScreenshot('card', { dir });
  expect(again.hashHit).toBe(true);
  expect(again.ms.raster).toBe(0);

  expect.extend(screenshotMatchers);
  await (expect(page) as any).toMatchScreenshot('card', { dir });

  const changed = page.run([
    evaluate(`document.getElementById('card').style.background = '#6d28da'`),
    matchScreenshot('card', { dir }),
  ]);
  const error = await changed.catch((e) => e);
  expect(error).toBeInstanceOf(ScreenshotMismatchError);
  expect(error.result.diffPixels).toBeGreaterThan(30_000);
  expect(existsSync(join(dir, 'card.actual.png'))).toBe(true);
  expect(existsSync(join(dir, 'card.diff.png'))).toBe(true);
});

test.skipIf(!bin)('form state and elements; update mode rewrites the baseline', async () => {
  await using page = await lp.page({ url: `http://127.0.0.1:${server.port}/` });
  process.env.PYRO_UPDATE_SNAPSHOTS = '1';
  try {
    const r = await page.run([css('#card').matchScreenshot('element', { dir }), matchScreenshot('form', { dir })]);
    expect(r.screenshots.element.recorded).toBe(true);
    expect([r.screenshots.element.width, r.screenshots.element.height]).toEqual([300, 120]);
  } finally {
    delete process.env.PYRO_UPDATE_SNAPSHOTS;
  }
  const typed = page.run([label('Name').fill('Ada Lovelace'), matchScreenshot('form', { dir })]);
  const error = await typed.catch((e) => e);
  expect(error).toBeInstanceOf(ScreenshotMismatchError);
  expect(error.result.diffPixels).toBeGreaterThan(0);
});

/** An ffmpeg on PATH that reads raw video (Homebrew's, distributions'). */
const ffmpeg = spawnSync('ffmpeg', ['-hide_banner', '-demuxers'], { encoding: 'utf8' }).stdout?.match(/^ *D\S* +rawvideo /m) ? 'ffmpeg' : undefined;

test.skipIf(!bin)('captureFrame: raw pixels, appended to a file, the browser’s timings and CPU time', async () => {
  await using page = await lp.page({ url: `http://127.0.0.1:${server.port}/` });
  await page.send('Emulation.setDeviceMetricsOverride', { width: 400, height: 300, deviceScaleFactor: 2, mobile: false });
  const rgba = await page.captureFrame();
  expect([rgba.width, rgba.height, rgba.bytes, rgba.data.length]).toEqual([800, 600, 800 * 600 * 4, 800 * 600 * 4]);
  // The card's purple at (100, 100) CSS px.
  const i = (200 * 800 + 200) * 4;
  expect([...rgba.data.subarray(i, i + 4)]).toEqual([0x6d, 0x28, 0xd9, 255]);
  expect(rgba.ms.cpu).toBeGreaterThan(0);
  expect(rgba.ms.total).toBeGreaterThanOrEqual(rgba.ms.raster);
  const yuv = await page.captureFrame({ format: 'yuv420p' });
  expect(yuv.bytes).toBe(800 * 600 * 3 / 2);
  const file = join(dir, 'frames.rgb');
  for (let n = 0; n < 2; n++) {
    const f = await page.captureFrame({ format: 'rgb24', path: file, clip: { x: 0, y: 0, width: 100, height: 50 } });
    expect([f.width, f.height, f.bytes, f.data.length]).toEqual([200, 100, 200 * 100 * 3, 0]);
  }
  expect(statSync(file).size).toBe(2 * 200 * 100 * 3);
  await expect(page.captureFrame({ path: 'relative.rgba' })).rejects.toThrow('not absolute');
});

test.skipIf(!bin || !ffmpeg)('captureFrame into ffmpeg through a FrameFifo: one stream, each frame as captured', async () => {
  await using page = await lp.page({ url: `http://127.0.0.1:${server.port}/` });
  await page.send('Emulation.setDeviceMetricsOverride', { width: 160, height: 120, deviceScaleFactor: 2, mobile: false });
  const fifo = new FrameFifo(join(dir, 'frames.fifo'));
  const out = join(dir, 'frames.yuv');
  // ffmpeg copies the stream to a file: what it read, frame by frame.
  const ff = spawn('ffmpeg', ['-v', 'error', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-s', '320x240', '-r', '30', '-i', fifo.path, '-f', 'rawvideo', '-pix_fmt', 'yuv420p', '-y', out]);
  const exited = new Promise<number>((resolve) => ff.on('close', (code) => resolve(code ?? 1)));
  await fifo.open();
  const colors = ['#ff0000', '#00ff00', '#0000ff'];
  for (const color of colors) {
    await page.run([evaluate(`document.body.innerHTML = ''; document.body.style.background = '${color}'`)]);
    // The browser closes the FIFO after each frame; the stream goes on.
    await page.captureFrame({ format: 'rgb24', path: fifo.path });
  }
  await fifo.close();
  expect(await exited).toBe(0);
  const frame = 320 * 240 * 3 / 2;
  const yuv = readFileSync(out);
  expect(yuv.length).toBe(3 * frame);
  // BT.601 luma of red, green and blue (82, 145, 41), give or take ffmpeg's rounding.
  const luma = [0, 1, 2].map((k) => yuv[k * frame + 1000]);
  [82, 145, 41].forEach((expected, k) => expect(Math.abs(luma[k] - expected), `${luma}`).toBeLessThanOrEqual(2));
});
