// Mobile emulation (`isMobile`, `devices['iPhone 13']`) as Chromium does it:
// `<meta name="viewport">` decides the layout viewport and the page scale,
// and a page without one lays out 980 px wide, zoomed out to the device.
// The expected values are what Playwright 1.59.1's Chromium (Chrome 147)
// reports for the same pages and options. A local site on a free port.
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { devices } from '../../src/wright/index';
import { decodePng } from '../../src/wright/snapshot';
import { expect, test } from './fixtures';

const doc = (meta: string | null, body = '<p style="margin:0">Hello</p>') =>
  `<!doctype html><html><head>${meta === null ? '' : `<meta name="viewport" content="${meta}">`}</head><body style="margin:0">${body}</body></html>`;
const PAGES: Record<string, string> = {
  '/device-width': doc('width=device-width, initial-scale=1'),
  '/none': doc(null),
  '/w600': doc('width=600'),
  '/scale2': doc('initial-scale=2'),
  '/wide': doc('width=device-width, initial-scale=1', '<div style="width:1200px;height:10px"></div>'),
  // A 980 px layout: a red-to-blue band across it, a lime block on its left half.
  '/layout': doc(null, '<div style="height:200px;background:linear-gradient(90deg,red,blue)"></div><div style="width:50%;height:300px;background:lime"></div>'),
};

let server: Server;
let SITE = '';

