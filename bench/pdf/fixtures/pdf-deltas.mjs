// bun pdf-deltas.mjs <chrome pdf dir> <lp pdf dir> <out.json> [case,case]: position of Pyro's content vs Chrome's in raw points from each page's top-left,
// by matching row and column ink profiles of 30 pt tiles at 600 dpi (sub-pixel by a parabola), and pixel diff at 150 dpi.
import { execFileSync } from 'node:child_process'; import fs from 'node:fs'; import path from 'node:path'; import { createRequire } from 'node:module';
const require = createRequire(import.meta.url); const { PNG } = require('pngjs'); const pm = require('pixelmatch'); const pixelmatch = pm.default ?? pm;
const [ca, lp, outf, only] = process.argv.slice(2); const DPI = 600, K = 72 / DPI, T = 250, R = 20;
const pgm = (f, page, dpi) => { const o = execFileSync('pdftoppm', ['-r', dpi, '-f', page, '-l', page, '-gray', f], { maxBuffer: 1 << 30, stdio: ['ignore', 'pipe', 'ignore'] }); let i = 0, tok = []; while (tok.length < 4) { while (o[i] <= 32) i++; let s = i; while (o[i] > 32) i++; tok.push(o.slice(s, i).toString()); } i++; return { w: +tok[1], h: +tok[2], d: o.subarray(i) }; };
const prof = (im, x0, y0, n, axis) => { const a = new Float64Array(n + 2 * R); for (let k = -R; k < n + R; k++) { let s = 0; for (let j = 0; j < T; j++) { const x = axis === 'x' ? x0 + k : x0 + j, y = axis === 'x' ? y0 + j : y0 + k; if (x >= 0 && y >= 0 && x < im.w && y < im.h) s += 255 - im.d[y * im.w + x]; } a[k + R] = s; } return a; };
const best = (A, B) => { let m = 0, v = 0; for (let k = R; k < R + T; k++) m += A[k]; m /= T; for (let k = R; k < R + T; k++) v += (A[k] - m) ** 2; if (v / T < 2500 * 25) return null;
  const sad = []; for (let s = -R; s <= R; s++) { let e = 0; for (let k = R; k < R + T; k++) e += Math.abs(A[k] - B[k + s]); sad.push(e); }
  let bi = 0; sad.forEach((e, i) => { if (e < sad[bi]) bi = i; }); let sub = bi - R; if (bi > 0 && bi < sad.length - 1) { const d = sad[bi - 1] - 2 * sad[bi] + sad[bi + 1]; if (d > 0) sub += (sad[bi - 1] - sad[bi + 1]) / (2 * d); }
  const total = A.subarray(R, R + T).reduce((a, b) => a + b, 0); return { s: sub * K, conf: sad[bi] / (total + 1) }; };
const toPng = (im) => { const p = new PNG({ width: im.w, height: im.h }); for (let i = 0; i < im.w * im.h; i++) { p.data[i * 4] = p.data[i * 4 + 1] = p.data[i * 4 + 2] = im.d[i]; p.data[i * 4 + 3] = 255; } return p; };
const res = {};
for (const f of fs.readdirSync(ca).filter((n) => n.endsWith('.pdf')).sort()) {
  const c = f.slice(0, -4); if (only && !only.split(',').includes(c)) continue;
  const A = path.join(ca, f), B = path.join(lp, f); const info = (p) => execFileSync('pdfinfo', [p], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] });
  const np = +/Pages:\s+(\d+)/.exec(info(A))[1], nb = +/Pages:\s+(\d+)/.exec(info(B))[1];
  const r = { pages: [np, nb], size: [/Page size:\s+([\d.]+ x [\d.]+)/.exec(info(A))[1], /Page size:\s+([\d.]+ x [\d.]+)/.exec(info(B))[1]], dx: [], dy: [], worst: null, diff: 0, px: 0 };
  for (let pg = 1; pg <= Math.min(np, nb); pg++) {
    const a = pgm(A, pg, DPI), b = pgm(B, pg, DPI);
    for (let y = 0; y + T <= a.h; y += T) for (let x = 0; x + T <= a.w; x += T) {
      let ink = 0; for (let j = y; j < y + T; j += 4) for (let i = x; i < x + T; i += 4) if (a.d[j * a.w + i] < 128) ink++; if (ink < 12) continue;
      const dx = best(prof(a, x, y, T, 'x'), prof(b, x, y, T, 'x')), dy = best(prof(a, x, y, T, 'y'), prof(b, x, y, T, 'y'));
      if (dx && dx.conf < 0.35) { r.dx.push(+dx.s.toFixed(3)); if (!r.worst || Math.abs(dx.s) > Math.abs(r.worst[0])) r.worst = [+dx.s.toFixed(3), 'x', pg, Math.round(x * K), Math.round(y * K)]; }
      if (dy && dy.conf < 0.35) { r.dy.push(+dy.s.toFixed(3)); if (!r.worst || Math.abs(dy.s) > Math.abs(r.worst[0])) r.worst = [+dy.s.toFixed(3), 'y', pg, Math.round(x * K), Math.round(y * K)]; }
    }
    const s = pgm(A, pg, 150), t = pgm(B, pg, 150), w = Math.min(s.w, t.w), h = Math.min(s.h, t.h); const crop = (im) => { const o = new PNG({ width: w, height: h }); PNG.bitblt(toPng(im), o, 0, 0, w, h, 0, 0); return o; };
    r.diff += pixelmatch(crop(s).data, crop(t).data, null, w, h, { threshold: 0.2 }); r.px += w * h;
  }
  const stat = (v) => { if (!v.length) return null; const a = v.map(Math.abs).sort((p, q) => p - q), sv = [...v].sort((p, q) => p - q); return { n: v.length, med: sv[v.length >> 1], p95: a[Math.floor(v.length * 0.95)], max: a[v.length - 1] }; };
  r.dxv = null; r.dx = stat(r.dx); r.dy = stat(r.dy); res[c] = r;
  console.log(c.padEnd(50), 'pp', r.pages.join('/'), 'dx', JSON.stringify(r.dx), 'dy', JSON.stringify(r.dy), 'diff', r.diff, 'worst', JSON.stringify(r.worst));
}
fs.writeFileSync(outf, JSON.stringify(res, null, 1)); process.exit(0);
