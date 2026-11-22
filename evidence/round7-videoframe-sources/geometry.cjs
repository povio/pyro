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
   const out={};const canvas=new OffscreenCanvas(4,2),ctx=canvas.getContext('2d');
   ctx.fillStyle='rgb(0, 255, 0)';ctx.fillRect(2,0,2,2);
   for(const alpha of ['keep','discard']){
     try{
       const f=new VideoFrame(canvas,{timestamp:-4,duration:32,alpha,visibleRect:{x:1,y:0,width:3,height:2},displayWidth:6,displayHeight:4,rotation:90,flip:true});
       const rect=f.visibleRect,bytes=new Uint8Array(f.allocationSize({format:'RGBA'}));await f.copyTo(bytes,{format:'RGBA'});
       out[alpha]={coded:[f.codedWidth,f.codedHeight],visible:[rect.x,rect.y,rect.width,rect.height],display:[f.displayWidth,f.displayHeight],metadata:[f.timestamp,f.duration,f.rotation,f.flip],colors:[[...bytes.slice(0,4)],[...bytes.slice(4,8)]]};f.close();
     }catch(e){out[alpha]={error:e.name}}
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-geometry.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
