const { chromium } = require('node:module').createRequire(require('node:path').resolve('bench/package.json'))('playwright-core');
(async () => {
  const browser = process.argv[2] === 'chrome' ? await chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'}) : await chromium.connectOverCDP(process.argv[2]);
  try {
    const page = await browser.newPage();
    console.log(JSON.stringify(await page.evaluate(() => {
      const rows = [];
      for (const mode of ['horizontal-tb','vertical-lr','vertical-rl','sideways-lr','sideways-rl']) for (const direction of ['ltr','rtl']) {
        for (const block of ['start','center','end','nearest']) for (const inline of ['start','center','end','nearest']) {
          const box = document.createElement('div');
          box.style.cssText = `position:relative;overflow:hidden;width:100px;height:120px;writing-mode:${mode};direction:${direction}`;
          box.innerHTML = '<div style="position:relative;width:500px;height:500px"><div style="position:absolute;top:200px;left:210px;width:20px;height:30px"></div></div>';
          document.body.append(box);
          const target = box.firstChild.firstChild;
          const read = () => {const rect=target.getBoundingClientRect(), port=box.getBoundingClientRect();return [rect.left-port.left,rect.top-port.top,box.scrollLeft,box.scrollTop]};
          const before=read(); target.scrollIntoView({block,inline}); rows.push([mode,direction,block,inline,before,read()]); box.remove();
        }
      }
      return rows;
    })));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode=1; });
