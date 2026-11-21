const { chromium } = require('node:module').createRequire(require('node:path').resolve('bench/package.json'))('playwright-core');
const fs = require('node:fs');
(async () => {
  const browser = process.argv[2] === 'chrome' ? await chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'}) : await chromium.connectOverCDP(process.argv[2]);
  try {
    const rows = [];
    for (const url of require('./urls.json').filter(url => url.includes('sideways-lr'))) {
      const page = await browser.newPage();
      const fixture = fs.readFileSync(`${process.env.WPT_DIR}/${url}`, 'utf8').replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, '');
      await page.goto('data:text/html,' + encodeURIComponent(fixture));
      rows.push([url, await page.evaluate(() => {
        const box=document.querySelector('#scroller'), target=document.querySelector('#target');
        const position = () => {const port=box.getBoundingClientRect(), rect=target.getBoundingClientRect();return [rect.left-port.left,rect.top-port.top,box.scrollLeft,box.scrollTop,box.clientWidth,box.clientHeight]};
        const before=position(); target.scrollIntoView({block:'start',inline:'start'}); return [before,position()];
      })]); await page.close();
    }
    console.log(JSON.stringify(rows));
  } finally { await browser.close(); }
})().catch(error => {console.error(error);process.exitCode=1});
