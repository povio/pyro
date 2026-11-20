const fs = require('node:fs');
const path = require('node:path');
const net = require('node:net');
const cp = require('node:child_process');
const pw = require(path.resolve(__dirname,'../../manifest-runner/bench/node_modules/playwright-core'));
const [label, bin] = process.argv.slice(2);
(async()=>{
 let browser, proc;
 try {
  if(bin==='chrome') browser=await pw.chromium.launch({executablePath:path.resolve(process.env.HOME,'Library/Caches/lightningpanda/round6-reference-158/Google Chrome.app/Contents/MacOS/Google Chrome'),headless:true});
  else {
   const server=net.createServer();await new Promise(r=>server.listen(0,'127.0.0.1',r));const port=server.address().port;await new Promise(r=>server.close(r));
   proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{stdio:'ignore'});
   const url='http://127.0.0.1:'+port;
   for (let i=0;i<120;i++) {try {if ((await fetch(url+'/json/version')).ok) break;}catch {} await new Promise(r=>setTimeout(r,100));}
   browser=await pw.chromium.connectOverCDP(url);
  }
  const page=await browser.newPage();
  const data=await page.evaluate(async()=>{
   const rate=48000, out={};
   function impulse(c,n=1) {
    const s=c.createBufferSource(); s.buffer=c.createBuffer(n,64,rate);
    for(let ch=0;ch<n;ch++)s.buffer.getChannelData(ch)[0]=1/(ch+1);
    return s;
   }
   for(const delayFrames of [0,2.5,127.5,128,256.5]) {
    const c=new OfflineAudioContext(2,512,rate),src=impulse(c,2),delay=new DelayNode(c,{delayTime:delayFrames/rate});
    src.connect(delay).connect(c.destination);src.start();
    const b=await c.startRendering();out['impulse-'+delayFrames]=[0,1].map(ch=>Array.from(b.getChannelData(ch)).flatMap((v,f)=>Math.abs(v)>1e-6?[[f,v]]:[]));
   }
   {
    const c=new OfflineAudioContext(1,640,rate),s=impulse(c),dl=c.createDelay(),g=c.createGain();g.gain.value=.5;
    s.connect(dl).connect(c.destination);dl.connect(g).connect(dl);s.start();
    const a=(await c.startRendering()).getChannelData(0);out.feedback=Array.from(a).flatMap((v,i)=>Math.abs(v)>1e-6?[[i,v]]:[]);
   }
   {
    const c=new OfflineAudioContext(1,512,rate),s=impulse(c),dl=new DelayNode(c,{delayTime:256/rate});s.connect(dl);s.start();
    const pause=c.suspend(128/rate),done=c.startRendering();await pause;dl.connect(c.destination);await c.resume();
    out.reconnected=(await done).getChannelData(0)[256];
   }
   {
    const c=new OfflineAudioContext(2,512,rate),src=c.createBufferSource(),dl=new DelayNode(c,{delayTime:128/rate,channelCount:2,channelCountMode:'explicit'});
    src.buffer=c.createBuffer(2,512,rate);src.buffer.getChannelData(0).fill(.25);src.buffer.getChannelData(1).fill(.75);
    const sp=c.createChannelSplitter(2),merger=c.createChannelMerger(2);
    src.connect(dl).connect(sp);sp.connect(merger,0,0);sp.connect(merger,1,1);merger.connect(c.destination);src.start();
    const pause=c.suspend(128/rate),done=c.startRendering();await pause;dl.channelCount=1;await c.resume();
    const b=await done;out.layout=[0,1].map(c=>[64,192,320,448].map(i=>b.getChannelData(c)[i]));
   }
   return out;
  });
  fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify(data,null,2));
  console.log(label,JSON.stringify(data));
 } finally {if(browser)await browser.close();if(proc)proc.kill();}
})().catch(e=>{console.error(e);process.exitCode=1;});
