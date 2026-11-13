// Screen capture for a video pipeline: a 2056x1200 viewport at device scale
// factor 3 saved as lossless 6168x3600 PNGs, CSS-scale captures, clips and
// element crops at device resolution, previews within the model's image
// limits, full pages past the model's and the renderer's limits, and element
// boxes to compute crops from.
import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createBrowserTools, type BrowserTools, type ToolResult } from '../../src/agent/index';
import { parseArgs } from '../../src/mcp/cli';
import { pngInfo, pngPixel } from './png';
import { bin, refOf, textOf } from './site';

const RED = [255, 0, 0, 255];
const BLUE = [0, 0, 255, 255];
const WHITE = [255, 255, 255, 255];

/** A red 100x100 box at (100, 100), a blue 200x40 heading at (300, 1500), and a document `h` CSS px tall. */
function startCaptureSite() {
  return Bun.serve({
    port: 0,
    fetch(req) {
      const h = Number(new URL(req.url).searchParams.get('h') ?? 2000);
      return new Response(
        `<!doctype html><html><head><title>Capture</title><style>body{margin:0;background:#fff}</style></head><body>
        <button id="red" style="position:absolute;left:100px;top:100px;width:100px;height:100px;margin:0;padding:0;border:0;background:rgb(255,0,0);color:rgb(255,0,0)">Red box</button>
        <h1 id="far" style="position:absolute;left:300px;top:1500px;width:200px;height:40px;margin:0;font-size:10px;background:rgb(0,0,255);color:rgb(0,0,255)">Far heading</h1>
        <div style="width:10px;height:${h}px"></div>
        </body></html>`,
        { headers: { 'content-type': 'text/html' } },
      );
    },
  });
}

let server: ReturnType<typeof startCaptureSite>;
let url: string;
let dir: string;
let out: string;

beforeAll(() => {
  server = startCaptureSite();
  url = `http://127.0.0.1:${server.port}`;
  dir = mkdtempSync(join(tmpdir(), 'pyro-capture-'));
  out = join(dir, 'out');
});
afterAll(() => {
  server?.stop(true);
  rmSync(dir, { recursive: true, force: true });
});

const image = (r: ToolResult) => {
  const c = r.content.find((x) => x.type === 'image');
  if (c?.type !== 'image') throw new Error('no image in the result');
  return Buffer.from(c.data, 'base64');
};
/** The JSON in a result's ```json fence. */
const fencedJson = (t: string) => JSON.parse(/```json\n([^]*?)\n```/.exec(t)![1]);
/** Within Claude's high-resolution image limits: 2576 px a side, 4784 visual tokens of 28x28 px. */
const fitsModel = (png: Buffer) => {
  const { width, height } = pngInfo(png);
  return Math.max(width, height) <= 2576 && Math.ceil(width / 28) * Math.ceil(height / 28) <= 4784;
};

