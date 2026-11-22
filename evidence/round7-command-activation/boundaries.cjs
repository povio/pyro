const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const markup='<!doctype html><body><button type="button" commandfor="invokee" command="--preview" id="invoker">go</button><div id="invokee"></div><script>window.parsedLookup=[typeof invokee,window.invokee===document.getElementById("invokee")];</script>';
const server=http.createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end(markup)});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-audio-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(async()=>{
   const result={};const host=document.createElement('section');const peer=document.createElement('aside');document.body.append(host,peer);const sr=host.attachShadow({mode:'open'});const s=document.createElement('button'),t=document.createElement('div');sr.append(s,t);
   for(const kind of ['command','toggle']){
     const C=kind==='command'?CommandEvent:ToggleEvent;if(typeof C!=='function')continue;
     const cases={};for(const [label,src] of [['inside',s],['same',t],['outside',host],['outsidePeer',peer],['null',null]]){
       const hits=[];const listeners=[];for(const [id,target] of [['host',host],['root',sr],['target',t]]){const cb=e=>{hits.push([id,e.source===host?'host':e.source===s?'inside':e.source===t?'target':e.source===null?'null':'other',e.composedPath().includes(host)]);};target.addEventListener(kind,cb,true);listeners.push([target,cb]);}
       t.dispatchEvent(new C(kind,{source:src,composed:true}));for(const [target,cb] of listeners)target.removeEventListener(kind,cb,true);cases[label]=hits;
     }result[kind]=cases;
   }
   const out=result;
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-boundaries.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
