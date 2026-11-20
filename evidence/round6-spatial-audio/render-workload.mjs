import fs from 'node:fs';
const audio=eval(fs.readFileSync(process.argv[2]||'crates/pyro-webapi/src/webaudio.js','utf8'))({});
for (const automated of [false,true]) {
 const times=[];let checksum;
 for(let sample=0;sample<5;sample++){
  const c=new audio.OfflineAudioContext(2,48000*2,48000);
  for(let j=0;j<8;j++){
   const source=c.createConstantSource();source.offset.value=(j+1)/100;
   const p=new audio.PannerNode(c,{positionX:j-3,positionZ:-3});
   if(automated)p.positionX.setValueAtTime(j-3,0).linearRampToValueAtTime(3-j,2);
   source.connect(p).connect(c.destination);source.start();
  }
  const start=performance.now(),b=await c.startRendering();times.push(Math.round(performance.now()-start));
  let out=0;for(const ch of [0,1]){const a=b.getChannelData(ch);for(let i=0;i<a.length;i+=61)out+=a[i]*(1+ch);}
  if(sample===0)checksum=out;if(Math.abs(out-checksum)>1e-6)throw Error('unstable output');
 }
 console.log(JSON.stringify({automated,times,checksum}));
}
