const fs = require('node:fs');
const path = require('node:path');
const net = require('node:net');
const cp = require('node:child_process');
const pw = require(path.resolve(__dirname,'../../manifest-runner/bench/node_modules/playwright-core'));
const [label, bin] = process.argv.slice(2);
(async()=>{
 let browser, proc;
 try {
  if(bin==='chrome') browser=await pw.chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless:true});
  else {
   const server=net.createServer();await new Promise(r=>server.listen(0,'127.0.0.1',r));const port=server.address().port;await new Promise(r=>server.close(r));
   proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{stdio:'ignore'});
   const url='http://127.0.0.1:'+port;
   for (let i=0;i<120;i++) {try {if ((await fetch(url+'/json/version')).ok) break;}catch {} await new Promise(r=>setTimeout(r,100));}
   browser=await pw.chromium.connectOverCDP(url);
  }
  const page=await browser.newPage();
  const data=await page.evaluate(async()=>{
   const render=async(opts,pos,orient,stereo=false)=>{
    const c=new OfflineAudioContext(2,256,48000),source=c.createBufferSource();
    source.buffer=c.createBuffer(stereo?2:1,256,48000);source.buffer.getChannelData(0).fill(1);if(stereo)source.buffer.getChannelData(1).fill(.5);
    const n=new PannerNode(c,opts);if(pos)c.listener.setPosition(...pos);if(orient)c.listener.setOrientation(...orient);
    source.connect(n).connect(c.destination);source.start();const b=await c.startRendering();
    return [b.getChannelData(0)[129],b.getChannelData(1)[129]];
   };
   const c=new OfflineAudioContext(2,128,48000);const n=c.createPanner();
   n.positionZ.setValueCurveAtTime([0,1],0,0.5);let error=null;
   try {n.setPosition(1,2,3);}catch(e){error=e.name;}
   const out={};for(const [key,opts,pos,ori,st] of [
    ['left',{positionX:-2}],['right',{positionX:2}],['rotated',{positionZ:-1},null,[1,0,0,0,1,0]],
    ['back',{positionZ:3}],['linear',{positionX:3,distanceModel:'linear',maxDistance:5}],
    ['exponential',{positionZ:3,distanceModel:'exponential',rolloffFactor:2}],
    ['outsideCone',{positionZ:-3,orientationX:0,orientationZ:-1,coneInnerAngle:20,coneOuterAngle:40,coneOuterGain:0.25}],
    ['coincidentCone',{coneInnerAngle:0,coneOuterAngle:0,coneOuterGain:0}],
    ['stereoLeft',{positionX:-1},null,null,true],['stereoCenter',{},null,null,true],
   ])out[key]=await render(opts,pos,ori,st);
   return {renders:out,curveSetter:{error,x:n.positionX.value,y:n.positionY.value}};
  });
  fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify(data,null,2));
  console.log(label,JSON.stringify(data));
 } finally {if(browser)await browser.close();if(proc)proc.kill();}
})().catch(e=>{console.error(e);process.exitCode=1;});
