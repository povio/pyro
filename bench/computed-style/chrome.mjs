// getComputedStyle answers from headless Chrome for the pyro-dom parity test
// (crates/pyro-layout/tests/computed_style.rs, `chrome_parity`).
//
//   node bench/computed-style/chrome.mjs cases.json > chrome.json
//     PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> CHROME=<headless shell>
//
// A case is {name, html, sel, props} (one element) or {name, html_file,
// width, height, stride, props} (every `stride`-th element of a page, in
// tree order). Scripts do not run: both sides style the parsed markup.
import fs from 'fs';

const { chromium } = await import(process.env.PLAYWRIGHT_CORE ?? 'playwright-core');
const cases = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
const browser = await chromium.launch({ executablePath: process.env.CHROME });
const out = [];
for (const c of cases) {
  const page = await browser.newPage({
    viewport: { width: c.width ?? 800, height: c.height ?? 600 },
    javaScriptEnabled: false,
  });
  await page.setContent(c.html ?? fs.readFileSync(c.html_file, 'utf8'), { waitUntil: 'load' });
  const r = await page.evaluate(({ sel, stride, props }) => {
    const read = (el) => {
      const cs = getComputedStyle(el);
      return Object.fromEntries(props.map((p) => [p, cs.getPropertyValue(p)]));
    };
    if (sel) {
      const el = document.querySelector(sel);
      return el ? read(el) : null;
    }
    return [...document.querySelectorAll('*')].map((el, i) => (i % stride === 0 ? read(el) : null));
  }, c);
  out.push({ name: c.name, r });
  await page.close();
}
console.log(JSON.stringify(out));
await browser.close();
