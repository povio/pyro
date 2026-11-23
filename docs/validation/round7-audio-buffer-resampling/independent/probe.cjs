const fs = require('node:fs');
const assert = require('node:assert/strict');
const {chromium} = require('/private/tmp/pyro-r7-param/bench/node_modules/playwright-core');
(async () => {
  assert.equal(require('node:crypto').createHash('sha256').update(fs.readFileSync('candidate.js')).digest('hex'),'348c96efc5355df4bf26fb6ff11a92b65617c58484a721d15f60e4ea22c652a6');
  const browser = await chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',pipe:true,timeout:15000});
  try {
    console.log(browser.version());
    for (const mode of ['native','baseline','candidate']) {
      const page = await browser.newPage();
      if (mode !== 'native') await page.evaluate(source => {globalThis.probeAudio = (0,eval)('('+source+'\n)')({activated:()=>false,timer:(delay,fn)=>setTimeout(fn,delay),clearTimer:clearTimeout});},fs.readFileSync(mode+'.js','utf8'));
      const results = await page.evaluate(async () => {
        const rows = [];
        for(const length of [1,4]) for(const bufferRate of [24000,48000]) {
          const Context = (globalThis.probeAudio || globalThis).OfflineAudioContext;
          const context = new Context(2,12,48000), source = context.createBufferSource();
          source.buffer = context.createBuffer(2,length,bufferRate);
          source.buffer.getChannelData(0).set([0.125,0.25,0.375,0.5].slice(0,length));
          source.buffer.getChannelData(1).fill(-0.25);
          source.connect(context.destination); source.start(2/48000);
          const start = performance.now(); const output = await context.startRendering();
          rows.push({length,bufferRate,renderMs:performance.now()-start,contextTime:context.currentTime,pcm:[0,1].map(channel=>Array.from(output.getChannelData(channel)))});
        } return rows;
      }); console.log(mode,JSON.stringify(results)); await page.close();
    }
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
