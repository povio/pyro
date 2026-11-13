// Boxes of collapsed-border tables. bun table-collapse.mjs chrome|<pyro binary> out.json
// For each table: every cell's border box and its content box (an inner div), in CSS px from the table.
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import { createRequire } from 'node:module';
const require = createRequire(new URL('../package.json', import.meta.url));
const { chromium } = require('playwright-core');
const [engine, outFile] = process.argv.slice(2);
const td = (css = '', attrs = '', tag = 'td') => `<${tag} ${attrs} style="${css}"><div class=i>x</div></${tag}>`;
const L1 = 'border-left:1px solid #000;padding:0 3px';
const rows = (...r) => r.map((c) => `<tr>${c.join('')}</tr>`).join('');
const table = (css, body, cols = '') => `<table style="border-collapse:collapse;${css}">${cols}<tbody>${body}</tbody></table>`;
const fixedCols = '<colgroup><col style="width:100px"><col style="width:80px"><col style="width:60px"><col style="width:120px"></colgroup>';
const CASES = {
  // ToS CMR: no borders on the first cell and the spanning one, a left border on the cells after it.
  'fixed-colspan-left': table('width:600px;table-layout:fixed', rows([td(), td(), td(), td()], [td('', 'colspan=2'), td(L1), td(L1)]), fixedCols),
  'fixed-right-on-first': table('width:600px;table-layout:fixed', rows([td('border-right:1px solid #000'), td(), td(), td()], [td(), td(), td(), td()]), fixedCols),
  'fixed-second-row-only': table('width:600px;table-layout:fixed', rows([td(), td(), td(), td()], [td(), td('border-left:2px solid #000'), td(), td()]), fixedCols),
  'auto-left': table('width:600px', rows([td('width:100px'), td(L1), td(L1)], [td(), td(L1), td(L1)])),
  'auto-shrink-left': table('', rows([td(), td(L1), td(L1)], [td(), td(L1), td(L1)])),
  'top-on-second-row': table('width:400px', rows([td(), td()], [td('border-top:2px solid #000'), td('border-top:2px solid #000')], [td(), td()])),
  'bottom-on-first-row': table('width:400px', rows([td('border-bottom:3px solid #000'), td()], [td(), td()])),
  'uniform-all': table('width:400px', rows([td('border:1px solid #000'), td('border:1px solid #000')], [td('border:1px solid #000'), td('border:1px solid #000')])),
  'uniform-3px': table('width:400px', rows([td('border:3px solid #000'), td('border:3px solid #000')], [td('border:3px solid #000'), td('border:3px solid #000')])),
  'wider-later': table('width:400px', rows([td('border:1px solid #000'), td('border:1px solid #000')], [td('border:1px solid #000'), td('border:3px solid #000')])),
  'table-border': table('width:400px;border:3px solid #000', rows([td(), td()], [td(), td()])),
  'row-border': table('width:400px', `<tr style="border-bottom:1px solid #000">${td()}${td()}</tr><tr>${td()}${td()}</tr>`),
  'tbody-border': table('width:400px', rows([td(), td()]) + `</tbody><tbody style="border-top:2px solid #000">${rows([td(), td()])}`),
  'outer-left': table('width:400px', rows([td('border-left:2px solid #000'), td()])),
  'outer-right': table('width:400px', rows([td(), td('border-right:2px solid #000')])),
  'none-style': table('width:400px', rows([td('border-right:3px none #000'), td()])),
  'hidden-wins': table('width:400px', rows([td('border-right:3px hidden #000'), td('border-left:5px solid #000')])),
  'odd-3px-edge': table('width:400px', rows([td('border-right:3px solid #000'), td()])),
  'rowspan': table('width:400px', rows([td('', 'rowspan=2'), td(L1)], [td(L1)])),
};
const run = async () => {
  const chrome = engine === 'chrome';
  const port = chrome ? 9601 : 9602;
  const p = chrome
    ? spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${fs.mkdtempSync('/tmp/tc')}`, '--no-first-run', '--disable-gpu', 'about:blank'], { stdio: 'ignore' })
    : spawn(engine, ['serve', '--port', String(port)], { stdio: 'ignore' });
  for (let i = 0; i < 400; i++) { try { if ((await fetch(`http://127.0.0.1:${port}/json/version`)).ok) break; } catch {} await new Promise((r) => setTimeout(r, 50)); }
  const b = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  const ctx = b.contexts()[0] ?? (await b.newContext());
  const page = await ctx.newPage();
  await page.setViewportSize({ width: 794, height: 600 });
  const out = {};
  for (const [name, html] of Object.entries(CASES)) {
    await page.setContent(`<!doctype html><meta charset=utf-8><style>html,body{margin:0;font:13px/18px Arial}table{border-spacing:0}td{padding:0}.i{height:18px;background:#eee}</style><div style="margin:10px">${html}</div>`);
    out[name] = await page.evaluate(() => {
      const t = document.querySelector('table');
      const o = t.getBoundingClientRect();
      const q = (r) => [r.x - o.x, r.y - o.y, r.width, r.height].map((v) => +v.toFixed(3));
      return { table: [+o.x.toFixed(3), +o.y.toFixed(3), +o.width.toFixed(3), +o.height.toFixed(3)], cells: [...t.querySelectorAll('td')].map((c) => ({ box: q(c.getBoundingClientRect()), inner: q(c.querySelector('.i').getBoundingClientRect()) })) };
    });
  }
  await b.close().catch(() => {});
  p.kill('SIGKILL');
  fs.writeFileSync(outFile, JSON.stringify(out).replace(/\},"/g, '},\n"'));
};
await run();
process.exit(0);
