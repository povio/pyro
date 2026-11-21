// Independent pixel vectors and lifecycle checks for the production factory.
import {readFileSync} from 'node:fs';
import assert from 'node:assert/strict';
globalThis.DOMRectReadOnly = class DOMRectReadOnly {
  constructor(x,y,width,height) { Object.assign(this,{x,y,width,height}); }
};
const {VideoFrame:F, VideoColorSpace:C,canvasSource} = (0,eval)(readFileSync(process.argv[2],'utf8'))({maxSide:16384,maxArea:33554432});
async function pixel(f, options, expected, tolerance=0) {
  const out = new Uint8Array(f.allocationSize(options)); await f.copyTo(out, options);
  assert.equal(out.length,expected.length);
  for (let i=0;i<out.length;i++) assert.ok(Math.abs(out[i]-expected[i])<=tolerance, JSON.stringify({i,out:[...out],expected}));
}
const packed = new Uint8Array([255,0,0,13,0,128,0,23]);
const f=new F(packed,{format:'RGBA',codedWidth:2,codedHeight:1,timestamp:4});
packed.fill(0);
await pixel(f,{},[255,0,0,13,0,128,0,23]);
await pixel(f,{format:'BGRA',rect:{x:1,y:0,width:1,height:1}},[0,128,0,23]);
await pixel(f,{format:'RGBA',colorSpace:'display-p3',rect:{x:0,y:0,width:1,height:1}},[234,51,35,13],1);
const limited=new F(new Uint8Array([63,63,63,63,102,240]),{format:'I420',codedWidth:2,codedHeight:2,timestamp:0});
await pixel(limited,{format:'RGBA',rect:{width:1,height:1}},[255,2,0,255],1);
await pixel(limited,{colorSpace:'display-p3'},[63,63,63,63,102,240]);
assert.throws(()=>limited.allocationSize({format:'I420'}),{name:'NotSupportedError'});
const clone=new F(f,{rotation:90,flip:true,displayWidth:11,displayHeight:3});
assert.deepEqual([clone.displayWidth,clone.displayHeight],[11,3]);
assert.deepEqual(canvasSource(clone).slice(2),[1,2,11,3]);
f.close();
await pixel(clone,{},[255,0,0,13,0,128,0,23]);
assert.equal(f.colorSpace.primaries,'bt709');
assert.notEqual(clone.visibleRect,clone.visibleRect);
assert.throws(()=>new C(1),{name:'TypeError'});
const transfer=new ArrayBuffer(4);
assert.throws(()=>new F(transfer,{format:'RGBX',codedWidth:1,codedHeight:1,timestamp:0,transfer:[transfer,{}]}),{name:'TypeError'});
assert.equal(transfer.byteLength,4);
const taken=new F(transfer,{format:'RGBX',codedWidth:1,codedHeight:1,timestamp:0,transfer:[transfer]});
assert.equal(transfer.byteLength,0); await pixel(taken,{},[0,0,0,255]);
console.log('Pixel, layout, clone, close, transfer, and color space checks passed.');