describe.skipIf(!bin)('capture at device pixels', () => {
  let b: BrowserTools;
  beforeAll(() => {
    b = createBrowserTools({ bin, outputDir: out, fileRoots: [dir], profileDir: join(dir, 'profiles') });
  });
  afterAll(() => b?.close());

  test('browser_resize sets 2056x1200 at deviceScaleFactor 3, and results state the viewport', async () => {
    await b.execute('browser_navigate', { url: `${url}/` });
    const r = await b.execute('browser_resize', { width: 2056, height: 1200, deviceScaleFactor: 3 });
    expect(r.isError).toBeUndefined();
    expect(textOf(r)).toContain('Resized the viewport to 2056x1200 CSS px at deviceScaleFactor 3: a device-pixel screenshot of it is 6168x3600.');
    expect(textOf(r)).toContain('- Viewport: 2056x1200 CSS px at deviceScaleFactor 3 (6168x3600 device px)');
    expect(textOf(await b.execute('browser_snapshot', {}))).toContain('- Viewport: 2056x1200 CSS px at deviceScaleFactor 3 (6168x3600 device px)');
    const page = await b.execute('browser_evaluate', { function: '() => [innerWidth, innerHeight, devicePixelRatio]' });
    expect(textOf(page).replace(/\s/g, '')).toContain('[2056,1200,3]');
    expect(textOf(await b.execute('browser_resize', { width: 2056, height: 1200, deviceScaleFactor: 20 }))).toContain('deviceScaleFactor is 0.5 to 8');
  }, 30_000);

  test('a 6168x3600 PNG saved byte for byte as the browser encoded it, with its path, size and sha256', async () => {
    const r = await b.execute('browser_take_screenshot', { filename: 'screen.png', includeImage: false });
    expect(r.isError).toBeUndefined();
    expect(r.content.some((c) => c.type === 'image')).toBe(false);
    const path = join(out, 'screen.png');
    const png = readFileSync(path);
    expect(pngInfo(png)).toMatchObject({ width: 6168, height: 3600, bitDepth: 8 });
    // The red box spans CSS 100..200: device 300..600.
    expect(pngPixel(png, 450, 450)).toEqual(RED);
    expect(pngPixel(png, 301, 301)).toEqual(RED);
    expect(pngPixel(png, 290, 290)).toEqual(WHITE);
    expect(pngPixel(png, 610, 450)).toEqual(WHITE);
    const sha256 = createHash('sha256').update(png).digest('hex');
    expect(r.structuredContent).toMatchObject({ path, width: 6168, height: 3600, bytes: png.length, sha256, scale: 'device', deviceScaleFactor: 3, format: 'png' });
    expect(textOf(r)).toContain(`saved to ${path}: 6168x3600 PNG at device pixels (deviceScaleFactor 3), ${png.length} bytes, sha256 ${sha256}`);
    expect(textOf(r)).toContain('No image returned (includeImage: false).');
    // Lossless: the very bytes of the browser's own Page.captureScreenshot.
    const tab = b.session.current!;
    const cdp = await tab.entry.context.newCDPSession(tab.page);
    const direct = Buffer.from((await cdp.send<{ data: string }>('Page.captureScreenshot', { format: 'png' })).data, 'base64');
    expect(direct.equals(png)).toBe(true);
    const nothing = await b.execute('browser_take_screenshot', { includeImage: false });
    expect(textOf(nothing)).toContain('give a filename');
  }, 60_000);

  test("scale 'css' is 2056x1200; clips and elements are cropped at device resolution", async () => {
    await b.execute('browser_take_screenshot', { filename: 'css.png', scale: 'css', includeImage: false });
    const css = readFileSync(join(out, 'css.png'));
    expect(pngInfo(css)).toMatchObject({ width: 2056, height: 1200 });
    expect(pngPixel(css, 150, 150)).toEqual(RED);
    expect(pngPixel(css, 95, 150)).toEqual(WHITE);

    const clipped = await b.execute('browser_take_screenshot', { filename: 'clip.png', clip: { x: 50, y: 60, width: 200, height: 100 }, includeImage: false });
    expect(clipped.structuredContent).toMatchObject({ area: { x: 50, y: 60, width: 200, height: 100, relativeTo: 'viewport' }, width: 600, height: 300 });
    const clip = readFileSync(join(out, 'clip.png'));
    expect(pngInfo(clip)).toMatchObject({ width: 600, height: 300 });
    expect(pngPixel(clip, 300, 200)).toEqual(RED); // CSS (150, 126.7)
    expect(pngPixel(clip, 100, 200)).toEqual(WHITE); // CSS (83.3, 126.7)

    const snap = textOf(await b.execute('browser_snapshot', {}));
    const el = await b.execute('browser_take_screenshot', { element: 'Red box', ref: refOf(snap, 'button', 'Red box'), filename: 'red.png', includeImage: false });
    expect(el.structuredContent).toMatchObject({ area: { x: 100, y: 100, width: 100, height: 100, relativeTo: 'document' }, width: 300, height: 300 });
    const red = readFileSync(join(out, 'red.png'));
    expect(pngInfo(red)).toMatchObject({ width: 300, height: 300 });
    for (const [x, y] of [[1, 1], [150, 150], [298, 298]]) expect(pngPixel(red, x, y)).toEqual(RED);

    // Below the viewport: scrolled into view first, then cropped the same way.
    const far = await b.execute('browser_take_screenshot', { element: 'Far heading', ref: refOf(snap, 'heading', 'Far heading'), filename: 'far.png', includeImage: false });
    expect(textOf(far)).toContain('Scrolled it into view first');
    const farPng = readFileSync(join(out, 'far.png'));
    expect(pngInfo(farPng)).toMatchObject({ width: 600, height: 120 });
    expect(pngPixel(farPng, 300, 60)).toEqual(BLUE);

    const trimmed = await b.execute('browser_take_screenshot', { clip: { x: 2000, y: 0, width: 200, height: 100 }, filename: 'edge.png', includeImage: false });
    expect(textOf(trimmed)).toContain('The clip reached past the viewport: trimmed to x 2000, y 0, 56x100.');
    expect(pngInfo(readFileSync(join(out, 'edge.png')))).toMatchObject({ width: 168, height: 300 });
    expect(textOf(await b.execute('browser_take_screenshot', { clip: { x: 3000, y: 0, width: 10, height: 10 } }))).toContain('is outside the viewport');
    expect(textOf(await b.execute('browser_take_screenshot', { clip: { x: 0, y: 0, width: 10, height: 10 }, element: 'Red box', ref: 'e1' }))).toContain('not both');
  }, 60_000);

  test('with includeImage the model gets a preview within its image limits; a capture that fits is the image', async () => {
    await b.execute('browser_evaluate', { function: '() => window.scrollTo(0, 0)' });
    const r = await b.execute('browser_take_screenshot', { filename: 'preview.png' });
    expect(pngInfo(readFileSync(join(out, 'preview.png')))).toMatchObject({ width: 6168, height: 3600 });
    const preview = image(r);
    const { width, height } = pngInfo(preview);
    expect(fitsModel(preview)).toBe(true);
    expect(width).toBeGreaterThan(2000);
    expect(width / height).toBeCloseTo(2056 / 1200, 1);
    expect(preview.toString('base64').length).toBeLessThanOrEqual(5 * 1024 * 1024);
    // The preview shows the same page: the red box where the scale puts it.
    expect(pngPixel(preview, Math.round((150 * width) / 2056), Math.round((150 * height) / 1200))).toEqual(RED);
    expect(textOf(r)).toContain(`The image below is a ${width}x${height} PNG preview of it`);
    expect(textOf(r)).toContain("sized to the model's image limits (at most 2576 px a side and 4784 image tokens); the file is full size.");
    expect(r.structuredContent?.image).toEqual({ width, height, format: 'png', preview: true });
    // A resource link to the full-size file comes with it.
    expect(r.content.find((c) => c.type === 'resource_link')).toMatchObject({ uri: `file://${join(out, 'preview.png')}`, mimeType: 'image/png' });

    const look = await b.execute('browser_take_screenshot', {});
    expect(fitsModel(image(look))).toBe(true);
    expect(textOf(look)).toMatch(/Screenshot of the viewport \(png, \d+x\d+\): a preview at 41% of the 6168x3600 device-pixel capture/);

    const small = await b.execute('browser_take_screenshot', { scale: 'css', filename: 'small.png' });
    expect(textOf(small)).toContain('The image below is the saved file itself.');
    expect(image(small).equals(readFileSync(join(out, 'small.png')))).toBe(true);
  }, 60_000);

  test('bounding boxes match getBoundingClientRect, x3 in device pixels, after scrolling too, and crop as clips', async () => {
    await b.execute('browser_evaluate', { function: '() => window.scrollTo(0, 500)' });
    const snap = textOf(await b.execute('browser_snapshot', {}));
    const red = refOf(snap, 'button', 'Red box');
    const far = refOf(snap, 'heading', 'Far heading');
    const r = await b.execute('browser_get_bounding_box', { elements: [{ element: 'Red box', ref: red }, { element: 'Far heading', ref: `[ref=${far}]` }] });
    expect(r.isError).toBeUndefined();
    const data = r.structuredContent as any;
    expect(data).toMatchObject({ viewport: { width: 2056, height: 1200 }, scroll: { x: 0, y: 500 }, document: { width: 2056, height: 2000 }, deviceScaleFactor: 3 });
    const rects = fencedJson(textOf(await b.execute('browser_evaluate', { function: '() => ["#red", "#far"].map((s) => { const r = document.querySelector(s).getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width, height: r.height }; })' })));
    for (const [k, e] of (data.elements as any[]).entries()) {
      const c = rects[k];
      expect(e.css.viewport).toEqual(c);
      expect(e.css.document).toEqual({ ...c, y: c.y + 500 });
      expect(e.device.viewport).toEqual({ x: c.x * 3, y: c.y * 3, width: c.width * 3, height: c.height * 3 });
      expect(e.device.document).toEqual({ x: c.x * 3, y: (c.y + 500) * 3, width: c.width * 3, height: c.height * 3 });
    }
    expect(data.elements[0]).toMatchObject({ element: 'Red box', ref: red, css: { viewport: { x: 100, y: -400, width: 100, height: 100 } }, inViewport: 'none' });
    expect(data.elements[1]).toMatchObject({ element: 'Far heading', ref: far, css: { viewport: { x: 300, y: 1000 }, document: { x: 300, y: 1500, width: 200, height: 40 } }, inViewport: 'full' });
    // The model reads the same numbers.
    expect(fencedJson(textOf(r))).toEqual(data);
    // The boxes crop: css.viewport as a clip, css.document as a clip with fullPage.
    await b.execute('browser_take_screenshot', { clip: data.elements[1].css.viewport, filename: 'far-clip.png', includeImage: false });
    const farClip = readFileSync(join(out, 'far-clip.png'));
    expect(pngInfo(farClip)).toMatchObject({ width: 600, height: 120 });
    expect(pngPixel(farClip, 300, 60)).toEqual(BLUE);
    await b.execute('browser_take_screenshot', { clip: data.elements[0].css.document, fullPage: true, filename: 'red-doc.png', includeImage: false });
    const redDoc = readFileSync(join(out, 'red-doc.png'));
    expect(pngInfo(redDoc)).toMatchObject({ width: 300, height: 300 });
    expect(pngPixel(redDoc, 150, 150)).toEqual(RED);

    const stale = await b.execute('browser_get_bounding_box', { elements: [{ element: 'gone', ref: 'e999' }] });
    expect(stale.isError).toBe(true);
    expect(textOf(stale)).toContain('Ref e999 is not on the page now');
    expect(textOf(await b.execute('browser_get_bounding_box', { elements: [] }))).toContain('give at least one element');
  }, 60_000);

  test('full pages: at device pixels up to the renderer limit, refused past it with how to capture parts', async () => {
    await b.execute('browser_navigate', { url: `${url}/` });
    await b.execute('browser_take_screenshot', { fullPage: true, filename: 'full.png', includeImage: false });
    const full = readFileSync(join(out, 'full.png'));
    expect(pngInfo(full)).toMatchObject({ width: 6168, height: 6000 });
    expect(pngPixel(full, 1200, 4560)).toEqual(BLUE); // the heading at CSS (400, 1520)
    // The browser's own full-page capture has the same size (the engine counts the document in CSS px).
    const tab = b.session.current!;
    const cdp = await tab.entry.context.newCDPSession(tab.page);
    const direct = Buffer.from((await cdp.send<{ data: string }>('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true })).data, 'base64');
    expect(pngInfo(direct)).toMatchObject({ width: 6168, height: 6000 });

    await b.execute('browser_navigate', { url: `${url}/?h=7000` });
    const refused = await b.execute('browser_take_screenshot', { fullPage: true, filename: 'tall.png' });
    expect(refused.isError).toBe(true);
    expect(textOf(refused)).toContain('The full page is 2056x7000 CSS px: 6168x21000 px at device pixels (deviceScaleFactor 3), more than the 16384 px a side Pyro renders.');
    expect(textOf(refused)).toContain('fullPage: true with clip { x: 0, y: 0, width: 2056, height: 5461 }, then y: 5461');
    expect(textOf(refused)).toContain('Or use scale: "css" (2056x7000 px).');
    expect(existsSync(join(out, 'tall.png'))).toBe(false);
    await b.execute('browser_take_screenshot', { fullPage: true, clip: { x: 0, y: 5461, width: 2056, height: 5461 }, filename: 'tall-2.png', includeImage: false });
    expect(pngInfo(readFileSync(join(out, 'tall-2.png')))).toMatchObject({ width: 6168, height: 4617 });
    const css = await b.execute('browser_take_screenshot', { fullPage: true, scale: 'css', filename: 'tall-css.png' });
    expect(pngInfo(readFileSync(join(out, 'tall-css.png')))).toMatchObject({ width: 2056, height: 7000 });
    expect(fitsModel(image(css))).toBe(true);

    // Past 8000 px, the largest image the model takes: saved whole, previewed within the limits.
    await b.execute('browser_resize', { width: 1280, height: 720, deviceScaleFactor: 1 });
    await b.execute('browser_navigate', { url: `${url}/?h=9000` });
    const long = await b.execute('browser_take_screenshot', { fullPage: true, filename: 'long.png' });
    expect(pngInfo(readFileSync(join(out, 'long.png')))).toMatchObject({ width: 1280, height: 9000 });
    expect(fitsModel(image(long))).toBe(true);
    expect(textOf(long)).toContain('much taller than wide');
  }, 120_000);
});

describe.skipIf(!bin)('device scale factor from options and identities', () => {
  test('deviceScaleFactor (pyro mcp --device-scale-factor) applies to new tabs', async () => {
    const t = createBrowserTools({ bin, viewport: { width: 2056, height: 1200 }, deviceScaleFactor: 3, outputDir: join(dir, 'out-dsf') });
    try {
      const nav = await t.execute('browser_navigate', { url: `${url}/` });
      expect(textOf(nav)).toContain('- Viewport: 2056x1200 CSS px at deviceScaleFactor 3 (6168x3600 device px)');
      await t.execute('browser_take_screenshot', { filename: 'a.png', includeImage: false });
      expect(pngInfo(readFileSync(join(dir, 'out-dsf', 'a.png')))).toMatchObject({ width: 6168, height: 3600 });
    } finally {
      await t.close();
    }
  }, 30_000);

  test('an identity file sets the viewport and device scale factor of a loaded profile', async () => {
    const profiles = join(dir, 'ids');
    mkdirSync(profiles, { recursive: true });
    writeFileSync(join(profiles, 'capture.identity.json'), JSON.stringify({ viewport: { width: 2056, height: 1200 }, deviceScaleFactor: 3 }));
    const t = createBrowserTools({ bin, profileDir: profiles, outputDir: join(dir, 'out-id') });
    try {
      const loaded = await t.execute('browser_profiles', { action: 'load', name: 'capture' });
      expect(textOf(loaded)).toContain('identity settings: viewport, deviceScaleFactor');
      expect(textOf(loaded)).toContain('- Viewport: 2056x1200 CSS px at deviceScaleFactor 3');
      await t.execute('browser_navigate', { url: `${url}/` });
      await t.execute('browser_take_screenshot', { filename: 'b.png', includeImage: false });
      expect(pngInfo(readFileSync(join(dir, 'out-id', 'b.png')))).toMatchObject({ width: 6168, height: 3600 });
      writeFileSync(join(profiles, 'bad.identity.json'), JSON.stringify({ deviceScaleFactor: 'three' }));
      expect(textOf(await t.execute('browser_profiles', { action: 'load', name: 'bad' }))).toContain('deviceScaleFactor must be a number');
    } finally {
      await t.close();
    }
  }, 30_000);
});

test('pyro mcp --device-scale-factor', () => {
  expect(parseArgs(['--viewport', '2056x1200', '--device-scale-factor', '3']).options).toMatchObject({ viewport: { width: 2056, height: 1200 }, deviceScaleFactor: 3 });
  expect(() => parseArgs(['--device-scale-factor', '0'])).toThrow('--device-scale-factor takes a number from 0.5 to 8');
  expect(() => parseArgs(['--device-scale-factor', 'x'])).toThrow('--device-scale-factor takes a number from 0.5 to 8');
});
