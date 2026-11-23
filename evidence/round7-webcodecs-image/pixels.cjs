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
   const out={surface:[typeof ImageDecoder,typeof ImageTrackList,typeof ImageTrack]};
   if(typeof ImageDecoder==='function'){
     for(const [file,mime] of [['four-colors.png','image/png'],['animated-zero-delay.gif','image/gif']]){
       const buffer=await (await fetch('/'+file)).arrayBuffer();
       const decoder=new ImageDecoder({type:mime,data:buffer});
       await decoder.tracks.ready;await decoder.completed;
       const tracks=decoder.tracks,track=tracks.selectedTrack;
       const entry={type:decoder.type,complete:decoder.complete,list:[tracks.length,tracks.selectedIndex,track===tracks[0]],track:[track.animated,track.frameCount,String(track.repetitionCount)],frames:[]};
       for(let i=0;i<track.frameCount;i++){
         const {image,complete}=await decoder.decode({frameIndex:i});
         const canvas=document.createElement('canvas');canvas.width=image.displayWidth;canvas.height=image.displayHeight;
         const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0);
         const colors=[];for(const y of [0.25,0.75])for(const x of [0.25,0.75])colors.push([...ctx.getImageData(Math.floor(x*canvas.width),Math.floor(y*canvas.height),1,1).data]);
         entry.frames.push({complete,width:image.codedWidth,height:image.codedHeight,colors});image.close();
       }
       decoder.close();out[file]=entry;
     }
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-pixels.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
