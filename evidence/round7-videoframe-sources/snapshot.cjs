const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const fixtures=path.join(w,'../wptci/wpt-full/webcodecs');
const server=http.createServer((req,res)=>{
 const name=req.url.slice(1);
 if(['four-colors.png','animated-zero-delay.gif'].includes(name)){
   res.setHeader('Content-Type',name.endsWith('.png')?'image/png':'image/gif');return res.end(fs.readFileSync(path.join(fixtures,name)));
 }
 res.setHeader('Content-Type','text/html');res.end('<!doctype html><canvas></canvas>');
});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-audio-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(async()=>{
   const out={};
   const sources=[['dom',document.createElement('canvas')],['offscreen',new OffscreenCanvas(4,2)]];
   for(const [label,canvas] of sources){
     canvas.width=4;canvas.height=2;const ctx=canvas.getContext('2d');
     ctx.fillStyle='red';ctx.fillRect(0,0,4,2);ctx.fillStyle='blue';ctx.fillRect(2,0,2,2);
     const bitmap=await createImageBitmap(canvas);
     for(const [type,source] of [['surface',canvas],['bitmap',bitmap]]){
       const key=label+'-'+type;
       try{
         const frame=new VideoFrame(source,{timestamp:7700,duration:2800});
         if(type==='surface'){ctx.fillStyle='lime';ctx.fillRect(0,0,4,2)}
         const clone=frame.clone();frame.close();
         const dest=document.createElement('canvas');dest.width=4;dest.height=2;const draw=dest.getContext('2d');draw.drawImage(clone,0,0);
         out[key]={size:[clone.codedWidth,clone.codedHeight,clone.displayWidth,clone.displayHeight],timing:[clone.timestamp,clone.duration],colors:[[...draw.getImageData(0,0,1,1).data],[...draw.getImageData(3,0,1,1).data]]};clone.close();
       }catch(e){out[key]={error:e.name}}
     }
     bitmap.close();
   }
   const raw=new VideoFrame(new Uint8Array([5,6,7,255]),{format:'RGBA',codedWidth:1,codedHeight:1,timestamp:-7});const dup=raw.clone();raw.close();const bytes=new Uint8Array(4);await dup.copyTo(bytes);out.raw=[dup.timestamp,dup.format,...bytes];dup.close();
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-snapshot.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
