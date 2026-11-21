const { chromium } = require('node:module').createRequire(require('node:path').resolve('bench/package.json'))('playwright-core');

(async () => {
  const chrome = process.argv[2] === 'chrome';
  const browser = chrome ? await chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'}) : await chromium.connectOverCDP(process.argv[2]);
  try {
    const page = await browser.newPage();
    const rows = [];
    for (const mode of ['sideways-lr','vertical-lr','sideways-rl','vertical-rl','horizontal-tb']) {
      for (const direction of ['ltr','rtl']) {
        for (const side of ['left','right','inline-start','inline-end','none']) {
          for (const groups of [1,3]) {
            const boxes = '<div class="box"></div>'.repeat(9/groups);
            const group = `<div class="row">${boxes}</div>`;
            const html = `<!doctype html><style>#port{width:300px;height:300px;overflow:scroll;writing-mode:${mode};direction:${direction}}#container{width:600px;height:600px}.box{width:200px;height:200px;float:${side}}</style><div id="port"><div id="container">${group.repeat(groups)}</div></div>`;
            await page.goto('data:text/html,'+encodeURIComponent(html));
            const geometry = await page.evaluate(() => {
              const parent = document.querySelector('#container').getBoundingClientRect();
              const rects = selector => Array.from(document.querySelectorAll(selector), node => {
                const rect = node.getBoundingClientRect();
                return [rect.left-parent.left,rect.top-parent.top,rect.width,rect.height];
              });
              return {rows:rects('.row'), boxes:rects('.box')};
            });
            rows.push({mode,direction,side,groups,...geometry});
          }
        }
      }
    }
    console.log(JSON.stringify(rows));
  } finally { await browser.close(); }
})().catch(error => {console.error(error);process.exitCode=1});
