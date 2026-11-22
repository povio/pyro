const fs = require('node:fs');
const crypto = require('node:crypto');
const { chromium } = require('/private/tmp/pyro-r7-param/bench/node_modules/playwright-core');
const hash = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
async function run(){
  const source = fs.readFileSync(process.argv[2]);
  const browser = await chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',chromiumSandbox:true});
  try {
    const result = {chrome:browser.version(),sourceSHA:hash(source),scriptSHA:hash(fs.readFileSync(__filename))};
    for(const mode of ['native','candidate']){
      const page = await browser.newPage();
      if(mode==='candidate') await page.evaluate(`Object.assign(globalThis, (${source})({activated:()=>false})); void 0`);
      result[mode]=await page.evaluate(async()=>{
        const rows=[];
        for(const rate of [44100,48000]){
          const ctx=new OfflineAudioContext(2,rate,rate), node=ctx.createDynamicsCompressor(), buffer=ctx.createBuffer(2,rate,rate);
          buffer.getChannelData(0).fill(0.01); buffer.getChannelData(0).fill(0.8,0,rate/2); buffer.getChannelData(1).fill(-0.01);
          const signal=ctx.createBufferSource(); signal.buffer=buffer; signal.connect(node).connect(ctx.destination); signal.start();
          const before=node.reduction, observations=[];
          const pauses=[0.45,0.51,0.65,0.95].map(time=>ctx.suspend(Math.round(time*rate/128)*128/rate));
          const render=ctx.startRendering();
          for(const pause of pauses){await pause; observations.push({frame:Math.round(ctx.currentTime*rate),db:node.reduction}); await ctx.resume();}
          await render; rows.push({rate,before,observations,after:node.reduction});
        }
        return rows;
      }); await page.close();
    }
    const data=JSON.stringify(result,null,2); fs.writeFileSync(__dirname+'/meters-'+result.sourceSHA+'.json',data+'\n',{flag:'wx',mode:0o600}); console.log(data);
  } finally { await browser.close(); }
}
run().catch(error=>{console.error(error);process.exitCode=1});
