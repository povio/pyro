// Minimal repros of the fidelity gaps (bench/fidelity/repros/*.html) in
// Chromium and pyro: each page captured in both at deviceScaleFactor 2
// (repros/shots/<name>.chromium.png, .pyro.png), SSIM and MAD of the pair, and
// what the page publishes as `window.__measure()` (layout numbers) printed
// side by side.
//
//   (cd bench && bun install)                    # playwright-core, the version ToS pins
//   bun bench/fidelity/repro.ts [--bin target/release/pyro] [--suffix -fixed] [name ...]
//
// A page sets its viewport with <meta name="fidelity-viewport" content="640x240">
// (default 640x360). Pages are served from bench/ (/fidelity/repros/<name>.html),
// so they can use bench's fixtures (the Lato web font of bench/screenshot).
// Chromium is the Playwright build of bench/package.json.
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { decodePng, encodePng, luminance, meanAbsoluteDifference, ssim } from './image';

const args = process.argv.slice(2);
const opt = (name: string, fallback: string) => (args.includes(name) ? args[args.indexOf(name) + 1] : fallback);
const repo = resolve(import.meta.dir, '../..');
const dir = join(import.meta.dir, 'repros');
const shots = join(dir, 'shots');
const bin = resolve(opt('--bin', join(repo, 'target/release/pyro')));
const names = args.filter((a, i) => !a.startsWith('--') && !args[i - 1]?.startsWith('--'));
// `--suffix -fixed` writes <name>.pyro-fixed.png (compared with the Chromium shot), leaving <name>.pyro.png alone.
const suffix = opt('--suffix', '');
const pages = readdirSync(dir)
  .filter((f) => f.endsWith('.html'))
  .map((f) => f.slice(0, -5))
  .filter((n) => !names.length || names.includes(n));
const DSF = 2;
process.env.TZ ??= 'Europe/Ljubljana';

const root = resolve(import.meta.dir, '..');
const server = Bun.serve({
  port: 0,
  hostname: '127.0.0.1',
  async fetch(req) {
    const file = Bun.file(join(root, decodeURIComponent(new URL(req.url).pathname)));
    return (await file.exists()) ? new Response(file) : new Response(null, { status: 404 });
  },
});
const base = `http://127.0.0.1:${server.port}`;
mkdirSync(shots, { recursive: true });

const { chromium } = await import(join(import.meta.dir, '../node_modules/playwright-core/index.mjs'));
const wright = await import(join(repo, 'packages/pyro/src/wright/index.ts'));
const chrome = await chromium.launch({ channel: 'chromium' });
const probe = await chrome.newPage();
const userAgent = (await probe.evaluate(() => navigator.userAgent)).replace('HeadlessChrome', 'Chrome');
await probe.close();
const pyro = await wright.chromium.launch({ bin });

async function shoot(kind: 'chromium' | 'pyro', name: string, viewport: { width: number; height: number }) {
  const context =
    kind === 'chromium'
      ? await chrome.newContext({ viewport, deviceScaleFactor: DSF, userAgent })
      : await pyro.newContext({ viewport, userAgent });
  try {
    const page = await context.newPage();
    if (kind === 'pyro') await (await context.newCDPSession(page)).send('Emulation.setDeviceMetricsOverride', { ...viewport, deviceScaleFactor: DSF, mobile: false });
    await page.goto(`${base}/fidelity/repros/${name}.html`, { waitUntil: 'load' });
    await page.evaluate(() => document.fonts.ready.then(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))));
    await page.waitForTimeout(200);
    const measure = await page.evaluate(() => ((window as any).__measure ? (window as any).__measure() : null));
    const png: Buffer = await page.screenshot({ type: 'png', ...(kind === 'chromium' ? { caret: 'initial' } : {}) });
    // Re-encoded (RGB, best compression): the shots are committed.
    writeFileSync(join(shots, `${name}.${kind}${kind === 'pyro' ? suffix : ''}.png`), encodePng(decodePng(png), 9));
    return measure;
  } finally {
    await context.close();
  }
}

for (const name of pages) {
  const html = readFileSync(join(dir, `${name}.html`), 'utf8');
  const [w, h] = (html.match(/name="fidelity-viewport" content="(\d+)x(\d+)"/)?.slice(1) ?? ['640', '360']).map(Number);
  const viewport = { width: w, height: h };
  const c = await shoot('chromium', name, viewport);
  const p = await shoot('pyro', name, viewport);
  const a = decodePng(readFileSync(join(shots, `${name}.chromium.png`)));
  const b = decodePng(readFileSync(join(shots, `${name}.pyro${suffix}.png`)));
  const s = a.width === b.width && a.height === b.height ? ssim(luminance(a), luminance(b), a.width, a.height).mean : NaN;
  const { mad } = a.width === b.width && a.height === b.height ? meanAbsoluteDifference(a, b) : { mad: NaN };
  console.log(`\n${name}: SSIM ${s.toFixed(4)}, MAD ${mad.toFixed(2)}`);
  if (Array.isArray(c) && Array.isArray(p))
    for (let i = 0; i < c.length; i++) {
      const row = Object.entries(c[i])
        .map(([k, v]) => {
          const w = p[i]?.[k];
          if (typeof v === 'number' && typeof w === 'number') return `${k} ${v} / ${w} (${((w / v - 1) * 100).toFixed(1)}%)`;
          return k === 'case' || v === w ? `${k} ${v}` : `${k} ${JSON.stringify(v)} / ${JSON.stringify(w)}`;
        })
        .join('  ');
      console.log(`  ${row}`);
    }
  else if (c || p) console.log('  chromium', JSON.stringify(c), '\n  pyro    ', JSON.stringify(p));
}

await chrome.close();
await pyro.close();
server.stop(true);
process.exit(0);
