const { createRequire } = require('node:module');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const { chromium } = createRequire(path.resolve('bench/package.json'))('playwright-core');

(async () => {
  const browser = await chromium.connectOverCDP(process.argv[2]);
  try {
    const page = await browser.newPage();
    const html = readFileSync(path.join(__dirname, 'controls.html'), 'utf8');
    await page.goto('data:text/html,' + encodeURIComponent(html));
    console.log(await page.locator('#results').textContent());
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
