// Fidelity of the PDFs run.mjs wrote: every page rasterized with pdftoppm
// (100 dpi), compared per page with pixelmatch (threshold 0.1, as the ToS
// pdf-visual e2e did), text compared with pdftotext (word multiset overlap).
//
//   bun compare.mjs --out <run.mjs out dir> [--a chrome-fixed] [--b lp-fixed] [--worst 6]

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import pixelmatch from 'pixelmatch';
import { PNG } from 'pngjs';

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => (a.startsWith('--') ? [...acc, [a.slice(2), all[i + 1]]] : acc), []),
);
const out = path.resolve(args.out);
const A = args.a ?? 'chrome-fixed';
const B = args.b ?? 'lp-fixed';
const worstN = Number(args.worst ?? 6);
const rasterDir = path.join(out, 'raster');

const raster = (engine, name) => {
  const pdf = path.join(out, engine, `${name}.pdf`);
  const dir = path.join(rasterDir, engine, name);
  if (!existsSync(pdf)) return null;
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true });
    execFileSync('pdftoppm', ['-r', '100', '-png', pdf, path.join(dir, 'p')]);
  }
  return readdirSync(dir)
    .filter((f) => f.endsWith('.png'))
    .sort()
    .map((f) => path.join(dir, f));
};

const words = (engine, name) => {
  const pdf = path.join(out, engine, `${name}.pdf`);
  try {
    return execFileSync('pdftotext', [pdf, '-'], { encoding: 'utf8' }).split(/\s+/).filter(Boolean);
  } catch {
    return null;
  }
};

const overlap = (a, b) => {
  if (!a || !b || a.length + b.length === 0) return null;
  const count = new Map();
  for (const w of a) count.set(w, (count.get(w) ?? 0) + 1);
  let common = 0;
  for (const w of b) {
    const n = count.get(w) ?? 0;
    if (n > 0) {
      common++;
      count.set(w, n - 1);
    }
  }
  return (2 * common) / (a.length + b.length);
};

const crop = (png, w, h) => {
  if (png.width === w && png.height === h) return png.data;
  const data = Buffer.alloc(w * h * 4, 255);
  for (let y = 0; y < h; y++) png.data.copy(data, y * w * 4, y * png.width * 4, y * png.width * 4 + w * 4);
  return data;
};

const names = readdirSync(path.join(out, A))
  .filter((f) => f.endsWith('.pdf'))
  .map((f) => f.slice(0, -4))
  .sort();
const results = [];
const pagesAll = [];
for (const name of names) {
  const pa = raster(A, name);
  const pb = raster(B, name);
  const row = { case: name, pagesA: pa?.length ?? 0, pagesB: pb?.length ?? 0, pages: [] };
  if (pa && pb) {
    for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
      if (!pa[i] || !pb[i]) {
        row.pages.push({ page: i + 1, ratio: 1, missing: pa[i] ? B : A });
        continue;
      }
      const a = PNG.sync.read(readFileSync(pa[i]));
      const b = PNG.sync.read(readFileSync(pb[i]));
      const w = Math.min(a.width, b.width);
      const h = Math.min(a.height, b.height);
      const diff = new PNG({ width: w, height: h });
      const n = pixelmatch(crop(a, w, h), crop(b, w, h), diff.data, w, h, { threshold: 0.1 });
      const p = { page: i + 1, ratio: n / (w * h), sizeA: [a.width, a.height], sizeB: [b.width, b.height] };
      row.pages.push(p);
      pagesAll.push({ name, i, ratio: p.ratio, a: pa[i], b: pb[i] });
    }
  }
  const ratios = row.pages.map((p) => p.ratio);
  row.meanRatio = ratios.length ? ratios.reduce((s, r) => s + r, 0) / ratios.length : null;
  row.maxRatio = ratios.length ? Math.max(...ratios) : null;
  const wa = words(A, name);
  const wb = words(B, name);
  row.wordsA = wa?.length ?? null;
  row.wordsB = wb?.length ?? null;
  row.textOverlap = overlap(wa, wb);
  results.push(row);
  console.log(
    `${name}: pages ${row.pagesA}/${row.pagesB} mean ${(100 * (row.meanRatio ?? 0)).toFixed(2)}% max ${(100 * (row.maxRatio ?? 0)).toFixed(2)}% words ${row.wordsA}/${row.wordsB} overlap ${row.textOverlap?.toFixed(3)}`,
  );
}
writeFileSync(path.join(out, `compare-${A}-vs-${B}.json`), JSON.stringify(results, null, 1));

// Side by side of the worst pages: A | B | diff.
if (worstN > 0) {
  const sbs = path.resolve(args.sbs ?? path.join(out, 'side-by-side'));
  mkdirSync(sbs, { recursive: true });
  pagesAll.sort((x, y) => y.ratio - x.ratio);
  const seen = new Set();
  for (const p of pagesAll) {
    if (seen.size >= worstN) break;
    if (seen.has(p.name)) continue;
    seen.add(p.name);
    const a = PNG.sync.read(readFileSync(p.a));
    const b = PNG.sync.read(readFileSync(p.b));
    const w = Math.min(a.width, b.width);
    const h = Math.min(a.height, b.height);
    const diff = new PNG({ width: w, height: h });
    pixelmatch(crop(a, w, h), crop(b, w, h), diff.data, w, h, { threshold: 0.1 });
    const gap = 8;
    const img = new PNG({ width: a.width + b.width + w + 2 * gap, height: Math.max(a.height, b.height) });
    img.data.fill(200);
    PNG.bitblt(a, img, 0, 0, a.width, a.height, 0, 0);
    PNG.bitblt(b, img, 0, 0, b.width, b.height, a.width + gap, 0);
    PNG.bitblt(diff, img, 0, 0, w, h, a.width + b.width + 2 * gap, 0);
    const file = path.join(sbs, `${p.name}-p${p.i + 1}-${(100 * p.ratio).toFixed(1)}pct.png`);
    writeFileSync(file, PNG.sync.write(img));
    console.log('wrote', file);
  }
}
