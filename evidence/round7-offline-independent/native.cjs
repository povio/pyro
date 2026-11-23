const {chromium} = require('../../bench/node_modules/playwright-core');
const fs = require('node:fs');
(async () => {
  const browser = await chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', chromiumSandbox:true});
  try {
    const page = await browser.newPage(), results = {version: browser.version(), started: new Date().toISOString(), runs: []};
    const candidate = process.argv[2];
    if (candidate) {
      const source = fs.readFileSync(candidate,'utf8');
      results.sourceDigest = require('node:crypto').createHash('sha256').update(source).digest('hex');
      await page.evaluate(`globalThis.OfflineAudioContext = (${source})({activated:()=>true, timer:(delay, callback)=>setTimeout(callback,delay), clearTimer:clearTimeout}).OfflineAudioContext`);
    }
    for (const file of ['pcm.js','queue.js','lifecycle.js','lifecycle.js','lifecycle.js']) {
      const start = performance.now();
      const value = await page.evaluate(fs.readFileSync(`${__dirname}/${file}`, 'utf8'));
      results.runs.push({file, ms: Math.round(performance.now()-start), value});
    }
    console.log(JSON.stringify(results, null, 2));
  } finally { await browser.close(); }
})().catch(error => {console.error(error); process.exitCode = 1;});
