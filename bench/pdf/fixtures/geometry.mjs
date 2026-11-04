// usage: node measure.mjs chrome|<lp binary> out.json
import { createRequire } from 'node:module';
import { execFileSync, spawn } from 'node:child_process';
import { writeFileSync, mkdtempSync } from 'node:fs';
import os from 'node:os'; import path from 'node:path';
const here = path.dirname(new URL(import.meta.url).pathname);
const require = createRequire(new URL('../../package.json', import.meta.url));
const { chromium } = require('playwright-core');
const engine = process.argv[2], outFile = process.argv[3];
const PAPERS = { Letter: [8.5, 11], A4: [8.27, 11.69], A4x: [8.2677165, 11.6929134], Legal: [8.5, 14], Tiny: [4.01, 5.03] };
const M = process.env.QUICK ? [0.4, 0] : [0.4, 1 / 2.54, 0.5, 0, 0.25, 0.1, 0.39, 0.41, 1, 0.123];
const SC = [1, 0.8, 1.5];
const html = (n) => `<!doctype html><style>html,body{margin:0;padding:0}a{display:block;height:1px;background:#000}</style>` + Array.from({ length: n }, (_, i) => `<a href="https://x.test/${i}"></a>`).join('');
let browser, kill = () => {};
if (engine === 'chrome') browser = await chromium.launch({ executablePath: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: true });
else {
  const port = 9300 + Math.floor(Math.random() * 500);
  const p = spawn(engine, ['serve', '--port', String(port)], { stdio: 'ignore' }); kill = () => p.kill();
  for (let i = 0; ; i++) { try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`); break; } catch (e) { if (i > 50) throw e; await new Promise(r => setTimeout(r, 100)); } }
}
const ctx = browser.contexts()[0] ?? await browser.newContext();
const page = await ctx.newPage();
const cdp = await ctx.newCDPSession(page);
await page.setContent(html(+(process.env.ROWS || 3000)));
const tmp = mkdtempSync(path.join(os.tmpdir(), 'pdfm-'));
const out = [];
const cases = [];
for (const [pn, [w, h]] of Object.entries(PAPERS)) for (const m of M) for (const s of SC) cases.push({ pn, w, h, t: m, r: m, b: m, l: m, s });
cases.push({ pn: 'Letter', w: 8.5, h: 11, t: 0.3, r: 0.2, b: 0.7, l: 0.9, s: 1 }, { pn: 'A4', w: 8.27, h: 11.69, t: 0.77, r: 0.13, b: 0.21, l: 0.55, s: 0.8 });
for (const c of cases) {
  const { data } = await cdp.send('Page.printToPDF', { paperWidth: c.w, paperHeight: c.h, marginTop: c.t, marginRight: c.r, marginBottom: c.b, marginLeft: c.l, scale: c.s, printBackground: true });
  const f = path.join(tmp, 'x.pdf'); writeFileSync(f, Buffer.from(data, 'base64'));
  const pages = JSON.parse(execFileSync('python3', [path.join(here, 'pdfrects.py'), f], { maxBuffer: 1 << 28 }));
  const summ = pages.map((p) => { const r = p.rects; if (!r.length) return { mb: p.mediabox, n: 0 };
    return { mb: p.mediabox, n: r.length, x0: Math.min(...r.map(a => a[0])), x1: Math.max(...r.map(a => a[2])), top: Math.max(...r.map(a => a[3])), bot: Math.min(...r.map(a => a[1])) }; });
  out.push({ ...c, pages: summ.length, p1: summ[0], p2: summ[1] });
  console.log(c.pn, 'm', [c.t, c.r, c.b, c.l].map(x => +x.toFixed(4)).join('/'), 's', c.s, 'pages', summ.length, JSON.stringify(summ[0]));
}
writeFileSync(outFile, JSON.stringify(out, null, 1));
await browser.close(); kill(); process.exit(0);
