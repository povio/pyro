// The ToS PdfRenderer flow (tos-monorepo apps/be/src/modules/document/pdf-renderer.ts:
// preparePage, render, releasePage) with puppeteer-core 25 against a
// Lightningpanda server, over every case, from a pool of 3 pages.
//   bun puppeteer-flow.mjs <binary> <pdfgen build dir> <cases dir> <out dir>
import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import puppeteer from 'puppeteer-core';

const [bin, buildDir, casesDir, outDir] = process.argv.slice(2);
const DOCUMENT_VIEWPORT = { deviceScaleFactor: 1, hasTouch: false, height: 1080, isLandscape: false, isMobile: false, width: 794 };
const proc = spawn(bin, ['serve', '--port', '9499', '--static', `http://pdfgen.localhost=${path.resolve(buildDir)}`], { stdio: 'ignore' });
process.on('exit', () => proc.kill());
for (let i = 0; i < 200; i++) {
  try {
    if ((await fetch('http://127.0.0.1:9499/json/version')).ok) break;
  } catch {}
  await new Promise((r) => setTimeout(r, 50));
}
const browser = await puppeteer.connect({ browserURL: 'http://127.0.0.1:9499', defaultViewport: DOCUMENT_VIEWPORT });

// --- pdf-renderer.ts, unchanged apart from the bundle URL (no file: scheme) ---
const postRenderMessage = ({ data, token }) => {
  globalThis.postMessage({ type: 'generate-pdf', data, generationToken: token });
};
const waitForDocumentFonts = async () => {
  await globalThis.document.fonts.ready;
};
const resetPage = async (page) => {
  await page.emulateMediaType('screen');
  await page.setViewport({ height: 1123, width: 794 });
};
const preparePage = async (page) => {
  await resetPage(page);
  await page.goto('http://pdfgen.localhost/index.html', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('html[data-pdfgen-ready="true"]');
};
let renderSequence = 0;
const render = async (page, request) => {
  await resetPage(page);
  const token = `pdf-${++renderSequence}`;
  await page.evaluate(postRenderMessage, { data: request.data, token });
  await page.waitForSelector(`#document-generated[data-generation-token="${token}"]`);
  await page.evaluate(waitForDocumentFonts);
  return Buffer.from(
    await page.pdf({
      displayHeaderFooter: false,
      format: request.format,
      landscape: request.data.landscape === true,
      margin: { bottom: '0px', left: '0px', right: '0px', top: '0px' },
      preferCSSPageSize: false,
      printBackground: true,
      scale: 1,
      tagged: false,
      waitForFonts: true,
    }),
  );
};
// ---

const manifest = JSON.parse(await readFile(path.join(casesDir, 'pdf-visual-cases.json'), 'utf8'));
const cases = manifest.cases.filter((c) => c.enabled !== false);
const requests = await Promise.all(
  cases.map(async (c) => {
    const source = JSON.parse(await readFile(path.resolve(casesDir, c.payloadFile ?? c.dataFile), 'utf8'));
    const wrapped = source && typeof source === 'object' && ('template' in source || 'data' in source);
    const base = c.dataFile || !wrapped ? { data: source } : source;
    const data = { data: base.data ?? {}, language: c.language ?? base.language, showPageBreaks: c.showPageBreaks ?? base.showPageBreaks, template: c.template ?? base.template };
    const landscape = c.landscape === true || data.template === 'BmdReport';
    return { name: c.name, format: 'A4', data: landscape ? { ...data, landscape: true } : data };
  }),
);
await mkdir(outDir, { recursive: true });
const pool = [];
for (let i = 0; i < 3; i++) {
  const page = await browser.newPage();
  await preparePage(page);
  pool.push(page);
}
const t0 = performance.now();
let ok = 0;
const failures = [];
await Promise.all(
  pool.map(async (page) => {
    for (let r = requests.shift(); r; r = requests.shift()) {
      try {
        const pdf = await render(page, r);
        if (pdf.subarray(0, 5).toString() !== '%PDF-') throw new Error('not a PDF');
        await writeFile(path.join(outDir, `${r.name}.pdf`), pdf);
        ok++;
      } catch (e) {
        failures.push(`${r.name}: ${String(e).slice(0, 200)}`);
      }
      await preparePage(page); // releasePage
    }
  }),
);
console.log(`puppeteer-core ${(await import('puppeteer-core/package.json', { with: { type: 'json' } })).default.version}: ${ok}/${cases.length} PDFs in ${(performance.now() - t0).toFixed(0)} ms`);
for (const f of failures) console.log('FAIL', f);
for (const p of pool) await p.close();
await browser.disconnect();
proc.kill();
