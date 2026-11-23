const fs = require('node:fs');
const vm = require('node:vm');
const {chromium} = require('/tmp/pyro-r7-param/bench/node_modules/playwright-core');
async function suite(Offline) {
  const rate = 8192, result = {};
  for (const method of ['set','linear','exp','target','curve','cancel','hold']) {
    result[method] = {};
    for (const [label,frame] of Object.entries({past:32,now:384,future:448,invalid:-1})) {
      const ctx = new Offline(1,896,rate), src = ctx.createConstantSource(), param = src.offset;
      src.connect(ctx.destination); src.start();
      param.setValueAtTime(2,128/rate);
      if (method === 'hold') param.linearRampToValueAtTime(8,640/rate);
      if (method === 'cancel') param.setValueAtTime(8,416/rate);
      const pause = ctx.suspend(384/rate), render = ctx.startRendering(); await pause;
      if (ctx.currentTime * rate !== 384) throw Error('unexpected pause');
      const stamp = label === 'now' ? ctx.currentTime : frame/rate;
      let error = null;
      try {
        ({set:()=>param.setValueAtTime(5,stamp),linear:()=>param.linearRampToValueAtTime(5,stamp),exp:()=>param.exponentialRampToValueAtTime(5,stamp),target:()=>param.setTargetAtTime(5,stamp,72/rate),curve:()=>param.setValueCurveAtTime([3,6,4],stamp,64/rate),cancel:()=>param.cancelScheduledValues(stamp),hold:()=>param.cancelAndHoldAtTime(stamp)})[method]();
      } catch (caught) { error = caught.name; }
      await ctx.resume(); result[method][label] = {error,pcm:Array.from((await render).getChannelData(0))};
    }
  }
  return result;
}
(async()=>{
  const browser = await chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless:true});
  let native;
  try { const page=await browser.newPage(); native=await page.evaluate(`(${suite})(OfflineAudioContext)`); console.log('Chrome',browser.version()); } finally { await browser.close(); }
  for (const label of ['native','base','candidate']) {
    const data = label==='native' ? native : await suite(vm.runInNewContext(fs.readFileSync(label+'.js','utf8'),{EventTarget,Event,DOMException,performance,queueMicrotask,setTimeout,structuredClone})({activated:()=>true}).OfflineAudioContext);
    fs.writeFileSync(label+'-pcm.json',JSON.stringify(data));
    for (const [method,cases] of Object.entries(data)) {
      const failures=[];
      for (const [key,got] of Object.entries(cases)) {
        const expected=native[method][key], frame=got.pcm.findIndex((v,i)=>!(Math.abs(v-expected.pcm[i])<=2e-5));
        if(frame!==-1||got.error!==expected.error) failures.push({key,frame,got:got.pcm[frame],expected:expected.pcm[frame],error:got.error,expectedError:expected.error});
      }
      console.log(label,method,JSON.stringify(failures), 'native past=now:',native[method].past.pcm.every((v,i)=>v===native[method].now.pcm[i]));
    }
  }
})().catch(error=>{console.error(error);process.exitCode=1});
