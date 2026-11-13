// Videos: `recordVideo`, the runner's `video` option, and the CDP screencast
// they are recorded from. Plain Playwright code: in the parity run stock
// Playwright's own recorder drives pyro's screencast.
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { findFfmpeg, playwrightFfmpeg } from '../../src/wright/video';
import { expect, isStock, test } from './fixtures';

// Stock Playwright encodes with its own ffmpeg; pyro with the first it finds.
const ffmpeg = isStock ? playwrightFfmpeg() : findFfmpeg();
const NO_FFMPEG = 'no ffmpeg to encode videos with: brew install ffmpeg, apt-get install ffmpeg or bunx playwright install ffmpeg (or set PYRO_FFMPEG)';

const WEBM = [0x1a, 0x45, 0xdf, 0xa3];
const isWebm = (file: string) => [...readFileSync(file).subarray(0, 4)].every((b, i) => b === WEBM[i]);

/** Codec, size and duration (s) of a video: from ffprobe beside ffmpeg, else ffmpeg's report of its input. */
function probe(file: string): { codec: string; width: number; height: number; duration: number } {
  const ffprobe = join(dirname(ffmpeg!), process.platform === 'win32' ? 'ffprobe.exe' : 'ffprobe');
  if (existsSync(ffprobe)) {
    const args = ['-v', 'error', '-show_entries', 'stream=codec_name,width,height:format=duration', '-of', 'json', file];
    const out = JSON.parse(execFileSync(ffprobe, args, { encoding: 'utf8' }));
    const s = out.streams[0];
    return { codec: s.codec_name, width: s.width, height: s.height, duration: Number(out.format.duration) };
  }
  // Without an output ffmpeg describes the input and exits non-zero.
  const text = spawnSync(ffmpeg!, ['-hide_banner', '-i', file], { encoding: 'utf8' }).stderr;
  const stream = /Video: (\w+).*?, (\d+)x(\d+)/.exec(text);
  const d = /Duration: (\d+):(\d+):([\d.]+)/.exec(text);
  return {
    codec: stream?.[1] ?? '',
    width: Number(stream?.[2]),
    height: Number(stream?.[3]),
    duration: d ? Number(d[1]) * 3600 + Number(d[2]) * 60 + Number(d[3]) : 0,
  };
}

/** A counter that changes ten times over a second, on a background that changes with it. */
const COUNTER = `data:text/html,${encodeURIComponent(`<!doctype html><body style="margin:0;font:40px sans-serif">
<div id=n>0</div><script>let i = 0; const t = setInterval(() => {
  n.textContent = ++i; document.body.style.background = i % 2 ? '#fdd' : '#ddf'; if (i === 10) clearInterval(t);
}, 100)</script>`)}`;

