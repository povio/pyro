// usage: bun probe.mjs <fixture module> chrome|<lp binary> out.json [nameFilter]
import { createRequire } from 'node:module';
import { execFileSync, spawn } from 'node:child_process';
import { writeFileSync, mkdtempSync } from 'node:fs';
import os from 'node:os'; import path from 'node:path';
const here = path.dirname(new URL(import.meta.url).pathname);
const require = createRequire(new URL('../../package.json', import.meta.url));
const { chromium } = require('playwright-core');
const [mod, engine, outFile, filt] = process.argv.slice(2);
const { cases } = await import(path.resolve(mod));
let browser, kill = () => {};
if (engine === 'chrome') browser = await chromium.launch({ executablePath: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: true });
else { const port = 10000 + Math.floor(Math.random() * 3000); const p = spawn(engine, ['serve', '--port', String(port)], { stdio: 'ignore' }); kill = () => p.kill();
  for (let i = 0; ; i++) { try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`); break; } catch (e) { if (i > 200) throw e; await new Promise(r => setTimeout(r, 100)); } } }
const ctx = browser.contexts()[0] ?? await browser.newContext(); const page = await ctx.newPage(); const cdp = await ctx.newCDPSession(page);
const tmp = mkdtempSync(path.join(os.tmpdir(), 'prb-')); const out = {};
for (const c of cases) { if (filt && !new RegExp(filt).test(c.name)) continue;
  await page.setContent(c.html);
  const o = { paperWidth: 6, paperHeight: 4, marginTop: 0, marginBottom: 0, marginLeft: 0, marginRight: 0, scale: 1, printBackground: true, ...c.opts };
  const { data } = await cdp.send('Page.printToPDF', o);
  const f = path.join(tmp, 'x.pdf'); writeFileSync(f, Buffer.from(data, 'base64'));
  const pages = JSON.parse(execFileSync('python3', [path.join(here, 'pdfrects.py'), f], { maxBuffer: 1 << 28 }));
  const bb = execFileSync('pdftotext', ['-bbox', f, '-'], { maxBuffer: 1 << 28 }).toString();
  const ps = [...bb.matchAll(/<page [^>]*>([\s\S]*?)<\/page>/g)].map(m => [...m[1].matchAll(/<word xMin="(-?[\d.]+)" yMin="(-?[\d.]+)" xMax="(-?[\d.]+)" yMax="(-?[\d.]+)">([^<]*)<\/word>/g)].map(w => ({ x: +w[1], y: +w[2], x1: +w[3], y1: +w[4], t: w[5] })));
  out[c.name] = { pages: ps.length, words: ps, rects: pages.map(p => p.rects), mb: pages[0]?.mediabox };
}
writeFileSync(outFile, JSON.stringify(out));
await browser.close(); kill(); process.exit(0);
