const http=require('http'),net=require('net'),cp=require('child_process'),fs=require('fs'),path=require('path');
const cache=path.resolve(__dirname,'../..'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
const [label,exe]=process.argv.slice(2);const listen=s=>new Promise(resolve=>s.listen(0,'127.0.0.1',()=>resolve(s.address().port)));
(async()=>{const server=http.createServer((req,res)=>res.end('<body>')),port=await listen(server);let proc,b;try{
 if(label==='chrome')b=await pw.chromium.launch({executablePath:path.join(cache,'round6-reference-158/Google Chrome.app/Contents/MacOS/Google Chrome'),headless:true});
 else{const sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));proc=cp.spawn(exe,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error'],{stdio:'ignore'});for(let i=0;i<100;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,80));} b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp)}
 const page=await b.newPage();await page.goto('http://127.0.0.1:'+port);
 const results=await page.evaluate(async()=>{
   const rows={}, make=C=>new C({data:new Uint8Array([7,8]),type:'delta',timestamp:12});
   const error=fn=>{try{fn();return 'accepted'}catch(e){return e.name}};
   const open=indexedDB.open('chunks',1);open.onupgradeneeded=()=>open.result.createObjectStore('s');
   const db=await new Promise((ok,fail)=>{open.onsuccess=()=>ok(open.result);open.onerror=()=>fail(open.error)});
   for (const C of [globalThis.EncodedAudioChunk,globalThis.EncodedVideoChunk]) {
     if(!C)continue;
     const ch=make(C),wrapped={v:[ch]},tx=db.transaction('s','readwrite'),store=tx.objectStore('s');
     rows[C.name+'/historyPush']=error(()=>history.pushState(wrapped,''));
     rows[C.name+'/historyReplace']=error(()=>history.replaceState(wrapped,''));
     rows[C.name+'/add']=error(()=>store.add(wrapped,1));
     rows[C.name+'/put']=error(()=>store.put(wrapped,2));
     const put=store.put({ordinary:[2,3]},'ok'),get=store.get('ok');
     rows[C.name+'/dbAfterReject']=await new Promise((ok,fail)=>{get.onsuccess=()=>ok(get.result.ordinary.join(','));get.onerror=()=>fail(get.error)});
     const a=structuredClone({a:ch,b:ch});rows[C.name+'/alias']=(a.a===a.b && a.a!==ch && a.a instanceof C);
   }
   const o={n:1};history.replaceState({a:o,b:o},'');rows.ordinaryHistoryAliasing=history.state.a===history.state.b;
   const blob=structuredClone(new Blob(['ab']));rows.ordinaryBlob=await blob.text();
   db.close();return rows
 });fs.writeFileSync(path.join(__dirname,label+'-storage.json'),JSON.stringify({version:b.version(),results},null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
