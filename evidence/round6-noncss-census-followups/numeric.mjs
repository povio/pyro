import {readFileSync} from 'node:fs';
import assert from 'node:assert/strict';
const {OfflineAudioContext, StereoPannerNode} = eval(readFileSync('crates/pyro-webapi/src/webaudio.js','utf8'))({});
const c = new OfflineAudioContext(2, 128, 44100);
assert.throws(() => new StereoPannerNode(c, {pan:1e100}), TypeError);
for (const value of [-1,0,1]) {
 for (const channels of [1,2]) {
  const ctx = new OfflineAudioContext(2, 128, 44100);
  const input = ctx.createBufferSource();
  input.buffer = ctx.createBuffer(channels,128,44100);
  input.buffer.getChannelData(0).fill(1);
  if(channels===2) input.buffer.getChannelData(1).fill(.5);
  const pan = new StereoPannerNode(ctx,{pan:value});
  input.connect(pan).connect(ctx.destination);input.start();
  const buffer = await ctx.startRendering();
  const actual = [0,1].map(ch=>buffer.getChannelData(ch)[3]);
  const wanted = channels===1 ? [[1,0],[Math.SQRT1_2,Math.SQRT1_2],[0,1]][value+1] : [[1.5,0],[1,.5],[0,1.5]][value+1];
  assert(actual.every((v,i)=>Math.abs(v-wanted[i])<1e-6));
 }
}
console.log('Restricted float conversion and six mono/stereo equal-power renders passed.');
const source=c.createConstantSource();
const mod=c.createBufferSource();
mod.buffer=c.createBuffer(1,128,44100);
mod.buffer.getChannelData(0).fill(NaN);
const panner=c.createStereoPanner();
mod.connect(panner.pan);
source.connect(panner).connect(c.destination);
source.start();mod.start();
const result=await c.startRendering();
for (const ch of [0,1]) assert(Math.abs(result.getChannelData(ch)[7]-Math.SQRT1_2)<1e-6);
console.log('NaN input modulation yields the default pan.');
