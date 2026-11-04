// Repeat screenshots of one page: what a screenshot costs after nothing
// changed, after one text change and after one class toggle (the renderer
// keeps the page's document between screenshots and applies only changes),
// and whether DOM work and locators cost the same once a page has taken a
// screenshot.
//
//   bun bench/screenshot/repeat.ts [--bin target/release/lightningpanda] [--iters 30] [--pages <dir>]
//
// Pages: the fixtures of match-screenshot.ts, plus every snapshot in --pages
// (one directory each: page.html, meta.json with base_url and resources
// [{url, file}], res/<file>), served statically with their sheets and resources.
// 1280x720. Medians, ms.
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { Lightningpanda, css, evaluate } from '../../packages/lightning-panda/src/index';

const args = process.argv.slice(2);
const opt = (k: string, d: string) => (args.includes(k) ? args[args.indexOf(k) + 1] : d);
const root = join(import.meta.dir, '../..');
const bin = opt('--bin', [join(root, 'target/release/lightningpanda'), join(root, 'target/debug/lightningpanda')].find(existsSync) ?? '');
const iters = Number(opt('--iters', '30'));
const pagesDir = opt('--pages', '');
const fixtures = join(import.meta.dir, 'fixtures');
const font = readFileSync(join(fixtures, 'fonts/Lato-Regular.ttf'));
const fontFace = '<style>@font-face { font-family: Lato; src: url(/fonts/Lato-Regular.ttf) format("truetype"); }</style>';

// name -> { html, files: path -> bytes }
const sites = new Map<string, { html: string; files: Map<string, Uint8Array> }>();
for (const name of ['login', 'dashboard', 'article']) {
  const html = readFileSync(join(fixtures, `${name}.html`), 'utf8').replace('<head>', `<head>${fontFace}`);
  sites.set(name, { html, files: new Map([['/fonts/Lato-Regular.ttf', font]]) });
}
if (pagesDir) {
  for (const name of readdirSync(pagesDir)) {
    const dir = join(pagesDir, name);
    if (!existsSync(join(dir, 'meta.json'))) continue;
    const meta = JSON.parse(readFileSync(join(dir, 'meta.json'), 'utf8'));
    let html = readFileSync(join(dir, 'page.html'), 'utf8');
    const files = new Map<string, Uint8Array>();
    for (const r of meta.resources) {
      const path = `/${name}/res/${r.file}`;
      files.set(path, readFileSync(join(dir, 'res', r.file)));
      html = html.split(`"${r.url}"`).join(`"${path}"`);
      const rel = new URL(r.url).pathname;
      html = html.split(`"${rel}"`).join(`"${path}"`);
    }
    sites.set(name, { html, files });
  }
}

const server = Bun.serve({
  port: 0,
  fetch(req) {
    const path = new URL(req.url).pathname;
    for (const site of sites.values()) {
      const body = site.files.get(path);
      if (body) {
        const type = path.endsWith('.css') || path.includes('sheet') ? 'text/css' : path.endsWith('.ttf') ? 'font/ttf' : 'application/octet-stream';
        return new Response(body, { headers: { 'content-type': type, 'cache-control': 'max-age=3600' } });
      }
    }
    const site = sites.get(path.slice(1));
    return site ? new Response(site.html, { headers: { 'content-type': 'text/html' } }) : new Response('', { status: 404 });
  },
});

const median = (xs: number[]) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)];
const fmt = (x: number | undefined) => (x === undefined ? '-' : x.toFixed(2));