test.beforeAll(async () => {
  server = createServer((req, res) => {
    const path = new URL(req.url!, 'http://site').pathname;
    res.writeHead(PAGES[path] ? 200 : 404, { 'content-type': 'text/html' }).end(PAGES[path] ?? 'not found');
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  SITE = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

test.afterAll(() => {
  server.closeAllConnections();
  server.close();
});

/** innerWidth/Height, the root's clientWidth/Height, visualViewport width/height/scale, `(max-width: 600px)`. */
const metrics = () => [
  innerWidth,
  innerHeight,
  document.documentElement.clientWidth,
  document.documentElement.clientHeight,
  visualViewport!.width,
  visualViewport!.height,
  visualViewport!.scale,
  matchMedia('(max-width: 600px)').matches,
];

test('devices["iPhone 13"]: the viewport meta decides the layout viewport and the scale', async ({ browser }) => {
  const context = await browser.newContext({ ...devices['iPhone 13'] });
  const page = await context.newPage();
  const cases: [string, unknown[]][] = [
    ['/device-width', [390, 664, 390, 664, 390, 664, 1, true]],
    // No tag: a 980 px layout viewport, at 390/980 (Chromium: 980, 1669, 980, 1668, 980, 1668.5128173828125, 0.3979591727256775).
    ['/none', [980, 1669, 980, 1668, 980, 1668.5128173828125, 0.3979591727256775, false]],
    ['/w600', [600, 1022, 600, 1021, 600, 1021.5384521484375, 0.6499999761581421, true]],
    ['/scale2', [195, 332, 195, 332, 195, 332, 2, true]],
    // Content wider than the layout viewport widens the window, not the layout viewport.
    ['/wide', [1200, 2044, 390, 664, 390, 664, 1, true]],
  ];
  for (const [path, expected] of cases) {
    await page.goto(SITE + path);
    expect(await page.evaluate(metrics), path).toEqual(expected);
  }
  // The window's outer size and the screen stay the device's (Chromium: 390, 664, 390, 844).
  expect(await page.evaluate(() => [outerWidth, outerHeight, screen.width, screen.height])).toEqual([390, 664, 390, 844]);
  await context.close();
});

test('without isMobile the viewport meta is ignored', async ({ browser }) => {
  const context = await browser.newContext({ viewport: { width: 390, height: 664 } });
  const page = await context.newPage();
  for (const path of ['/w600', '/none']) {
    await page.goto(SITE + path);
    expect(await page.evaluate(metrics), path).toEqual([390, 664, 390, 664, 390, 664, 1, true]);
  }
  await context.close();
});

test('a viewport meta added by script lays the page out again and fires resize', async ({ browser }) => {
  const context = await browser.newContext({ ...devices['iPhone 13'] });
  const page = await context.newPage();
  await page.goto(`${SITE}/none`);
  await page.evaluate(() => {
    const w = window as unknown as { ev: string[] };
    w.ev = [];
    addEventListener('resize', () => w.ev.push(`w:${innerWidth}x${innerHeight}`));
    visualViewport!.addEventListener('resize', () => w.ev.push(`vv:${visualViewport!.width}@${visualViewport!.scale.toFixed(3)}`));
    matchMedia('(max-width: 600px)').addEventListener('change', (e) => w.ev.push(`mq:${e.matches}`));
  });
  // Chromium: the new values at once, then resize, the visual viewport's resize and the media query's change.
  const added = await page.evaluate(() => {
    const m = document.createElement('meta');
    m.name = 'viewport';
    m.content = 'width=device-width, initial-scale=1';
    document.head.append(m);
    return [innerWidth, innerHeight, document.documentElement.clientWidth, visualViewport!.scale, matchMedia('(max-width: 600px)').matches];
  });
  expect(added).toEqual([390, 664, 390, 1, true]);
  await expect.poll(() => page.evaluate(() => (window as unknown as { ev: string[] }).ev)).toEqual(['w:390x664', 'vv:390@1.000', 'mq:true']);
  await context.close();
});

test('a page without the meta screenshots its whole 980 px layout in the device width', async ({ browser }) => {
  const context = await browser.newContext({ ...devices['iPhone 13'] });
  const page = await context.newPage();
  await page.goto(`${SITE}/layout`);
  const png = decodePng(await page.screenshot())!;
  // Chromium: 1170 x 1991 (the visual viewport, 980 x 1668 CSS px at 390/980, in device pixels).
  expect([png.width, png.height]).toEqual([1170, 1991]);
  const at = (x: number, y: number) => Array.from(png.data.subarray((y * png.width + x) * 4, (y * png.width + x) * 4 + 3));
  const is = (x: number, y: number, rgb: (c: number[]) => boolean) => expect(rgb(at(x, y)), `${x},${y}: ${at(x, y)}`).toBe(true);
  // The band's red start and blue end both show: the layout's full width is in view.
  is(10, 100, ([r, , b]) => r > 200 && b < 60);
  is(1160, 100, ([r, , b]) => b > 200 && r < 60);
  // The lime block, half the layout's width, ends at the image's middle (490 CSS px: 585 device px).
  is(290, 400, ([r, g, b]) => g > 200 && r < 60 && b < 60);
  is(570, 400, ([r, g, b]) => g > 200 && r < 60 && b < 60);
  is(600, 400, ([r, g, b]) => r > 240 && g > 240 && b > 240);
  is(880, 400, ([r, g, b]) => r > 240 && g > 240 && b > 240);
  await context.close();
});

test('isMobile and hasTouch are independent', async ({ browser }) => {
  // [maxTouchPoints, 'ontouchstart' in window, (pointer: coarse), (hover: none), (any-hover: hover), innerWidth]
  const probe = () => [navigator.maxTouchPoints, 'ontouchstart' in window, matchMedia('(pointer: coarse)').matches, matchMedia('(hover: none)').matches, matchMedia('(any-hover: hover)').matches, innerWidth];
  const viewport = { width: 390, height: 664 };
  for (const [options, expected] of [
    [{ isMobile: true }, [0, false, false, false, true, 980]],
    [{ hasTouch: true }, [1, true, true, true, false, 390]],
    [{ isMobile: true, hasTouch: true }, [1, true, true, true, false, 980]],
  ] as const) {
    const context = await browser.newContext({ viewport, ...options });
    const page = await context.newPage();
    await page.goto(`${SITE}/none`);
    expect(await page.evaluate(probe), JSON.stringify(options)).toEqual(expected);
    await context.close();
  }
});
