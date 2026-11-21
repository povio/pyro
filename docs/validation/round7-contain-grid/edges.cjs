const path = require('node:path');
const { chromium } = require('node:module').createRequire(path.resolve('bench/package.json'))('playwright-core');
(async () => {
  const browser = process.argv[2] === 'chrome' ? await chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'}) : await chromium.connectOverCDP(process.argv[2]);
  try {
    const page = await browser.newPage();
    const rows = await page.evaluate(() => {
      const rows = [];
      for (const contain of ['none','layout']) for (const content of ['implicit','explicit','empty','wide-item'])
        for (const width of [0,1]) for (const pad of ['0','0 1px','1px 0']) for (const overflow of ['visible','hidden']) {
          const box = document.createElement('div');
          box.style.cssText = `display:grid;contain:${contain};width:${width}px;height:0;padding:${pad};overflow:${overflow}`;
          if (content === 'explicit' || content === 'empty') box.style.gridTemplate = '2000px / 0px';
          if (content !== 'empty') {
            const child = document.createElement('div');
            child.style.cssText = 'height:2000px;' + (content === 'wide-item' ? 'width:10px' : '');
            box.append(child);
          }
          document.body.append(box);
          rows.push([contain,content,width,pad,overflow,box.scrollWidth,box.scrollHeight]);
          box.remove();
        }
      return rows;
    });
    console.log(JSON.stringify(rows));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