test.describe('recordVideo', () => {
  test.skip(!ffmpeg, NO_FFMPEG);
  let dir = '';
  test.beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'pyro-video-test-'));
  });
  test.afterEach(() => rmSync(dir, { recursive: true, force: true }));

  test('a page is recorded into a WebM, written when it closes', async ({ browser }) => {
    const context = await browser.newContext({ recordVideo: { dir } });
    const page = await context.newPage();
    await page.goto(COUNTER);
    await expect(page.locator('#n')).toHaveText('10');
    const video = page.video()!;
    const path = await video.path();
    expect(dirname(path)).toBe(dir);
    expect(path).toMatch(/\.webm$/);
    await page.close();
    // As Playwright: written by the time close() returns.
    expect(isWebm(path)).toBe(true);
    const info = probe(path);
    expect(info).toMatchObject({ codec: 'vp8', width: 800, height: 450 });
    expect(info.duration).toBeGreaterThan(0.5);
    const copy = join(dir, 'nested', 'copy.webm');
    await video.saveAs(copy);
    expect(readFileSync(copy).equals(readFileSync(path))).toBe(true);
    await video.delete();
    expect(existsSync(path)).toBe(false);
    await expect(video.saveAs(join(dir, 'late.webm'))).rejects.toThrow();
    await context.close();
  });

  test('size: as given, else the viewport scaled down to fit 800x800 with even sides', async ({ browser }) => {
    const cases: [object, [number, number]][] = [
      [{ recordVideo: { dir, size: { width: 320, height: 240 } } }, [320, 240]],
      [{ recordVideo: { dir, size: { width: 401, height: 301 } } }, [401, 301]],
      [{ recordVideo: { dir }, viewport: { width: 1003, height: 1601 } }, [500, 800]],
      [{ recordVideo: { dir }, viewport: { width: 1000, height: 1600 } }, [500, 800]],
      [{ recordVideo: { dir }, viewport: { width: 640, height: 480 } }, [640, 480]],
    ];
    for (const [options, size] of cases) {
      const context = await browser.newContext(options);
      const page = await context.newPage();
      await page.setContent('<body style="background:#6d28d9"><h1>size</h1>');
      const path = await page.video()!.path();
      // context.close() writes its pages' videos too.
      await context.close();
      const info = probe(path);
      expect([info.width, info.height], JSON.stringify(options)).toEqual(size);
    }
  });

  test('a popup gets a video of its own', async ({ browser }) => {
    const context = await browser.newContext({ recordVideo: { dir } });
    const page = await context.newPage();
    await page.setContent('<h1>opener</h1>');
    const [popup] = await Promise.all([context.waitForEvent('page'), page.evaluate(() => void window.open('about:blank'))]);
    await popup.setContent('<h1>popup</h1>');
    const paths = [await page.video()!.path(), await popup.video()!.path()];
    expect(paths[0]).not.toBe(paths[1]);
    await context.close();
    for (const p of paths) expect(isWebm(p)).toBe(true);
  });

  test('without recordVideo there is no video', async ({ page }) => {
    await page.setContent('<h1>no video</h1>');
    expect(page.video()).toBeNull();
  });

  test('no ffmpeg: newContext says how to get one', async ({ browser }) => {
    test.skip(isStock, 'pyro/wright looks for ffmpeg');
    const saved = process.env.PYRO_FFMPEG;
    process.env.PYRO_FFMPEG = join(dir, 'missing-ffmpeg');
    try {
      const e = await browser.newContext({ recordVideo: { dir } }).catch((e: Error) => e);
      expect(e).toBeInstanceOf(Error);
      expect((e as Error).message).toContain('brew install ffmpeg');
      expect((e as Error).message).toContain('bunx playwright install ffmpeg');
    } finally {
      if (saved === undefined) delete process.env.PYRO_FFMPEG;
      else process.env.PYRO_FFMPEG = saved;
    }
  });

  test('the runner keeps videos as `video` says, attached to the test', async () => {
    test.skip(isStock, 'runs our CLI on a generated project');
    test.setTimeout(120_000);
    // Outside the specs' testDir, so no runner picks the generated spec up.
    const project = mkdtempSync(join(tmpdir(), 'pyro-video-runner-'));
    try {
      const src = join(import.meta.dirname, '../../src');
      writeFileSync(join(project, 'playwright.config.ts'), `export default { testDir: '.', testMatch: '*.pw.ts', workers: 1, use: { video: process.env.VIDEO } };\n`);
      writeFileSync(
        join(project, 'x.pw.ts'),
        `import { expect, test } from ${JSON.stringify(join(src, 'wright/index'))};
test('passes', async ({ page }) => { await page.setContent('<h1>pass</h1>'); });
test('fails', async ({ page }) => { await page.setContent('<h1>fail</h1>'); expect(1).toBe(2); });
`,
      );
      const run = (video: string) => {
        rmSync(join(project, 'test-results'), { recursive: true, force: true });
        const r = spawnSync('bun', [join(src, 'cli.ts'), 'run', '-c', join(project, 'playwright.config.ts')], {
          cwd: project,
          env: { ...process.env, PYRO_RESULTS: '', VIDEO: video },
          encoding: 'utf8',
        });
        const results = join(project, 'test-results');
        const videos = existsSync(results) ? readdirSync(results, { recursive: true }).map(String).filter((f) => f.endsWith('.webm')) : [];
        return { out: r.stdout + r.stderr, status: r.status, videos };
      };
      const retained = run('retain-on-failure');
      expect(retained.status, retained.out).not.toBe(0);
      expect(retained.videos, retained.out).toEqual([join('x.pw.ts-fails', 'video.webm')]);
      expect(isWebm(join(project, 'test-results', retained.videos[0]))).toBe(true);
      // The report names it under the failure.
      expect(retained.out).toContain('attachment #1: video (video/webm)');
      expect(retained.out).toContain(join('test-results', 'x.pw.ts-fails', 'video.webm'));
      const all = run('on');
      expect(all.videos.sort(), all.out).toEqual([join('x.pw.ts-fails', 'video.webm'), join('x.pw.ts-passes', 'video.webm')]);
      expect(run('off').videos).toEqual([]);
    } finally {
      rmSync(project, { recursive: true, force: true });
    }
  });
});

test('Page.startScreencast frames over CDP, as Puppeteer reads them', async ({ context, page }) => {
  await page.setContent('<body style="margin:0"><div id=box style="width:50px;height:50px;background:#00f"></div>');
  const session = await context.newCDPSession(page);
  const frames: any[] = [];
  session.on('Page.screencastFrame', (f) => frames.push(f));
  await session.send('Page.startScreencast', { format: 'png' });
  // A frame at once (Puppeteer's page.screencast() waits for it).
  await expect.poll(() => frames.length).toBe(1);
  const [first] = frames;
  expect(Object.keys(first).sort()).toEqual(['data', 'metadata', 'sessionId']);
  expect(typeof first.sessionId).toBe('number');
  const png = Buffer.from(first.data, 'base64');
  expect([...png.subarray(1, 4)]).toEqual([0x50, 0x4e, 0x47]);
  expect([png.readUInt32BE(16), png.readUInt32BE(20)]).toEqual([1280, 720]);
  expect(Object.keys(first.metadata).sort()).toEqual(['deviceHeight', 'deviceWidth', 'offsetTop', 'pageScaleFactor', 'scrollOffsetX', 'scrollOffsetY', 'timestamp']);
  expect(first.metadata).toMatchObject({ deviceWidth: 1280, deviceHeight: 720, offsetTop: 0, pageScaleFactor: 1, scrollOffsetX: 0, scrollOffsetY: 0 });
  expect(Math.abs(first.metadata.timestamp - Date.now() / 1000)).toBeLessThan(60);
  // Until it is acked, a change makes no frame; then it does.
  await page.evaluate(() => void (document.getElementById('box')!.style.width = '100px'));
  await page.waitForTimeout(300);
  expect(frames.length).toBe(1);
  await session.send('Page.screencastFrameAck', { sessionId: first.sessionId });
  await expect.poll(() => frames.length).toBe(2);
  await session.send('Page.screencastFrameAck', { sessionId: frames[1].sessionId });
  // Nothing changes: no frame. Stopped: none either.
  await page.waitForTimeout(300);
  expect(frames.length).toBe(2);
  await session.send('Page.stopScreencast');
  await page.evaluate(() => void (document.getElementById('box')!.style.width = '150px'));
  await page.waitForTimeout(300);
  expect(frames.length).toBe(2);
  await session.detach();
});
