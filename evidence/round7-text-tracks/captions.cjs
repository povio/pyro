const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const files={
 '/a.vtt':'WEBVTT\n\na\n00:00:01.250 --> 00:00:03.750 align:start line:40%\n<v Speaker><b>Hello</b> &amp; <lang fr>salut</lang></v>\n\nb\n00:00:05.000 --> 00:00:06.000\nLater\n',
 '/b.vtt':'WEBVTT\n\nreplacement\n00:00:07.500 --> 00:00:09.125\nSecond caption\n'};
const requests=[];
const server=http.createServer((req,res)=>{const item=files[req.url];if(item){requests.push(req.url);res.setHeader('Content-Type','text/vtt; charset=utf-8');return res.end(item)}res.setHeader('Content-Type','text/html');res.end('<!doctype html><body></body>')});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-audio-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(async()=>{
   const out={available:[typeof VTTCue,typeof TextTrack,typeof TextTrackCueList,typeof VTTRegion]};
   const video=document.createElement('video');document.body.appendChild(video);
   if(typeof video.addTextTrack!=='function'||typeof VTTCue!=='function')return out;
   const list=video.textTracks,track=video.addTextTrack('captions','Program','en');
   out.program=[list===video.textTracks,list.length,track===list[0],track.kind,track.label,track.language,track.mode];
   track.mode='hidden';const late=new VTTCue(8,9,'late'),early=new VTTCue(1,3,'<v Alice><b>Hi</b> &amp; <lang fr>salut</lang></v>');early.id='early';track.addCue(late);track.addCue(early);
   const frag=early.getCueAsHTML(),fresh=early.getCueAsHTML();out.cues=[track.cues.length,track.cues[0]===early,track.cues.getCueById('early')===early,early.track===track,frag!==fresh,frag.textContent,frag.firstChild?.getAttribute('title'),frag.querySelector('b')?.textContent,frag.querySelector('[lang]')?.getAttribute('lang')];
   frag.appendChild(document.createTextNode('changed'));out.detached=fresh.textContent;track.removeCue(early);out.removed=[track.cues.length,early.track===null];
   const events=[];const el=document.createElement('track');el.kind='metadata';el.label='File';list.addEventListener('addtrack',e=>events.push(['addtrack',e.track===el.track]));list.addEventListener('removetrack',e=>events.push(['removetrack',e.track===el.track]));
   const loaded=()=>new Promise(resolve=>{const t=setTimeout(()=>resolve('timeout'),2500);el.onload=()=>{clearTimeout(t);resolve('load')};el.onerror=()=>{clearTimeout(t);resolve('error')}});
   let ready=loaded();el.src='/a.vtt';video.appendChild(el);el.track.mode='hidden';out.firstEvent=await ready;const fileTrack=el.track;
   const collect=()=>({state:el.readyState,identity:[fileTrack===el.track,fileTrack===list[0]],cues:fileTrack.cues?[...fileTrack.cues].map(c=>[c.id,c.startTime,c.endTime,c.text,c.getCueAsHTML().textContent]):null});
   out.first=collect();ready=loaded();el.src='/b.vtt';out.secondEvent=await ready;out.second=collect();el.remove();await new Promise(r=>setTimeout(r,20));out.events=events;out.finalLength=list.length;video.remove();
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-captions.json'),JSON.stringify({values,requests},null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
