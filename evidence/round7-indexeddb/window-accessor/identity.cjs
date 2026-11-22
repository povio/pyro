const http=require('http'),net=require('net'),cp=require('child_process'),fs=require('fs'),path=require('path');
const cache=path.resolve(__dirname,'../..'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
const [label,exe]=process.argv.slice(2);const listen=s=>new Promise(resolve=>s.listen(0,'127.0.0.1',()=>resolve(s.address().port)));
(async()=>{const probe=fs.readFileSync(path.join(__dirname,'identity.js'),'utf8');
const server=http.createServer((req,res)=>{
if(req.url==='/')return res.end('<body>');
res.setHeader('Content-Type','application/javascript');
const send=req.url==='/ded.js'?'data.then(x=>postMessage(x))':req.url==='/shared.js'?'onconnect=async e=>e.ports[0].postMessage(await data)':"oninstall=e=>self.skipWaiting(); onactivate=e=>e.waitUntil(clients.claim()); onmessage=async e=>e.source.postMessage(await data)";
res.end(probe+'; const data=probe(); '+send);
}),port=await listen(server);let proc,b;try{
 if(label==='chrome')b=await pw.chromium.launch({executablePath:path.join(cache,'round6-reference-158/Google Chrome.app/Contents/MacOS/Google Chrome'),headless:true});
 else{const sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));proc=cp.spawn(exe,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error'],{stdio:'ignore'});for(let i=0;i<100;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,80));} b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp)}
 const page=await b.newPage();await page.goto('http://127.0.0.1:'+port);
 const descriptor=await page.evaluate(()=>{
 const own=Object.getOwnPropertyDescriptor(globalThis,'indexedDB'), proto=Object.getOwnPropertyDescriptor(Window.prototype,'indexedDB'), d=own||proto;
 const initial=indexedDB;
 function receiver(value){try{return d.get.call(value)===initial}catch(e){return e.name}}
 return {own:!!own,prototype:!!proto,enumerable:d.enumerable,configurable:d.configurable,setterAbsent:d.set===undefined,getterName:d.get?.name,getterLength:d.get?.length,sameFactory:indexedDB===initial,globalReceiver:receiver(globalThis),undefinedReceiver:receiver(undefined),wrongReceiver:receiver({}),assigned:Reflect.set(globalThis,'indexedDB',1),deleted:delete globalThis.indexedDB,presentAfterDelete:'indexedDB' in globalThis};
 });
 await page.close();
 const fresh=await b.newPage();await fresh.goto('http://127.0.0.1:'+port);
 const results={descriptor,window:await fresh.evaluate('('+probe+')()')};
 for(const realm of ['dedicated','shared','service']){
 try{results[realm]=await fresh.evaluate(async realm=>await Promise.race([new Promise(async(resolve,reject)=>{try{
 if(realm==='dedicated'){const w=new Worker('/ded.js');w.onmessage=e=>resolve(e.data);w.onerror=e=>reject(Error(e.message))}
 if(realm==='shared'){const w=new SharedWorker('/shared.js');w.port.onmessage=e=>resolve(e.data);w.onerror=e=>reject(Error(e.message))}
 if(realm==='service'){navigator.serviceWorker.onmessage=e=>resolve(e.data);await navigator.serviceWorker.register('/service.js');const r=await navigator.serviceWorker.ready;r.active.postMessage('probe')}
 }catch(e){reject(e)}}),new Promise((_,reject)=>setTimeout(()=>reject(Error('probe timeout')),8000))]),realm)}catch(e){results[realm]={error:String(e)}}
 }
 fs.writeFileSync(path.join(__dirname,'identity-'+label+'.json'),JSON.stringify({version:b.version(),results},null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
