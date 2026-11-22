const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const fixtures=path.join(w,'../wptci/wpt-full/webcodecs');
const server=http.createServer((req,res)=>{
 const name=req.url.slice(1);
 if(['four-colors.png','animated-zero-delay.gif','four-colors.jpg'].includes(name)){
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
   const out={available:typeof ImageDecoder};
   if(typeof ImageDecoder==='function'){
     out.types={};for(const mime of ['IMAGE/PNG','image/png; profile=default','text/plain','image/example']){
       try{out.types[mime]=await ImageDecoder.isTypeSupported(mime)}catch(e){out.types[mime]=e.name}
     }
     for(const [file,type] of [['four-colors.png','IMAGE/PNG'],['four-colors.jpg','image/jpeg'],['animated-zero-delay.gif','image/gif']]){
       const bytes=await (await fetch('/'+file)).arrayBuffer();
       const init=file.endsWith('.png')?{data:bytes,type,desiredWidth:16,desiredHeight:12}:{data:bytes,type};
       const decoder=new ImageDecoder(init);await decoder.tracks.ready;
       const tracks=decoder.tracks,t=tracks.selectedTrack,record={type:decoder.type,frames:[]};
       for(let i=0;i<t.frameCount;i++){const {image}=await decoder.decode({frameIndex:i});
         record.frames.push([image.codedWidth,image.codedHeight,image.displayWidth,image.displayHeight,image.rotation,image.flip,image.timestamp,image.duration]);image.close();}
       decoder.close();record.closed=[t.animated,t.frameCount,String(t.repetitionCount),t.selected,tracks.length];out[file]=record;
     }
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-metadata.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
