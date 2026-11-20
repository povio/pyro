import assert from 'node:assert/strict';
import fs from 'node:fs';
const {OfflineAudioContext,DelayNode}=eval(fs.readFileSync('crates/pyro-webapi/src/webaudio.js','utf8'))({});
const sr=48000;
function impulse(c,chans=1) {
 const src=c.createBufferSource();src.buffer=c.createBuffer(chans,64,sr);
 for(let i=0;i<chans;i++)src.buffer.getChannelData(i)[0]=1/(i+1);
 return src;
}
const near=(a,b)=>assert(Math.abs(a-b)<1e-6,`${a} differs from ${b}`);
for (const dt of [0,2,2.5,127.5,128,256.5]) {
 const c=new OfflineAudioContext(2,512,sr),dl=new DelayNode(c,{delayTime:dt/sr}),s=impulse(c,2);
 s.connect(dl).connect(c.destination);s.start();const data=await c.startRendering();
 for(let ch=0;ch<2;ch++)for(let i=0;i<512;i++)near(data.getChannelData(ch)[i],Math.max(0,1-Math.abs(i-dt))/(ch+1));
}
const c=new OfflineAudioContext(1,640,sr),dl=c.createDelay(),g=c.createGain(),s=impulse(c);
g.gain.value=.5;s.connect(dl).connect(c.destination);dl.connect(g).connect(dl);s.start();
const fb=(await c.startRendering()).getChannelData(0);
for(let i=0;i<640;i++)near(fb[i],i>0 && i%128===0 ? Math.pow(.5,i/128-1):0);
console.log('Checked six integer/fractional stereo impulse responses and a zero-delay feedback loop over five quanta.');
const disconnected=new OfflineAudioContext(1,512,sr);
const remembered=new DelayNode(disconnected,{delayTime:256/sr});
const one=impulse(disconnected);one.connect(remembered);one.start();
const paused=disconnected.suspend(128/sr);const finished=disconnected.startRendering();
await paused;remembered.connect(disconnected.destination);await disconnected.resume();
const rememberedAudio=(await finished).getChannelData(0);
for(let i=0;i<512;i++)near(rememberedAudio[i],i===256?1:0);
console.log('A suspended/reconnected delay retains previously fed samples.');
