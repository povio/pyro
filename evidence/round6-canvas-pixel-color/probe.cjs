const fs=require('fs'), path=require('path'),cp=require('child_process'),net=require('net');
const pw=require('playwright-core');
const [label,bin]=process.argv.slice(2);
(async()=>{
 let browser,proc;
 try {
 if(bin==='chrome')browser=await pw.chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless:true});
 else {
 const server=net.createServer();await new Promise(r=>server.listen(0,'127.0.0.1',r));const port=server.address().port;await new Promise(r=>server.close(r));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{stdio:'ignore'});
 const url='http://127.0.0.1:'+port;
 for(let i=0;i<100;i++){try{if((await fetch(url+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 browser=await pw.chromium.connectOverCDP(url)}
 const page=await browser.newPage();await page.goto('data:text/html,<html><body></body></html>');
 const result=await page.evaluate(async()=>{
 const ctx=new OffscreenCanvas(3,2).getContext('2d',{colorSpace:'srgb'});
 const rgba=(space,x=0)=>Array.from(ctx.getImageData(x,0,1,1,{colorSpace:space}).data);
 ctx.fillStyle='red';ctx.fillRect(0,0,1,1);const red=[rgba('srgb'),rgba('display-p3')];
 const bytes=new Uint8ClampedArray([117,72,40,255,149,96,58,255]);
 const image=new ImageData(bytes,2,1,{colorSpace:'display-p3'});ctx.putImageData(image,0,0,0,0,1,1);
 const p3Input=[rgba('srgb'),rgba('display-p3'),rgba('srgb',1)];
 const bmp=await createImageBitmap(image);ctx.drawImage(bmp,0,0);const bitmap=[rgba('srgb'),rgba('display-p3')];
 const p3ctx=new OffscreenCanvas(1,1).getContext('2d',{colorSpace:'display-p3'});p3ctx.fillStyle='color(display-p3 1 0 0)';p3ctx.fillRect(0,0,1,1);
 const p3Backing={space:p3ctx.getContextAttributes().colorSpace,red:Array.from(p3ctx.getImageData(0,0,1,1,{colorSpace:'display-p3'}).data)};
 const frameOutputs={};
 if(typeof VideoFrame==='function'){
  for(const format of ['RGBA','I420']) {
   const data=format==='RGBA'?new Uint8Array([255,0,0,255, 0,255,0,255, 0,0,255,255, 100,100,100,255]):new Uint8Array([16,63,126,235,128,128]);
   const f=new VideoFrame(data,{format,codedWidth:2,codedHeight:2,timestamp:17});const clone=f.clone();f.close();
   const frame={cloneTimestamp:clone.timestamp,raw:Array.from(new Uint8Array(clone.allocationSize()))};const raw=new Uint8Array(frame.raw.length);
   frame.rawLayout=await clone.copyTo(raw);frame.raw=Array.from(raw);
   for(const colorSpace of ['srgb','display-p3']) {
    const dest=new Uint8Array(16);frame[colorSpace+'Layout']=await clone.copyTo(dest,{format:'RGBA',colorSpace});frame[colorSpace]=Array.from(dest);
   }
   const c=new OffscreenCanvas(2,2).getContext('2d');c.drawImage(clone,0,0);frame.canvas=Array.from(c.getImageData(0,0,2,2).data);clone.close();frameOutputs[format]=frame;
  }
 }
 return {red,p3Input,bitmap,p3Backing,VideoFrame:typeof VideoFrame,frameOutputs};
 });
 fs.writeFileSync(path.resolve(__dirname,label+'.json'),JSON.stringify(result,null,2))
 }finally{if(browser)await browser.close();if(proc)proc.kill()}
})().catch(e=>{console.error(e);process.exit(1)});
