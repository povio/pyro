const { createRequire } = require('node:module');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const { chromium } = createRequire(path.resolve('bench/package.json'))('playwright-core');

(async () => {
  const endpoint = process.argv[2];
  const browser = endpoint === 'chrome'
    ? await chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'})
    : await chromium.connectOverCDP(endpoint);
  try {
    const page = await browser.newPage();
    const rows = [];
    for (const file of ['../round7-overflow-geometry/controls.html','controls.html']) {
      const html = readFileSync(path.join(__dirname, file), 'utf8');
      await page.goto('data:text/html,' + encodeURIComponent(html));
      rows.push(...JSON.parse(await page.locator('#results').textContent()));
    }
    console.log(JSON.stringify(rows));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
