import {spawn} from 'node:child_process';
import {createServer} from 'node:net';
import {createRequire} from 'node:module';
import fs from 'node:fs';
import path from 'node:path';
const {chromium}=createRequire(path.join(process.env.LP_BENCH_DIR || path.join(process.cwd(),'bench'),'package.json'))('playwright-core');
const [label,bin]=process.argv.slice(2),server=createServer();
await new Promise(r=>server.listen(0,'127.0.0.1',r));const port=server.address().port;await new Promise(r=>server.close(r));
const state=path.join(import.meta.dir,'workload-state-'+label);fs.mkdirSync(state,{recursive:true});
const proc=spawn(bin,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{stdio:'ignore',detached:true,env:{...process.env,WPT_CHROME_STATE_DIR:state,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
const ep='http://127.0.0.1:'+port; let browser;
try {
 for(let i=0;i<300;i++){try{if((await fetch(ep+'/json/version')).ok)break}catch{}await Bun.sleep(20)}
 browser=await chromium.connectOverCDP(ep);const page=await browser.newPage();await page.setContent('<title>Offline impulse-response workload</title>');
 const observations=[];
 for(const length of [257,32768]) for(let iteration=0;iteration<6;iteration++){
  const start=performance.now();
  const result=await page.evaluate(async length=>{
   const rate=48000, frames=48000,ctx=new OfflineAudioContext(2,frames,rate), impulse=ctx.createBuffer(2,length,rate), audio=ctx.createBuffer(2,frames,rate);
   let seed=28175854;
   function noise(){seed=(Math.imul(1664525,seed)+1013904223)>>>0;return seed/2147483648-1}
   const inputs=[],kernels=[];
   for(let c=0;c<2;c++){const x=audio.getChannelData(c),h=impulse.getChannelData(c);for(let i=0;i<frames;i++)x[i]=noise()*.25;
    for(let i=0;i<length;i++)h[i]=noise()*Math.exp(-i/(length/6))*.003;h[0]+=.2;
    inputs.push(x.slice()); kernels.push(h.slice());}
   const src=ctx.createBufferSource();src.buffer=audio;
   const begin=performance.now();const conv=new ConvolverNode(ctx,{buffer:impulse,disableNormalization:true});
   const setupMs=performance.now()-begin;src.connect(conv).connect(ctx.destination);src.start();const t=performance.now();const output=await ctx.startRendering();
   const renderMs=performance.now()-t;let checksum=0,energy=0,maxError=0;
   for(let c=0;c<2;c++){const v=output.getChannelData(c);for(let i=0;i<v.length;i++){checksum+=v[i]*((i%67)+1);energy+=v[i]*v[i];}
    for(const i of [0,1,127,128,129,255,256,257,4095,4096,10000,32767,47999]){let expected=0;for(let k=0;k<=i&&k<length;k++)expected+=inputs[c][i-k]*kernels[c][k];maxError=Math.max(maxError,Math.abs(v[i]-expected));}}
   if(maxError>1e-4)throw Error('convolution error: '+maxError);
   return {rate,frames,length,setupMs,renderMs,checksum,energy,maxError};
  },length);
  observations.push({...result,iteration,wallMs:performance.now()-start});
 }
 fs.writeFileSync(path.join(import.meta.dir,'workload-'+label+'.json'),JSON.stringify(observations,null,2));
}finally{if(browser)await browser.close().catch(()=>{});try{process.kill(-proc.pid,'SIGKILL')}catch{}}
