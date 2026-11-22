const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const describe="async()=>{const m=navigator.storageBuckets;const result=[typeof StorageBucketManager,typeof StorageBucket,!!m];if(!m)return result;const b=await m.open('shared');result.push((await m.keys()).includes('shared'),typeof b.persist,b.caches instanceof CacheStorage,b.indexedDB instanceof IDBFactory,(await b.getDirectory()) instanceof FileSystemDirectoryHandle);result.push(await (await b.caches.match('message')).text());return result}";
const routes={
 '/dedicated.js':"onmessage=async()=>postMessage(await ("+describe+")())",
 '/nested.js':"onmessage=()=>{const w=new Worker('/dedicated.js');w.onmessage=e=>{postMessage(e.data);w.terminate()};w.postMessage(1)}",
 '/shared.js':"onconnect=e=>{const p=e.ports[0];p.onmessage=async()=>p.postMessage(await ("+describe+")());p.start()}",
 '/sw.js':"oninstall=e=>e.waitUntil(skipWaiting());onactivate=e=>e.waitUntil(clients.claim());onmessage=e=>e.waitUntil(("+describe+")().then(v=>e.ports[0].postMessage(v)))"
};
const server=http.createServer((req,res)=>{const js=routes[req.url];res.setHeader('Content-Type',js?'text/javascript':'text/html');res.end(js||'<!doctype html><body></body>')});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-svg-fixture-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(async()=>{
   const out={surface:!!navigator.storageBuckets};if(!out.surface)return out;
   const bucket=await navigator.storageBuckets.open('shared');await (await bucket.caches.open('data')).put('message',new Response('shared value'));
   const reply=(p,fn)=>new Promise((resolve,reject)=>{const t=setTimeout(()=>reject(Error('timeout')),4500);p.onmessage=e=>{clearTimeout(t);resolve(e.data)};fn()});
   try{
     for(const flavor of ['dedicated','shared','service']){
       try{
         if(flavor==='service'){
           const reg=await navigator.serviceWorker.register('/sw.js');
           try{const active=await Promise.race([navigator.serviceWorker.ready,new Promise((_,reject)=>setTimeout(()=>reject(Error('ready-timeout')),4500))]);
             const ch=new MessageChannel();out[flavor]=await reply(ch.port1,()=>active.active.postMessage(true,[ch.port2]));ch.port1.close();
           }finally{await reg.unregister()}
         }else if(flavor==='shared'){
           const w=new SharedWorker('/shared.js');out[flavor]=await reply(w.port,()=>{w.port.start();w.port.postMessage(true)});w.port.close();
         }else{
           const w=new Worker('/'+flavor+'.js');try{out[flavor]=await reply(w,()=>w.postMessage(1))}finally{w.terminate()}
         }
       }catch(e){out[flavor]={error:e.name,detail:e.message}}
     }
   }finally{await navigator.storageBuckets.delete('shared')}
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-workers.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
