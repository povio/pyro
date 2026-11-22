const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const server=http.createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end('<!doctype html><body style="margin:0">');});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-svg-fixture-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(async()=>{
   const out={};const c=await caches.open('default-check');await c.put('default',new Response('bytes'));out.cache=[(await caches.keys()).includes('default-check'),await (await caches.match('default')).text()];
   await caches.delete('default-check');out.kept=await (await c.match('default')).text();
   const idb=indexedDB;const req=idb.open('default-check',1);req.onupgradeneeded=()=>req.result.createObjectStore('s');
   const db=await new Promise((r,j)=>{req.onsuccess=()=>r(req.result);req.onerror=()=>j(req.error)});
   const tx=db.transaction('s','readwrite');tx.objectStore('s').put({n:1},'x');await new Promise((r,j)=>{tx.oncomplete=r;tx.onabort=()=>j(tx.error)});
   const read=db.transaction('s').objectStore('s').get('x');const obj=await new Promise((r,j)=>{read.onsuccess=()=>r(read.result);read.onerror=()=>j(read.error)});
   out.idb=[idb===indexedDB,req instanceof IDBOpenDBRequest,db instanceof IDBDatabase,db.name,db.version,obj.n];db.close();
   const deletion=idb.deleteDatabase('default-check');await new Promise((r,j)=>{deletion.onsuccess=r;deletion.onerror=()=>j(deletion.error)});
   const root=await navigator.storage.getDirectory(),f=await root.getFileHandle('default-check',{create:true}),w=await f.createWritable();await w.write('value');await w.close();
   out.opfs=[root instanceof FileSystemDirectoryHandle,await root.isSameEntry(await navigator.storage.getDirectory()),await (await f.getFile()).text()];await root.removeEntry('default-check');
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-default-endpoints.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
