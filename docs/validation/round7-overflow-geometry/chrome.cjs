const { createRequire } = require('node:module');
const path = require('node:path');
const { chromium } = createRequire(path.resolve('bench/package.json'))('playwright-core');
const urls = require('./urls.json');

(async () => {
  const browser = await chromium.launch({ executablePath: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: true });
  try {
    const rows = {};
    for (const url of urls) {
      const page = await browser.newPage();
      await page.addInitScript(() => {
        window.addEventListener('DOMContentLoaded', () => {
          add_completion_callback((tests, harness) => {
            window.snapshot = [harness.status, tests.map(test => [test.name, test.status, test.message])];
          });
        });
      });
      await page.goto(`${process.argv[2]}/${url}`);
      await page.waitForFunction(() => window.snapshot);
      rows[url] = await page.evaluate(() => window.snapshot);
      await page.close();
    }
    console.log(JSON.stringify(rows));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
