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
   const m=navigator.storageBuckets,out={surface:!!m};if(!m)return out;
   const block=()=>{const a=new Uint8Array(1000000);for(let i=0;i<a.length;i+=65536)crypto.getRandomValues(a.subarray(i,Math.min(a.length,i+65536)));return a};
   const open=async b=>{const req=b.indexedDB.open('records',1);req.onupgradeneeded=()=>req.result.createObjectStore('data');return await new Promise((r,j)=>{req.onsuccess=()=>r(req.result);req.onerror=()=>j(req.error)})};
   const add=async(db,rows)=>{const tx=db.transaction('data','readwrite'),store=tx.objectStore('data');const done=new Promise(r=>{tx.oncomplete=()=>r('complete');tx.onabort=()=>r(tx.error?.name||'AbortError')});for(const [key,value] of rows){store.put(value,key)}return await done};
   const count=db=>new Promise((r,j)=>{const tx=db.transaction('data');const req=tx.objectStore('data').count();req.onsuccess=()=>r(req.result);req.onerror=()=>j(req.error)});
   let b=await m.open('idb-limit',{quota:1500000}),db=await open(b);out.tx=await add(db,[[1,new Blob([block()])],[2,new Blob([block()])]]);out.afterTx=await count(db);
   out.retry=await add(db,[[1,new Blob([block().slice(0,16000)])]]);out.afterRetry=await count(db);db.close();await m.delete('idb-limit');
   b=await m.open('combined-limit',{quota:1500000});db=await open(b);const root=await b.getDirectory(),fh=await root.getFileHandle('block',{create:true});
   const wr=await fh.createWritable();await wr.write(block().slice(0,700000));await wr.close();
   out.cross=await add(db,[[1,new Blob([block()])]]);out.crossData=[await count(db),(await fh.getFile()).size];
   await root.removeEntry('block');out.refund=await add(db,[[1,new Blob([block()])]]);db.close();await m.delete('combined-limit');
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-quota.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
