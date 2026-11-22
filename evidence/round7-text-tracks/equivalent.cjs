const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const files={
 '/a.vtt':'WEBVTT\n\na\n00:00:01.250 --> 00:00:03.750 align:start line:40%\n<v Speaker><b>Hello</b> &amp; <lang fr>salut</lang></v>\n\nb\n00:00:05.000 --> 00:00:06.000\nLater\n',
 '/b.vtt':'WEBVTT\n\nreplacement\n00:00:07.500 --> 00:00:09.125\nSecond caption\n'};
const requests=[];
const server=http.createServer((req,res)=>{const item=files[req.url];if(item){requests.push(req.url);res.setHeader('Content-Type','text/vtt; charset=utf-8');res.setHeader('Cache-Control','no-store');return res.end(item)}res.setHeader('Content-Type','text/html');res.end('<!doctype html><body></body>')});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-audio-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(async()=>{
   const out={available:typeof VTTCue};if(typeof VTTCue!=='function')return out;
   for(const scenario of ['attached','detached']){
     const v=document.createElement('video'),el=document.createElement('track'),events=[];document.body.append(v);el.kind='metadata';el.src='a.vtt';
     el.onload=()=>events.push('load');el.onerror=()=>events.push('error');
     const first=new Promise(r=>el.addEventListener('load',r,{once:true}));v.append(el);el.track.mode='hidden';
     await Promise.race([first,new Promise(r=>setTimeout(r,2500))]);const original=el.track,c=original.cues?.[0];
     if(scenario==='detached')el.remove();
     el.src='./a.vtt';if(scenario==='detached')v.append(el);
     await new Promise(r=>setTimeout(r,250));
     out[scenario]={state:el.readyState,sameTrack:original===el.track,sameCue:!!c&&original.cues?.[0]===c,count:original.cues?.length,firstId:original.cues?.[0]?.id,events,listLength:v.textTracks.length};v.remove();
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-equivalent.json'),JSON.stringify({values,requests},null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
