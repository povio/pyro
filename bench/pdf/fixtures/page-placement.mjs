// How Chrome (and optionally Lightningpanda) places page boxes on paper.
//   bun placement.mjs <work dir> <out.json> [lightningpanda binary]
// Prints every document on every paper: MediaBox, pages, Chrome's first transform and clip of page 1, and the
// ink box (pt) of a 50 x 20 px black box at (100, 100) in the page. Also the 300 dpi page size of whole-point papers.
import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const { chromium } = require('playwright-core');
const [work, outFile, bin] = process.argv.slice(2);
fs.mkdirSync(work, { recursive: true });
const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const mark = '<div style="position:absolute;left:100px;top:100px;width:50px;height:20px;background:#000"></div>';
const box = (w, h) =>
  Array.from({ length: 2 }, () => `<div class=pz-page style="width:${w};height:${h};position:relative;overflow:hidden;break-after:page">${mark}</div>`).join('');
const head = (css) => `<!doctype html><meta charset=utf-8><style>html,body{margin:0}${css}</style>`;
const DOCS = {
  plain: head('') + '<div style="margin:100px 0 0 100px;width:50px;height:20px;background:#000"></div>',
  'page-a4-margin': head('@page{margin:10mm;size:210mm 297mm}') + '<div style="margin:100px 0 0 100px;width:50px;height:20px;background:#000"></div>',
  'pz-a4': head('@page{margin:0}') + box('210mm', '297mm'),
  'pz-a4-size': head('@page{margin:0;size:210mm 297mm}') + box('210mm', '297mm'),
  'pz-a4-named': head('@page{margin:0;size:auto!important}@page s{margin:0;size:210mm 297mm}') + `<section style="page: s">${box('210mm', '297mm')}</section>`,
  'pz-a4-named-print-only': head('@media print{@page{margin:0;size:auto!important}}@page s{margin:0;size:210mm 297mm}') + `<section style="page: s">${box('210mm', '297mm')}</section>`,
  'pz-letter-size': head('@page{margin:0;size:8.5in 11in}') + box('8.5in', '11in'),
  'pz-500': head('@page{margin:0}') + box('500px', '700px'),
  'pz-500-size': head('@page{margin:0;size:500px 700px}') + box('500px', '700px'),
  'pz-1000-size': head('@page{margin:0;size:1000px 1400px}') + box('1000px', '1400px'),
  'pz-land': head('@page{margin:0;size:297mm 210mm}') + box('297mm', '210mm'),
};
const PAPERS = { A4: { width: '8.27in', height: '11.7in' }, Letter: { width: '8.5in', height: '11in' }, A3: { width: '11.7in', height: '16.54in' }, small: { width: '6.3in', height: '9.1in' } };
const OPTIONS = {
  plain: {},
  landscape: { landscape: true },
  preferCSS: { preferCSSPageSize: true },
  'scale.8': { scale: 0.8 },
  'scale1.5': { scale: 1.5 },
  'margin.4': { margin: { top: '.4in', right: '.4in', bottom: '.4in', left: '.4in' } },
};
const pageStreams = (file) => {
  const d = fs.readFileSync(file), out = [];
  for (let i = d.indexOf('stream'); i >= 0; i = d.indexOf('stream', i + 6)) {
    if (d.slice(i - 3, i).toString() === 'end') continue;
    const s = i + 6 + (d[i + 6] === 13 ? 2 : 1), e = d.indexOf('endstream', s);
    try {
      const t = zlib.inflateSync(d.subarray(s, e)).toString('latin1');
      if (/^\s*\.2399\d* 0 0 -\.2399\d* 0 [\d.]+ cm/.test(t)) out.push(t);
    } catch {}
    i = e;
  }
  return out;
};
const chromePage = (file) => {
  const t = pageStreams(file)[0];
  if (!t) return {};
  const head = t.slice(0, 600);
  const clip = /q\s+(-?[\d.]+) (-?[\d.]+) (-?[\d.]+) (-?[\d.]+) re\s+W\*? n/.exec(head);
  const tr = /q\s+([\d.]+) 0 0 \1 (-?[\d.]+) (-?[\d.]+) cm/.exec(head);
  return { clip: clip?.slice(1).map(Number), scale: tr ? +tr[1] / 3.125 : undefined, cssPx: tr ? [+tr[2] / 3.125, +tr[3] / 3.125] : undefined };
};
const ink = (file) => {
  const o = execFileSync('pdftoppm', ['-r', 288, '-f', 1, '-l', 1, '-gray', file], { maxBuffer: 1 << 29, stdio: ['ignore', 'pipe', 'ignore'] });
  let i = 0, tok = [];
  while (tok.length < 4) { while (o[i] <= 32) i++; const s = i; while (o[i] > 32) i++; tok.push(o.slice(s, i).toString()); }
  i++;
  const [w, h] = [+tok[1], +tok[2]], d = o.subarray(i);
  let r = null;
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) if (d[y * w + x] < 128) { r = r ?? [x, y, x, y]; r = [Math.min(r[0], x), r[1], Math.max(r[2], x), y]; }
  return r && [r[0] / 4, r[1] / 4, (r[2] + 1) / 4, (r[3] + 1) / 4];
};
const info = (file) => {
  const t = execFileSync('pdfinfo', [file], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] });
  return { media: /Page size:\s+([\d.]+ x [\d.]+)/.exec(t)[1], pages: +/Pages:\s+(\d+)/.exec(t)[1] };
};
const engine = async (name, cmd, args, port, chrome) => {
  const p = spawn(cmd, args, { stdio: 'ignore' });
  for (let i = 0; i < 400; i++) { try { if ((await fetch(`http://127.0.0.1:${port}/json/version`)).ok) break; } catch {} await new Promise((r) => setTimeout(r, 50)); }
  const b = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  const ctx = b.contexts()[0] ?? (await b.newContext());
  const page = await ctx.newPage();
  await page.emulateMedia({ media: 'screen' });
  await page.setViewportSize({ width: 794, height: 1123 });
  const rows = {};
  const file = path.join(work, `${name}.pdf`);
  for (const [doc, html] of Object.entries(DOCS)) {
    await page.setContent(html);
    for (const [paper, size] of Object.entries(PAPERS)) for (const [option, o] of Object.entries(OPTIONS)) {
      if ((paper === 'A3' || paper === 'small') && !['plain', 'preferCSS'].includes(option)) continue;
      const pdf = await page.pdf({ ...size, printBackground: true, margin: { top: 0, right: 0, bottom: 0, left: 0 }, ...o }).catch((e) => null);
      if (!pdf) continue;
      fs.writeFileSync(file, pdf);
      rows[`${doc} ${paper} ${option}`] = { ...info(file), ink: ink(file), ...(chrome ? chromePage(file) : {}) };
    }
  }
  const dots = {};
  if (chrome) {
    const cdp = await ctx.newCDPSession(page);
    await page.setContent('<!doctype html><p>x');
    for (const pt of [261, 267, 273, 381, 387, 393, 500, 501, 502, 503, 504, 505, 506, 507, 595, 596, 612, 842, 843, 849, 855, 1191, 1197]) {
      const { data } = await cdp.send('Page.printToPDF', { paperWidth: pt / 72, paperHeight: pt / 72, marginTop: 0, marginBottom: 0, marginLeft: 0, marginRight: 0 });
      fs.writeFileSync(file, Buffer.from(data, 'base64'));
      const media = +info(file).media.split(' x ')[0];
      const predicted = Math.floor(Math.fround(pt * Math.fround(1 / 0.24)) + 0.5);
      dots[pt] = { media, dots: Math.round(media / 0.24), predicted };
    }
  }
  await b.close().catch(() => {});
  p.kill('SIGKILL');
  return { rows, dots };
};
const chrome = await engine('chrome', CHROME, ['--headless=new', '--remote-debugging-port=9561', `--user-data-dir=${path.join(work, '.chrome')}`, '--no-first-run', '--disable-gpu', 'about:blank'], 9561, true);
const lp = bin ? await engine('lp', bin, ['serve', '--port', '9562'], 9562, false) : null;
const close = (a, b) => a && b && a.every((v, i) => Math.abs(v - b[i]) <= 0.25);
const compare = lp && Object.fromEntries(Object.keys(chrome.rows).filter((k) => lp.rows[k]).map((k) => [k, chrome.rows[k].media === lp.rows[k].media && chrome.rows[k].pages === lp.rows[k].pages && !!close(chrome.rows[k].ink, lp.rows[k].ink)]));
fs.writeFileSync(outFile, JSON.stringify({ chrome: chrome.rows, dots: chrome.dots, lp: lp?.rows, matches: compare }, null, 0).replace(/\},"/g, '},\n"'));
if (compare) {
  const bad = Object.entries(compare).filter(([, ok]) => !ok).map(([k]) => k);
  console.log(`${Object.keys(compare).length - bad.length} of ${Object.keys(compare).length} match`);
  console.log(bad.join('\n'));
}
const wrong = Object.entries(chrome.dots).filter(([, v]) => v.dots !== v.predicted);
console.log('dots rule mismatches', wrong.length, JSON.stringify(wrong));
process.exit(0);
