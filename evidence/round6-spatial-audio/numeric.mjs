import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
const audio=eval(readFileSync('crates/pyro-webapi/src/webaudio.js','utf8'))({});
const approx=(a,b)=>assert(Math.abs(a-b)<0.00002,`${a} deviates from ${b}`);
async function render(options,configure=()=>{}) {
 const c=new audio.OfflineAudioContext(2,1024,48000),n=new audio.PannerNode(c,options);
 const source=c.createConstantSource();source.connect(n).connect(c.destination);
 configure(c,n);source.start();
 const r=await c.startRendering();return [r.getChannelData(0),r.getChannelData(1)];
}
for(const [opts,expected] of [
 [{positionX:-2},[0.5,0]],[{positionX:2},[0,0.5]],
 [{positionZ:3},[Math.SQRT1_2/3,Math.SQRT1_2/3]],
 [{positionZ:-3,orientationZ:-1,orientationX:0,coneInnerAngle:20,coneOuterAngle:40,coneOuterGain:0.25},[Math.SQRT1_2/12,Math.SQRT1_2/12]],
 [{distanceModel:'linear',positionZ:-3,refDistance:1,maxDistance:5},[Math.SQRT1_2/2,Math.SQRT1_2/2]],
 [{distanceModel:'exponential',positionZ:-3,rolloffFactor:2},[Math.SQRT1_2/9,Math.SQRT1_2/9]],
]) {
 const out=await render(opts);out.forEach((x,i)=>approx(x[257],expected[i]));
}
const rotated=await render({positionZ:-1},c=>c.listener.setOrientation(1,0,0,0,1,0));approx(rotated[0][0],1);approx(rotated[1][0],0);
const moving=await render({positionX:-1},(c,n)=>n.positionX.setValueAtTime(1,512/48000));approx(moving[0][511],1);approx(moving[1][512],1);
console.log('Verified mono geometry, cone and three distance models, listener rotation and sample-aligned automation.');
const coincident = await render({coneInnerAngle:0,coneOuterAngle:0,coneOuterGain:0});approx(coincident[0][0],0);approx(coincident[1][0],0);
const ctx=new audio.OfflineAudioContext(2,128,48000),pn=ctx.createPanner();
pn.positionZ.setValueCurveAtTime([0,1],0,0.5);
assert.throws(()=>pn.setPosition(1,2,3),{name:'NotSupportedError'});assert.equal(pn.positionX.value,0);
console.log('Verified coincident directional source and curve-start setter failure.');
