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
   const out={available:typeof ImageDecoder};
   if(typeof ImageDecoder==='function'){
     const bytes=new Uint8Array(await (await fetch('/four-colors.png')).arrayBuffer());
     const source=new ReadableStream({start(c){c.enqueue(bytes.subarray(0,24));c.enqueue(bytes.subarray(24));c.close()}});
     const decoder=new ImageDecoder({type:'image/png',data:source});await decoder.completed;await decoder.tracks.ready;
     const track=decoder.tracks.selectedTrack;
     out.complete=decoder.complete;out.locked=source.locked;out.selectedBefore=[track.selected,decoder.tracks.selectedIndex];
     track.selected=false;out.deselected=[track.selected,decoder.tracks.selectedIndex,decoder.tracks.selectedTrack===null];
     track.selected=true;const one=await decoder.decode();out.first=[one.complete,one.image.displayWidth,one.image.displayHeight];one.image.close();
     decoder.reset();const two=await decoder.decode();out.again=[two.complete,two.image.displayWidth,two.image.displayHeight];two.image.close();decoder.close();
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-lifecycle.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
