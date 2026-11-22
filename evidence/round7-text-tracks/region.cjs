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
   const out={cue:typeof VTTCue,region:typeof VTTRegion};
   if(typeof VTTCue==='function'){
     const cue=new VTTCue(1.25,5.5,'A <00:00:02.500><ruby>字<rt>ji</rt></ruby> &amp; B');
     const f=cue.getCueAsHTML(),fresh=cue.getCueAsHTML();
     const nodes=n=>[n.nodeType,n.nodeName,n.nodeValue,[...n.childNodes].map(nodes)];
     out.fragment=nodes(f);out.fresh=f!==fresh;
     if(typeof VTTRegion==='function'){
       const reg=new VTTRegion();reg.id='lower';reg.lines=4;reg.width=50;reg.regionAnchorX=20;reg.regionAnchorY=100;reg.viewportAnchorX=30;reg.viewportAnchorY=90;reg.scroll='up';cue.region=reg;
       out.regionFields=[reg.id,reg.lines,reg.width,reg.regionAnchorX,reg.regionAnchorY,reg.viewportAnchorX,reg.viewportAnchorY,reg.scroll,cue.region===reg];
     }
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-region.json'),JSON.stringify({values,requests},null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