// One text node in a cell (or paragraph), one row (or list item) to toggle a class on.
const SETUP = `(() => {
  const cell = [...document.querySelectorAll('tbody td, td, li, p, span')].find(e => e.firstChild && e.firstChild.nodeType === 3 && e.firstChild.data.trim());
  const row = document.querySelector('tbody tr') || document.querySelector('tr, li, p');
  window.__bench = { cell: cell.firstChild, text: cell.firstChild.data, row, n: 0 };
  return !!(cell && row);
})()`;
const CHANGES: Record<string, string> = {
  'no change': '1',
  'one text change': `__bench.cell.data = __bench.text + ' ' + (++__bench.n); 1`,
  'one class toggle': `__bench.row.classList.toggle('lp-bench-toggle'); 1`,
};
// DOM work and locator queries, timed inside the page and through LP.run.
const DOM_WORK = `(() => {
  const t = performance.now();
  const box = document.body.appendChild(document.createElement('div'));
  for (let i = 0; i < 2000; i++) { const d = document.createElement('span'); d.className = 'x' + (i % 7); d.textContent = 'n' + i; box.appendChild(d); }
  for (let i = 0; i < 500; i++) box.children[i].setAttribute('data-i', String(i));
  let found = 0; for (let i = 0; i < 50; i++) found += document.querySelectorAll('.x3, td, li').length;
  box.remove();
  return performance.now() - t;
})()`;

const lp = await Lightningpanda.launch({ bin });
const shotRows: string[] = [];
const domRows: string[] = [];
for (const name of sites.keys()) {
  await using page = await lp.page({ url: `http://127.0.0.1:${server.port}/${name}` });
  await page.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 720, deviceScaleFactor: 1, mobile: false });
  if (process.env.BENCH_VERBOSE) console.error(name, 'setup');
  await page.run([evaluate(SETUP, 'ok')]);

  // DOM work and locators before the page took any screenshot.
  const domBefore: number[] = [];
  const locBefore: number[] = [];
  const domRound = async (dom: number[], loc: number[]) => {
    for (let i = 0; i < iters; i++) {
      const r = await page.run([evaluate(DOM_WORK, 'ms')]);
      dom.push(r.values.ms as number);
      const t = performance.now();
      await page.run([css('body *').read('count', 'count'), css('body *').first().read('visible', 'visible'), css('body *').last().read('box', 'box')]);
      loc.push(performance.now() - t);
    }
  };
  await domRound(domBefore, locBefore);
  if (process.env.BENCH_VERBOSE) console.error(name, 'dom before done');

  for (const [label, change] of Object.entries(CHANGES)) {
    for (let i = 0; i < 3; i++) {
      await page.run([evaluate(change, 'x')]);
      await page.send('Page.captureScreenshot', { format: 'png' });
    }
    const client: number[] = [];
    const browser: Record<string, number[]> = {};
    for (let i = 0; i < iters; i++) {
      await page.run([evaluate(change, 'x')]);
      const t = performance.now();
      const r = await page.send('Page.captureScreenshot', { format: 'png' });
      client.push(performance.now() - t);
      for (const [k, v] of Object.entries(r.lp.ms as Record<string, unknown>)) {
        if (typeof v === 'number') (browser[k] ??= []).push(v);
      }
    }
    if (process.env.BENCH_VERBOSE) console.error(name, label, 'done');
    const b = (k: string) => fmt(browser[k] ? median(browser[k]) : undefined);
    shotRows.push(`| ${name} | ${label} | ${fmt(median(client))} | ${b('snapshot')} | ${b('layout')} | ${b('scene')} | ${b('raster')} | ${b('encode')} | ${b('total')} |`);
  }

  // The same DOM work with the page's document kept by the renderer.
  const domAfter: number[] = [];
  const locAfter: number[] = [];
  await domRound(domAfter, locAfter);
  domRows.push(`| ${name} | ${fmt(median(domBefore))} | ${fmt(median(domAfter))} | ${fmt(median(locBefore))} | ${fmt(median(locAfter))} |`);
}
lp.close();
server.stop(true);

console.log(`Binary: ${bin}; ${iters} iterations after 3 warm-up; 1280x720; medians, ms.\n`);
console.log('Page.captureScreenshot (PNG) after a change, client round trip and browser-side stages:\n');
console.log('| page | change | round trip | snapshot | layout | scene+hash | raster | encode | total |');
console.log('|---|---|---:|---:|---:|---:|---:|---:|---:|');
for (const r of shotRows) console.log(r);
console.log('\nDOM work (2,000 inserts, 500 setAttribute, 50 querySelectorAll, in the page) and an LP.run of 3 locator reads, before the first screenshot of the page and after the screenshots above:\n');
console.log('| page | DOM work before | DOM work after | locators before | locators after |');
console.log('|---|---:|---:|---:|---:|');
for (const r of domRows) console.log(r);
