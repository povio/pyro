(async () => {
const F=VideoFrame,C=VideoColorSpace;
const assert={
  ok(b,msg) {if(!b) throw Error(msg || 'assertion failed');},
  equal(a,b) {this.ok(Object.is(a,b),JSON.stringify([a,b]));},
  notEqual(a,b) {this.ok(!Object.is(a,b),'unexpected same object');},
  deepEqual(a,b) {this.equal(JSON.stringify(a),JSON.stringify(b));},
  throws(fn,expected) {try {fn();} catch (e) {this.equal(e.name,expected.name);return;} throw Error('missing exception');}
};
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
const canvas=new OffscreenCanvas(11,3),ctx=canvas.getContext('2d');
ctx.drawImage(clone,0,0);
assert.ok(ctx.getImageData(0,0,1,1).data[3]>0, 'Oriented display image failed to render');
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
})().catch(e => console.log('FAIL', String(e)));
