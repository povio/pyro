// Chromium against pyro on paint: the same pages rendered in Playwright's Chromium and in pyro, each pair scored
// with SSIM (luma) and mean absolute difference as bench/fidelity does, and the paint matrix's probe pixels compared.
// A report, not a gate. Three page sets:
//   wpt     a fixed sample of the WPT paint reftests' test pages (paint-wpt-sample.txt, the revision in
//           bench/wpt/wpt-revision.txt), 800x600 at deviceScaleFactor 1, settled as WPT settles a reftest
//   repros  bench/fidelity/repros, at their fidelity-viewport and deviceScaleFactor 2, as repro.ts captures them
//   matrix  paint-matrix.ts: each paint feature in each context, 360x260 at 1, with probe pixels
//
//   (cd bench && bun install) && bun bench/fidelity/paint.ts --bin target/release/pyro [--out DIR] [--wpt DIR]
//       [--sets wpt,repros,matrix] [--match RE] [--images N]
//
// --wpt: a WPT checkout at the pinned revision with the sample's pages; without it the sample is fetched (sparse,
// ~70 MB) into ~/Library/Caches/pyro/wpt-paint-<revision>. Output (default ~/Library/Caches/pyro/paint-report/<time>):
// report.md (the worst pages by feature, the matrix), report.json (every page) and, for the N worst pages,
// side-by-side images (Chromium | pyro | difference). Chromium is the Playwright build of bench/package.json
// (`bun bench/node_modules/playwright-core/cli.js install chromium`).
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { extname, join, resolve } from 'node:path';
import { decodePng, diffHeatmap, encodePng, luminance, meanAbsoluteDifference, sideBySide, ssim, type Image } from './image';
import { CONTEXTS, FEATURES, VIEWPORT as MATRIX_VIEWPORT, pages as matrixPages } from './paint-matrix';

const args = process.argv.slice(2);
const opt = (name: string, fallback?: string) => (args.includes(name) ? args[args.indexOf(name) + 1] : fallback);
const repo = resolve(import.meta.dir, '../..');
const bench = resolve(import.meta.dir, '..');
const bin = resolve(opt('--bin', join(repo, 'target/release/pyro'))!);
const sets = opt('--sets', 'wpt,repros,matrix')!.split(',');
const match = opt('--match') ? new RegExp(opt('--match')!) : null;
const imagesFor = Number(opt('--images', '40'));
const stamp = new Date().toISOString().replace(/[:.]/g, '-');
const out = resolve(opt('--out', join(homedir(), 'Library/Caches/pyro/paint-report', stamp))!);
const revision = readFileSync(join(bench, 'wpt/wpt-revision.txt'), 'utf8').trim();
process.env.TZ ??= 'Europe/Ljubljana';
// A probe pixel agrees when no channel differs by more than this (antialiasing, colour rounding).
const PROBE_TOLERANCE = 24;

type Page = { set: 'wpt' | 'repros' | 'matrix'; name: string; group: string; url: string; viewport: { width: number; height: number }; dsf: number; feature?: string; context?: string; probes?: string[] };
type Result = Page & { ssim?: number; mad?: number; changed?: number; probes?: string[]; probeResults?: { name: string; x: number; y: number; chromium: number[]; pyro: number[]; ok: boolean }[]; error?: string; image?: string };

// ---------- the WPT sample ----------
const SAMPLE = readFileSync(join(import.meta.dir, 'paint-wpt-sample.txt'), 'utf8').split('\n').map((s) => s.trim()).filter((s) => s && !s.startsWith('#'));
const SUPPORT = ['resources/', 'common/', 'fonts/', 'images/', 'css/support/', 'css/reference/', 'css/CSS2/support/', 'css/css-images/support/', 'media/1x1-green.png'];
function ensureWpt(): string {
  if (opt('--wpt')) return resolve(opt('--wpt')!);
  const dir = join(homedir(), 'Library/Caches/pyro', `wpt-paint-${revision.slice(0, 12)}`);
  if (SAMPLE.every((u) => existsSync(join(dir, u)))) return dir;
  const dirs = [...new Set(SAMPLE.map((u) => u.split('/').slice(0, u.startsWith('css/CSS2/') ? 3 : 2).join('/') + '/'))];
  const git = (...a: string[]) => {
    const r = spawnSync('git', ['-C', dir, ...a], { stdio: ['pipe', 'inherit', 'inherit'], input: a[0] === 'sparse-checkout' ? [...dirs, ...SUPPORT].map((p) => `/${p}`).join('\n') : undefined });
    if (r.status !== 0) throw new Error(`git ${a.join(' ')} failed`);
  };
  mkdirSync(dir, { recursive: true });
  if (!existsSync(join(dir, '.git'))) {
    git('init', '-q');
    git('remote', 'add', 'origin', 'https://github.com/web-platform-tests/wpt.git');
  }
  git('sparse-checkout', 'set', '--no-cone', '--stdin');
  git('fetch', '-q', '--depth', '1', '--filter=blob:none', 'origin', revision);
  git('-c', 'advice.detachedHead=false', 'checkout', '-q', 'FETCH_HEAD');
  return dir;
}

