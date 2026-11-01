// RSS of one lightningpanda server over N navigations of one page, each
// followed (with --flow) by the ToS generate + printToPDF steps.
//   bun navmem.mjs <binary> <build dir> <N> [payload.json]
import { chromium } from 'playwright-core';
import { spawn, execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
const [bin, dir, n, payloadFile] = process.argv.slice(2);
const proc = spawn(bin, ['serve', '--port', '9488', '--static', `http://site.localhost=${dir}`], { stdio: 'ignore' });
await new Promise((r) => setTimeout(r, 1000));
const rss = () => Math.round(Number(execFileSync('ps', ['-o', 'rss=', '-p', String(proc.pid)], { encoding: 'utf8' })) / 1024);
const b = await chromium.connectOverCDP('http://127.0.0.1:9488');
const page = await b.contexts()[0].newPage();
const payload = payloadFile && JSON.parse(readFileSync(payloadFile, 'utf8'));
const out = [];
let peak = 0;
const t0 = performance.now();
for (let i = 1; i <= Number(n); i++) {
  await page.goto('http://site.localhost/index.html', { waitUntil: 'load' });
  if (payload) {
    await page.waitForSelector('html[data-pdfgen-ready="true"]', { state: 'attached' });
    await page.evaluate(({ p, t }) => postMessage({ type: 'generate-pdf', data: p, generationToken: t }), { p: payload, t: `t${i}` });
    await page.waitForSelector(`#document-generated[data-generation-token="t${i}"]`, { state: 'attached' });
    await page.pdf({ format: 'A4', printBackground: true });
  }
  const r = rss();
  peak = Math.max(peak, r);
  if (i % Math.max(1, Math.floor(Number(n) / 8)) === 0) out.push(`${i}:${r}`);
}
console.log(out.join(' '), `peak ${peak} MB, ${((performance.now() - t0) / Number(n)).toFixed(0)} ms/iteration`);
await b.close();
proc.kill();
