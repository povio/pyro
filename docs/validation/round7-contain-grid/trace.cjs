const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require('node:module').createRequire(path.resolve('bench/package.json'))('playwright-core');
const url = 'css/cssom-view/scrollWidthHeight-contain-layout.html';
const original = fs.readFileSync(path.join(process.env.WPT_DIR, url), 'utf8');
const scripts = ['testharness', 'testharnessreport'];
let standalone = original;
for (const script of scripts) {
  const src = `/resources/${script}.js`;
  const code = fs.readFileSync(path.join(process.env.WPT_DIR, src), 'utf8');
  standalone = standalone.replace(`<script src="${src}"></script>`, () => `<script>${code.replace(/<\/script/gi, '<\\/script')}</script>`);
}
function inject(text, anchor, code) {
  if (text.split(anchor).length !== 2) throw Error(anchor);
  return text.replace(anchor, () => code + '\n' + anchor);
}
function fixture(mode) {
  let html = inject(standalone, 'let target = document', `window.rows = []; window.names = []; add_result_callback(t => names.push([t.name,t.status]));`);
  if (mode !== 'ordered') html = inject(html, 'target.style.display = display;', `const fresh = target.cloneNode(true); fresh.removeAttribute('style'); target.replaceWith(fresh); target = fresh;`);
  if (mode === 'static') html = inject(html, 'let visibleH', `target.remove(); document.body.appendChild(target);`);
  html = inject(html, 'target.style.overflow = overflow;', `const before = [getComputedStyle(target).overflowX,getComputedStyle(target).overflowY];`);
  if (mode === 'static') html = inject(html, 'let scrollableH', `const freshAfter = target.cloneNode(true); target.replaceWith(freshAfter); target = freshAfter;`);
  return inject(html, 'assert_equals(visibleH', `rows.push({display,overflow,padding,border,before,after:[getComputedStyle(target).overflowX,getComputedStyle(target).overflowY],visibleH,visibleW,scrollableH,scrollableW});`);
}
(async () => {
  const browser = process.argv[2] === 'chrome' ? await chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'}) : await chromium.connectOverCDP(process.argv[2]);
  try {
    const data = {};
    for (const mode of ['ordered','fresh','static']) {
      const page = await browser.newPage();
      await page.goto('data:text/html;charset=utf-8,' + encodeURIComponent(fixture(mode)));
      data[mode] = await page.evaluate(() => ({rows:window.rows,names:window.names}));
      if (data[mode].rows?.length !== 128 || data[mode].names?.length !== 128) throw Error('incomplete ' + mode);
      await page.close();
    }
    console.log(JSON.stringify(data));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