// What a WPT page is about, from its path (the same grouping as bench/wpt/chrome-compare.mjs, shorter).
const WPT_FEATURES: [RegExp, string][] = [
  [/box-shadow/, 'box-shadow'], [/text-shadow/, 'text-shadow'], [/drop-shadow/, 'filter: drop-shadow()'], [/backdrop-filter/, 'backdrop-filter'],
  [/filter-effects\//, 'filters'], [/mix-blend|background-blend|isolation|compositing\//, 'blending, compositing'], [/clip-path/, 'clip-path'],
  [/mask/, 'masks'], [/css-masking\/clip|visufx\/clip/, 'clip'], [/border-image/, 'border-image'], [/border-radius|rounded/, 'border-radius'],
  [/outline/, 'outlines'], [/border/, 'borders'], [/gradient/, 'gradients'], [/background/, 'backgrounds'], [/opacity/, 'opacity'],
  [/z-?index|stack|zorder/, 'stacking order'], [/overflow|visibility/, 'overflow, visibility'], [/text-decoration|underline|line-through/, 'text-decoration'],
  [/text-emphasis/, 'text-emphasis'], [/css-text-decor\//, 'text decoration (other)'], [/preserve-?3d|perspective|backface|3d/i, 'transforms: 3D'],
  [/css-transforms\//, 'transforms: 2D'], [/css-color\/|CSS2\/colors/, 'colors'],
];
const wptGroup = (url: string) => WPT_FEATURES.find(([re]) => re.test(url))?.[1] ?? url.split('/').slice(1, 2).join('/');

function pageList(): Page[] {
  const list: Page[] = [];
  if (sets.includes('wpt'))
    for (const url of SAMPLE) list.push({ set: 'wpt', name: url, group: wptGroup(url), url, viewport: { width: 800, height: 600 }, dsf: 1 });
  if (sets.includes('repros')) {
    const dir = join(import.meta.dir, 'repros');
    for (const f of spawnSync('ls', [dir], { encoding: 'utf8' }).stdout.split('\n').filter((x) => x.endsWith('.html'))) {
      const html = readFileSync(join(dir, f), 'utf8');
      const [w, h] = (html.match(/name="fidelity-viewport" content="(\d+)x(\d+)"/)?.slice(1) ?? ['640', '360']).map(Number);
      list.push({ set: 'repros', name: f.slice(0, -5), group: 'repros', url: `fidelity/repros/${f}`, viewport: { width: w, height: h }, dsf: 2 });
    }
  }
  if (sets.includes('matrix'))
    for (const p of matrixPages()) list.push({ set: 'matrix', name: p.name, group: p.group, url: `${p.name}.html`, viewport: MATRIX_VIEWPORT, dsf: 1, feature: p.feature, context: p.context, probes: p.probes });
  return match ? list.filter((p) => match.test(`${p.set}/${p.name}`)) : list;
}

// ---------- servers ----------
const TYPES: Record<string, string> = { '.html': 'text/html', '.htm': 'text/html', '.xht': 'application/xhtml+xml', '.xhtml': 'application/xhtml+xml', '.xml': 'application/xml', '.svg': 'image/svg+xml', '.css': 'text/css', '.js': 'text/javascript', '.png': 'image/png', '.jpg': 'image/jpeg', '.gif': 'image/gif', '.webp': 'image/webp', '.ttf': 'font/ttf', '.otf': 'font/otf', '.woff': 'font/woff', '.woff2': 'font/woff2' };
function serve(root: string | null, generated?: Map<string, string>) {
  return Bun.serve({
    port: 0,
    hostname: '127.0.0.1',
    async fetch(req) {
      const path = decodeURIComponent(new URL(req.url).pathname).replace(/^\/+/, '');
      const page = generated?.get(path);
      if (page != null) return new Response(page, { headers: { 'content-type': 'text/html; charset=utf-8' } });
      if (!root) return new Response(null, { status: 404 });
      const file = Bun.file(join(root, path));
      return (await file.exists()) ? new Response(file, { headers: { 'content-type': TYPES[extname(path)] ?? file.type } }) : new Response(null, { status: 404 });
    },
  });
}

// ---------- capture ----------
// Runs in the page: load, document.fonts.ready (after a layout, which is when pyro requests web fonts), two frames,
// and for a WPT page with class="reftest-wait", TestRendered and the class's removal; at most `ms`.
function settle(ms: number) {
  return new Promise<boolean>((resolve) => {
    const root = document.documentElement;
    const timer = setTimeout(() => resolve(false), ms);
    let observer: MutationObserver | null = null;
    const ready = () => {
      if (root?.classList.contains('reftest-wait')) {
        if (!observer) {
          observer = new MutationObserver(paints);
          observer.observe(root, { attributes: true });
          root.dispatchEvent(new Event('TestRendered', { bubbles: true }));
        }
        return;
      }
      observer?.disconnect();
      clearTimeout(timer);
      resolve(true);
    };
    function paints() { requestAnimationFrame(() => requestAnimationFrame(ready)); }
    const load = () => {
      void root?.getBoundingClientRect();
      void document.body?.offsetWidth;
      (document.fonts ? document.fonts.ready : Promise.resolve()).then(paints, paints);
    };
    if (document.readyState !== 'complete') addEventListener('load', load, { once: true });
    else load();
  });
}

type Engine = { newContext: (o: object) => Promise<any> };
async function shoot(engine: Engine, kind: 'chromium' | 'pyro', base: string, p: Page, userAgent: string) {
  const context = await engine.newContext({ viewport: p.viewport, deviceScaleFactor: p.dsf, userAgent });
  try {
    const page = await context.newPage();
    await page.goto(`${base}/${p.url}`, { waitUntil: 'load', timeout: 20000 });
    const settled = await page.evaluate(settle, 10000);
    await page.waitForTimeout(100);
    // Probe positions from Chromium's layout (the reference): 1x1 elements inside the subject.
    const probes = kind === 'chromium' && p.probes?.length
      ? await page.evaluate(() => [...document.querySelectorAll('.probe')].map((e) => { const r = (e as HTMLElement).getBoundingClientRect(); return { name: (e as HTMLElement).dataset.name!, x: Math.floor(r.left + r.width / 2), y: Math.floor(r.top + r.height / 2) }; }))
      : null;
    const png: Buffer = await page.screenshot({ type: 'png', ...(kind === 'chromium' ? { caret: 'initial' } : {}) });
    return { image: decodePng(png), probes, settled };
  } finally {
    await context.close();
  }
}

const pixel = (img: Image, x: number, y: number) => (x < 0 || y < 0 || x >= img.width || y >= img.height ? [] : [...img.data.subarray((y * img.width + x) * 4, (y * img.width + x) * 4 + 3)]);

async function main() {
  const list = pageList();
  mkdirSync(join(out, 'images'), { recursive: true });
  const wpt = list.some((p) => p.set === 'wpt') ? ensureWpt() : null;
  const generated = new Map(sets.includes('matrix') ? matrixPages().map((p) => [`${p.name}.html`, p.html]) : []);
  const wptServer = serve(wpt, generated);
  const benchServer = serve(bench);
  const baseFor = (p: Page) => (p.set === 'repros' ? `http://127.0.0.1:${benchServer.port}` : `http://127.0.0.1:${wptServer.port}`);

  const { chromium } = await import(join(bench, 'node_modules/playwright-core/index.mjs'));
  const wright = await import(join(repo, 'packages/pyro/src/wright/index.ts'));
  const chrome = await chromium.launch({ channel: 'chromium' });
  const probe = await chrome.newPage();
  const userAgent = (await probe.evaluate(() => navigator.userAgent)).replace('HeadlessChrome', 'Chrome');
  await probe.close();
  const pyro = await wright.chromium.launch({ bin });
  const pyroVersion = spawnSync(bin, ['version'], { encoding: 'utf8' }).stdout.split('\n')[0];
  console.error(`chromium ${chrome.version()}, ${pyroVersion}: ${list.length} pages`);

  const results: Result[] = [];
  const shots = new Map<Result, { a: Image; b: Image }>();
  const t0 = Date.now();
  for (const [i, p] of list.entries()) {
    const r: Result = { ...p };
    try {
      const [c, y] = await Promise.all([shoot(chrome, 'chromium', baseFor(p), p, userAgent), shoot(pyro, 'pyro', baseFor(p), p, userAgent)]);
      const { image: a } = c;
      const { image: b } = y;
      if (a.width !== b.width || a.height !== b.height) throw new Error(`sizes differ: ${a.width}x${a.height} / ${b.width}x${b.height}`);
      r.ssim = ssim(luminance(a), luminance(b), a.width, a.height).mean;
      const m = meanAbsoluteDifference(a, b);
      r.mad = m.mad;
      r.changed = m.changedShare;
      if (c.probes)
        r.probeResults = c.probes.map((q: { name: string; x: number; y: number }) => {
          const ca = pixel(a, q.x * p.dsf, q.y * p.dsf), pa = pixel(b, q.x * p.dsf, q.y * p.dsf);
          return { ...q, chromium: ca, pyro: pa, ok: ca.length > 0 && ca.every((v, k) => Math.abs(v - pa[k]) <= PROBE_TOLERANCE) };
        });
      if (!c.settled || !y.settled) r.error = `did not settle in 10 s in ${[!c.settled && 'Chromium', !y.settled && 'pyro'].filter(Boolean).join(' and ')}`;
      shots.set(r, { a, b });
    } catch (e) {
      r.error = String((e as Error).message ?? e).split('\n')[0].slice(0, 200);
    }
    results.push(r);
    if ((i + 1) % 50 === 0) console.error(`${i + 1}/${list.length} (${((Date.now() - t0) / 1000).toFixed(0)} s)`);
  }
  await chrome.close();
  await pyro.close();
  wptServer.stop(true);
  benchServer.stop(true);

  // Images for the worst pages: Chromium | pyro | difference.
  const scored = results.filter((r) => r.ssim != null).sort((x, y) => x.ssim! - y.ssim!);
  for (const r of scored.slice(0, imagesFor)) {
    const { a, b } = shots.get(r)!;
    r.image = `images/${r.set}--${r.name.replace(/[^\w.-]+/g, '_')}.png`;
    writeFileSync(join(out, r.image), encodePng(sideBySide([a, b, diffHeatmap(a, b)]), 6));
  }
  const meta = { date: new Date().toISOString(), chromium: chrome.version(), pyro: pyroVersion, bin, wptRevision: revision, seconds: Math.round((Date.now() - t0) / 1000) };
  writeFileSync(join(out, 'report.json'), JSON.stringify({ ...meta, results }, null, 1));
  const md = report(meta, results);
  writeFileSync(join(out, 'report.md'), md);
  console.log(md);
  console.error(`report: ${join(out, 'report.md')}`);
}

// ---------- report ----------
const f4 = (x?: number) => (x == null ? '–' : x.toFixed(4));
const probeMiss = (r: Result) => (r.probeResults ?? []).filter((q) => !q.ok);
function report(meta: Record<string, unknown>, results: Result[]) {
  let md = `# Paint: Chromium vs pyro\n\n${meta.date}: Chromium ${meta.chromium}, ${meta.pyro} (${meta.bin}), WPT ${String(meta.wptRevision).slice(0, 10)}; ${results.length} pages in ${meta.seconds} s. SSIM on luma (1 = identical), MAD the mean absolute RGB difference (0-255); a probe disagrees when a channel differs by more than ${PROBE_TOLERANCE}.\n\n`;
  md += `| set | pages | errors | mean SSIM | pages under 0.99 | worst |\n|---|---:|---:|---:|---:|---|\n`;
  for (const set of ['wpt', 'repros', 'matrix'] as const) {
    const rs = results.filter((r) => r.set === set);
    if (!rs.length) continue;
    const ok = rs.filter((r) => r.ssim != null).sort((a, b) => a.ssim! - b.ssim!);
    const mean = ok.reduce((s, r) => s + r.ssim!, 0) / (ok.length || 1);
    md += `| ${set} | ${rs.length} | ${rs.filter((r) => r.ssim == null).length} | ${f4(mean)} | ${ok.filter((r) => r.ssim! < 0.99).length} | ${ok[0] ? `${ok[0].name} (${f4(ok[0].ssim)})` : '–'} |\n`;
  }
  // The matrix: features down, contexts across; SSIM and disagreeing probes.
  const matrix = results.filter((r) => r.set === 'matrix');
  if (matrix.length) {
    md += `\n## Paint matrix\n\nSSIM per feature and context; ✗N: N probe pixels disagree; **bold**: under 0.99 or a probe disagrees.\n\n| feature | ${CONTEXTS.map((c) => c.name).join(' | ')} |\n|---|${CONTEXTS.map(() => '---:').join('|')}|\n`;
    for (const f of FEATURES) {
      const cells = CONTEXTS.map((c) => {
        const r = matrix.find((x) => x.feature === f.name && x.context === c.name);
        if (!r) return '';
        if (r.ssim == null) return 'error';
        const miss = probeMiss(r).length;
        const cell = `${r.ssim.toFixed(3)}${miss ? ` ✗${miss}` : ''}`;
        return r.ssim < 0.99 || miss ? `**${cell}**` : cell;
      });
      md += `| ${f.name} | ${cells.join(' | ')} |\n`;
    }
    const misses = matrix.filter((r) => probeMiss(r).length);
    if (misses.length) {
      md += `\n<details><summary>Probe pixels that disagree (${misses.length} pages)</summary>\n\n| page | probe | Chromium | pyro |\n|---|---|---|---|\n`;
      for (const r of misses) for (const q of probeMiss(r)) md += `| ${r.feature} / ${r.context} | ${q.name} (${q.x}, ${q.y}) | ${q.chromium.join(',')} | ${q.pyro.join(',')} |\n`;
      md += `\n</details>\n`;
    }
  }
  // The worst pages, by feature (group): every set, lowest SSIM first.
  const groups = new Map<string, Result[]>();
  for (const r of results) groups.set(r.group, [...(groups.get(r.group) ?? []), r]);
  const worstOf = (rs: Result[]) => Math.min(...rs.map((r) => r.ssim ?? -1));
  md += `\n## Worst pages by feature\n\nPer feature (WPT pages by path, matrix pages by feature group, the repros together), the five lowest SSIM; errors first. Images: \`images/\` in this report's directory.\n\n| feature | pages | mean SSIM | worst pages (SSIM, MAD) |\n|---|---:|---:|---|\n`;
  for (const [g, rs] of [...groups].sort((a, b) => worstOf(a[1]) - worstOf(b[1]))) {
    const ok = rs.filter((r) => r.ssim != null);
    const mean = ok.reduce((s, r) => s + r.ssim!, 0) / (ok.length || 1);
    const worst = [...rs].sort((a, b) => (a.ssim ?? -1) - (b.ssim ?? -1)).slice(0, 5)
      .map((r) => `${r.set === 'matrix' ? `${r.feature} / ${r.context}` : r.name}${r.ssim == null ? ` (error: ${r.error})` : ` (${r.ssim.toFixed(3)}, ${r.mad!.toFixed(1)}${probeMiss(r).length ? `, ✗${probeMiss(r).length}` : ''})`}`);
    md += `| ${g} | ${rs.length} | ${f4(mean)} | ${worst.join('<br>')} |\n`;
  }
  const errors = results.filter((r) => r.error);
  if (errors.length) {
    md += `\n<details><summary>Errors and pages that did not settle (${errors.length})</summary>\n\n`;
    for (const r of errors) md += `- ${r.set}/${r.name}: ${r.error}\n`;
    md += `\n</details>\n`;
  }
  return md;
}

await main();
process.exit(0);
