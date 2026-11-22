const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const requests=[];
const media=fs.readFileSync(path.join(w,'source/crates/pyro-media/tests/fixtures/frames-h264.mp4'));
const server=http.createServer((req,res)=>{if(req.url==='/normal.mp4'){requests.push(req.url);res.setHeader('Content-Type','video/mp4');res.end(media)}else{res.setHeader('Content-Type','text/html');res.end('<!doctype html><body></body>')}});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-audio-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(async()=>{
   const out={available:typeof VTTCue};if(typeof VTTCue!=='function')return out;
   const v=document.createElement('video');v.muted=true;v.preload='auto';document.body.append(v);
   const timeout=ms=>new Promise(r=>setTimeout(()=>r('timeout'),ms));
   const meta=new Promise(r=>v.addEventListener('loadedmetadata',()=>r('loadedmetadata'),{once:true}));v.src='/normal.mp4';out.metadata=await Promise.race([meta,timeout(3000)]);if(out.metadata==='timeout')return out;
   const events=[];for(const name of ['A','B']){
     const track=v.addTextTrack('metadata',name),cue=new VTTCue(.20,.55,name);track.mode='hidden';cue.pauseOnExit=name==='A';track.addCue(cue);
     cue.addEventListener('enter',()=>events.push(name+' enter'));cue.addEventListener('exit',()=>events.push(name+' exit'));track.addEventListener('cuechange',()=>events.push(name+' change:'+track.activeCues.length));
   }
   const paused=new Promise(r=>v.addEventListener('pause',()=>r('pause'),{once:true}));
   try{await v.play();out.pause=await Promise.race([paused,timeout(2200)])}catch(e){out.error=e.name}
   await new Promise(r=>setTimeout(r,80));out.events=events;out.paused=v.paused;out.ended=v.ended;out.at=v.currentTime;v.remove();
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-playback.json'),JSON.stringify({values,requests},null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
