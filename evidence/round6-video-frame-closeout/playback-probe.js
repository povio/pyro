(async () => {
  const width=640,height=360, bytes=new Uint8Array(width*height*3/2);
  bytes.fill(126,0,width*height);bytes.fill(108,width*height,width*height*5/4);bytes.fill(155,width*height*5/4);
  const frame=new VideoFrame(bytes,{format:'I420',codedWidth:width,codedHeight:height,timestamp:0});
  const packed=new Uint8Array(width*height*4), ctx=new OffscreenCanvas(width,height).getContext('2d');
  const samples=[];
  for(let i=0;i<5;i++) {
    const start=performance.now();
    await frame.copyTo(packed,{format:'RGBA'});
    const middle=performance.now();
    ctx.drawImage(frame,0,0);
    const pixel=[...ctx.getImageData(width/2,height/2,1,1).data];
    const end=performance.now();
    if (pixel.some((v,k)=>Math.abs(v-packed[k])>1)) throw Error('readback mismatch');
    samples.push({copyMs:middle-start,drawAndReadbackMs:end-middle,pixel});
  }
  console.log(JSON.stringify(samples));frame.close();
})().catch(e=>console.log('FAIL',String(e)));
