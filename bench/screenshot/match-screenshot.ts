// End-to-end screenshot benchmark through the real path: a lightningpanda
// binary built with `--features render`, the fixtures served over HTTP (Lato
// as a web font, fetched by the renderer through the page's network), and the
// lightning-panda client over CDP.
//
//   bun bench/screenshot/match-screenshot.ts [--bin target/release/lightningpanda] [--iters 50]
//
// Per fixture and viewport, after warm-up, median and p95 (ms) of:
// - capture:   Page.captureScreenshot (PNG) round trip
// - compare:   LP.run [matchScreenshot] with the baseline bytes under a new name
//              each time (decode + raster + compare, no hash to hit)
// - hash hit:  LP.run [matchScreenshot] against a cached baseline whose
//              display-list hash is known (no raster, no compare)
// - floor:     LP.run [eval "1"] (the CDP + LP.run round trip alone)
// plus the browser-side stage times of the compare path.
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Lightningpanda, evaluate } from '../../packages/lightning-panda/src/index';

const args = process.argv.slice(2);
const opt = (k: string, d: string) => (args.includes(k) ? args[args.indexOf(k) + 1] : d);
const root = join(import.meta.dir, '../..');
const bin = opt('--bin', [join(root, 'target/release/lightningpanda'), join(root, 'target/debug/lightningpanda')].find(existsSync) ?? '');
const iters = Number(opt('--iters', '50'));
const fixtures = join(import.meta.dir, 'fixtures');
const font = readFileSync(join(fixtures, 'fonts/Lato-Regular.ttf'));
const fontFace = '<style>@font-face { font-family: Lato; src: url(/fonts/Lato-Regular.ttf) format("truetype"); }</style>';

const server = Bun.serve({
  port: 0,
  fetch(req) {
    const path = new URL(req.url).pathname;
    if (path === '/fonts/Lato-Regular.ttf') return new Response(font, { headers: { 'content-type': 'font/ttf', 'cache-control': 'max-age=3600' } });
    const html = readFileSync(join(fixtures, `${path.slice(1)}.html`), 'utf8').replace('<head>', `<head>${fontFace}`);
    return new Response(html, { headers: { 'content-type': 'text/html' } });
  },
});

const stats = (xs: number[]) => {
  const s = [...xs].sort((a, b) => a - b);
  return { median: s[Math.floor(s.length / 2)], p95: s[Math.min(s.length - 1, Math.ceil(s.length * 0.95) - 1)] };
};
const cell = (xs: number[]) => {
  const { median, p95 } = stats(xs);
  return `${median.toFixed(2)} / ${p95.toFixed(2)}`;
};
async function time(n: number, f: () => Promise<unknown>): Promise<number[]> {
  for (let i = 0; i < 3; i++) await f();
  const out: number[] = [];
  for (let i = 0; i < n; i++) {
    const t = performance.now();
    await f();
    out.push(performance.now() - t);
  }
  return out;
}

const lp = await Lightningpanda.launch({ bin });
const rows: string[] = [];
const stages: string[] = [];
const cases: [string, number][] = [['login', 1], ['dashboard', 1], ['article', 1], ['dashboard', 2]];
for (const [name, dpr] of cases) {
  const label = dpr === 1 ? name : `${name} @${dpr}x`;
  await using page = await lp.page({ url: `http://127.0.0.1:${server.port}/${name}` });
  await page.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 720, deviceScaleFactor: dpr, mobile: false });
  await page.run([evaluate('1')]);
  const cold = performance.now();
  const shot = await page.send('Page.captureScreenshot', { format: 'png' });
  const coldMs = performance.now() - cold;
  const baseline = shot.data as string;

  const capture = await time(iters, () => page.send('Page.captureScreenshot', { format: 'png' }));
  let k = 0;
  const browser: Record<string, number[]> = {};
  const compare = await time(iters, async () => {
    const r = await page.send('LP.run', { steps: [{ matchScreenshot: { name: `${label}-${k++}`, baseline } }] });
    const res = r.results[0];
    if (!res.pass || res.hashHit) throw new Error(`${label}: unexpected ${JSON.stringify({ ...res, actual: undefined })}`);
    for (const [s, v] of Object.entries(res.ms as Record<string, number>)) (browser[s] ??= []).push(v);
  });
  await page.send('LP.run', { steps: [{ matchScreenshot: { name: `${label}-hit`, baseline } }] });
  const hit = await time(iters, async () => {
    const r = await page.send('LP.run', { steps: [{ matchScreenshot: { name: `${label}-hit` } }] });
    if (!r.results[0].hashHit) throw new Error(`${label}: no hash hit`);
  });
  const floor = await time(iters, () => page.send('LP.run', { steps: [{ eval: { expression: '1' } }] }));
  rows.push(`| ${label} | ${coldMs.toFixed(1)} | ${cell(capture)} | ${cell(compare)} | ${cell(hit)} | ${cell(floor)} |`);
  const b = (s: string) => (browser[s] ? stats(browser[s]).median.toFixed(2) : '-');
  stages.push(`| ${label} | ${b('decode')} | ${b('snapshot')} | ${b('fetch')} | ${b('layout')} | ${b('scene')} | ${b('raster')} | ${b('compare')} | ${b('total')} |`);
}
lp.close();
server.stop(true);

console.log(`Binary: ${bin}; ${iters} iterations after 3 warm-up; 1280x720; cells median / p95 ms (client-side, CDP round trip included).\n`);
console.log('| fixture | first capture | Page.captureScreenshot | LP.run matchScreenshot (compare) | LP.run matchScreenshot (hash hit) | LP.run eval floor |');
console.log('|---|---:|---:|---:|---:|---:|');
for (const r of rows) console.log(r);
console.log('\nBrowser-side stages of the compare path (median ms; `total` = from the step starting to its result on the browser thread):\n');
console.log('| fixture | baseline decode | snapshot | fetch | layout | scene+hash | raster | compare | total |');
console.log('|---|---:|---:|---:|---:|---:|---:|---:|---:|');
for (const r of stages) console.log(r);
