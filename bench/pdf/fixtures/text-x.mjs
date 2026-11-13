// bun text-x.mjs <work dir> <pyro binary>: paper-cut and `.pz-page` documents with fractional text x, printed by
// Chrome and Pyro into <work dir>/{chrome,lp}, and screenshots of them into <work dir>/shots-lp, for pdf-deltas.mjs.
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
const require = createRequire(new URL('../package.json', import.meta.url));
const { chromium } = require('playwright-core');
const [work, bin] = process.argv.slice(2);
const para = (n) => Array.from({ length: n }, (_, i) => `<p style="margin:0 0 8px">Line ${i} of ordinary text with words, numbers 12345 and a <b>bold run</b>.</p>`).join('');
const head = '<!doctype html><meta charset=utf-8><style>html,body{margin:0;font:13px/18px Arial,sans-serif}</style>';
const PAD = [37.7953, 10.5, 94.47, 12.25];
const DOCS = {};
for (const p of PAD) {
  DOCS[`cut-pad-${p}`] = `${head}<div style="padding-left:${p}px;width:500px">${para(12)}</div>`;
  DOCS[`cut-nest-${p}`] = `${head}<div style="margin-left:${p}px"><div style="padding-left:3.3px;border-left:1.2px solid #000">${para(8)}</div></div>`;
  DOCS[`pz-pad-${p}`] = `${head}<style>@page{margin:0;size:210mm 297mm}</style><div class=pz-page style="width:210mm;height:297mm;box-sizing:border-box;padding-left:${p}px">${para(12)}</div>`;
}
DOCS['cut-center'] = `${head}<div style="width:500px;text-align:center">${para(10)}</div>`;
DOCS['cut-right'] = `${head}<div style="width:500.3px;text-align:right">${para(10)}</div>`;
DOCS['cut-flex'] = `${head}<div style="display:flex;gap:7.3px"><div style="flex:1">${para(6)}</div><div style="flex:1.7">${para(6)}</div><div style="flex:.9">${para(6)}</div></div>`;
DOCS['cut-table'] = `${head}<table style="width:100%;border-collapse:collapse"><tr><td style="padding:3.3px">${para(2)}</td><td style="padding:3.3px;width:33.3%">${para(2)}</td></tr></table>`;
DOCS['cut-inline-block'] = `${head}<div style="padding-left:5.5px">Before <span style="display:inline-block;margin-left:3.3px;padding:2.2px">inline block</span> after, and <u style="margin-left:1.1px">underlined</u> text</div>`;
const SIZES = { Letter: { width: '8.5in', height: '11in' }, A4: { width: '8.27in', height: '11.7in' } };
const MARGINS = { m0: 0, 'm.4': 0.4 };
const run = async (name, cmd, args, port, shots) => {
  const p = spawn(cmd, args, { stdio: 'ignore' });
  for (let i = 0; i < 400; i++) { try { if ((await fetch(`http://127.0.0.1:${port}/json/version`)).ok) break; } catch {} await new Promise((r) => setTimeout(r, 50)); }
  const b = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  const ctx = b.contexts()[0] ?? (await b.newContext());
  const page = await ctx.newPage();
  const dir = path.join(work, name);
  fs.mkdirSync(dir, { recursive: true });
  await page.emulateMedia({ media: 'screen' });
  await page.setViewportSize({ width: 794, height: 1123 });
  for (const [d, html] of Object.entries(DOCS)) {
    await page.setContent(html);
    for (const [paper, size] of Object.entries(SIZES)) for (const [mn, m] of Object.entries(MARGINS)) {
      if (d.startsWith('pz') && mn !== 'm0') continue;
      const margin = { top: `${m}in`, right: `${m}in`, bottom: `${m}in`, left: `${m}in` };
      fs.writeFileSync(path.join(dir, `${d}__${paper}__${mn}.pdf`), await page.pdf({ ...size, margin, printBackground: true }));
    }
    if (shots) {
      fs.mkdirSync(path.join(work, shots), { recursive: true });
      fs.writeFileSync(path.join(work, shots, `${d}.png`), await page.screenshot());
    }
  }
  await b.close().catch(() => {});
  p.kill('SIGKILL');
};
if (bin === 'chrome') await run('chrome', '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new', '--remote-debugging-port=9581', `--user-data-dir=${path.join(work, '.c')}`, '--no-first-run', '--disable-gpu', 'about:blank'], 9581);
else await run(process.argv[4] ?? 'lp', bin, ['serve', '--port', '9582'], 9582, process.argv[5] ?? 'shots-lp');
process.exit(0);
