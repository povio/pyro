// matchScreenshot end to end: a real lightningpanda binary built with the
// `render` feature (LIGHTNINGPANDA_BIN, else target/{release,debug} of this
// checkout), a page served by Bun, baselines in a temporary directory.
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Lightningpanda, ScreenshotMismatchError, css, evaluate, goto, label, matchScreenshot, screenshotMatchers } from '../src/index';

const root = join(import.meta.dir, '../../..');
const bin =
  process.env.LIGHTNINGPANDA_BIN ??
  [join(root, 'target/release/lightningpanda'), join(root, 'target/debug/lightningpanda')].find(existsSync);

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
let lp: Lightningpanda;
let dir: string;

beforeAll(async () => {
  server = Bun.serve({
    port: 0,
    fetch: (req) =>
      new URL(req.url).pathname === '/red.png'
        ? new Response(RED, { headers: { 'content-type': 'image/png' } })
        : new Response(PAGE, { headers: { 'content-type': 'text/html' } }),
  });
  dir = mkdtempSync(join(tmpdir(), 'lp-shots-'));
  if (bin) lp = await Lightningpanda.launch({ bin });
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
  process.env.LP_UPDATE_SNAPSHOTS = '1';
  try {
    const r = await page.run([css('#card').matchScreenshot('element', { dir }), matchScreenshot('form', { dir })]);
    expect(r.screenshots.element.recorded).toBe(true);
    expect([r.screenshots.element.width, r.screenshots.element.height]).toEqual([300, 120]);
  } finally {
    delete process.env.LP_UPDATE_SNAPSHOTS;
  }
  const typed = page.run([label('Name').fill('Ada Lovelace'), matchScreenshot('form', { dir })]);
  const error = await typed.catch((e) => e);
  expect(error).toBeInstanceOf(ScreenshotMismatchError);
  expect(error.result.diffPixels).toBeGreaterThan(0);
});
