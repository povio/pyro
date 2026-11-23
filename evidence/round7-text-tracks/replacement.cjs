const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const requests=[];let arrived=false,released=false,watch=[],delayed=null;
const send=(res,txt)=>{res.setHeader('Content-Type','text/vtt');res.end('WEBVTT\n\n'+txt+'\n00:00:01.000 --> 00:00:02.000\n'+txt+'\n')};
const server=http.createServer((req,res)=>{switch(req.url){
 case '/slow.vtt':requests.push('old');arrived=true;delayed=res;for(const fn of watch)fn();watch=[];if(released)send(res,'old');return;
 case '/started':if(arrived)return res.end('yes');watch.push(()=>res.end('yes'));return;
 case '/release':released=true;if(delayed&&!delayed.writableEnded)send(delayed,'old');return res.end('released');
 case '/new.vtt':requests.push('new');return send(res,'new');
 default:res.setHeader('Content-Type','text/html');return res.end('<!doctype html><body></body>');
}});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-audio-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(async()=>{
   const out={available:typeof VTTCue};if(typeof VTTCue!=='function')return out;
   const video=document.createElement('video'),el=document.createElement('track'),events=[];document.body.append(video);el.kind='metadata';el.src='/slow.vtt';video.append(el);const t=el.track;t.mode='hidden';
   el.addEventListener('load',()=>events.push(['load',t.cues?.getCueById('new')?.text??null]));el.addEventListener('error',()=>events.push(['error']));
   const timeout=()=>new Promise(r=>setTimeout(()=>r('timeout'),3000));
   const started=await Promise.race([fetch('/started').then(r=>r.text()),timeout()]);out.started=started;
   if(started==='yes'){
     const done=new Promise(r=>el.addEventListener('load',()=>r('loaded'),{once:true}));el.src='/new.vtt';
     out.new=await Promise.race([done,timeout()]);await fetch('/release');await new Promise(r=>setTimeout(r,100));
     out.final=[el.track===t,el.readyState,t.cues?.length,t.cues?.[0]?.text];out.events=events;
   }
   video.remove();
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-replacement.json'),JSON.stringify({values,requests},null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
