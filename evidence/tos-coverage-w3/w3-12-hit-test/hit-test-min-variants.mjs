// W3-12 minimal static repro variants. Usage: node hit-test-min-variants.mjs [lightpanda-binary]
// (no binary: Playwright's Chromium, or CHROME_PATH). Same select-trigger shape as the TOS route table cell:
// TD(h 26) > DIV > DIV > DIV[tabindex=0] (click handler) > DIV > DIV(pointer-events none) > BUTTON.
// Variants: pointer-events via inline style, via a stylesheet class (as Tailwind does), and
// a control with overflow:hidden on the TD but no pointer-events:none.
import { createRequire } from 'node:module';
import { spawn } from 'node:child_process';
import net from 'node:net';
// playwright-core 1.59.1 is resolved from bench/ (or from PW_REQUIRE_FROM, a package.json path).
const require = createRequire(process.env.PW_REQUIRE_FROM ?? new URL('../../../bench/package.json', import.meta.url));
const { chromium } = require('playwright-core');
const bin = process.argv[2];
let browser, child;
if (bin) {
  const port = await new Promise(r => { const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => r(p)); }); });
  child = spawn(bin, ['serve', '--host', '127.0.0.1', '--port', String(port), '--log-level', 'error'], { stdio: 'ignore' });
  for (let i = 0; i < 200; i++) { try { if ((await fetch(`http://127.0.0.1:${port}/json/version`)).ok) break; } catch {} await new Promise(r => setTimeout(r, 25)); }
  browser = await chromium.connectOverCDP(`ws://127.0.0.1:${port}`);
} else browser = await chromium.launch({ executablePath: process.env.CHROME_PATH });
const page = await (await browser.newContext({ viewport: { width: 1200, height: 800 } })).newPage();
const cell = (inner, tdAttrs = 'style="height:26px"') => `<table><tbody><tr><td ${tdAttrs}><div><div><div class="wrap" tabindex="0" style="position:relative;display:flex"><div style="display:flex">${inner}</div></div></div></div></td></tr></tbody></table>`;
const variants = {
  inlinePointerEventsNone: cell('<div style="pointer-events:none"><button>CPT</button></div>'),
  classPointerEventsNone: '<style>.pointer-events-none{pointer-events:none}</style>' + cell('<div class="pointer-events-none"><button>CPT</button></div>'),
  controlNoPointerEventsTdOverflowHidden: '<style>.overflow-hidden{overflow:hidden}</style>' + cell('<div><button>CPT</button></div>', 'class="overflow-hidden" style="height:26px"'),
};
const out = { engine: bin ? bin.split('/').slice(-2, -1)[0] : browser.version() };
for (const [name, html] of Object.entries(variants)) {
  await page.goto('data:text/html,' + encodeURIComponent(html));
  const r = await page.evaluate(() => {
    const rect = (n) => { const x = n.getBoundingClientRect(); return [x.x, x.y, x.width, x.height].map(Math.round); };
    const b = document.querySelector('button'), w = document.querySelector('.wrap');
    const br = b.getBoundingClientRect();
    const hit = document.elementFromPoint(br.x + br.width / 2, br.y + br.height / 2);
    globalThis.clicked = false; w.addEventListener('click', () => { clicked = true; });
    return { td: rect(document.querySelector('td')), wrap: rect(w), button: rect(b), buttonPointerEvents: getComputedStyle(b).pointerEvents, tdOverflow: getComputedStyle(document.querySelector('td')).overflow, hit: hit === b ? 'button' : hit === w ? 'wrap' : hit && w.contains(hit) ? 'inside-wrap:' + hit.tagName : hit && hit.tagName };
  });
  const box = await page.locator('button').boundingBox();
  await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
  r.clickReachedWrap = await page.evaluate(() => clicked);
  out[name] = r;
}
console.log(JSON.stringify(out));
await browser.close(); child?.kill('SIGTERM');
