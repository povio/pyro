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
   const settle=r=>new Promise(resolve=>{r.onsuccess=()=>{r.result?.close?.();resolve('success')};r.onerror=()=>resolve(r.error?.name)});
   const b=await m.open('idb'),f=b.indexedDB,op=f.open('test',1);op.onupgradeneeded=()=>op.result.createObjectStore('s');
   const db=await new Promise((r,j)=>{op.onsuccess=()=>r(op.result);op.onerror=()=>j(op.error)});
   let closed=0,onclose=0;db.addEventListener('close',()=>closed++);db.onclose=()=>onclose++;
   const tx=db.transaction('s','readwrite'),done=new Promise(r=>{tx.oncomplete=()=>r('complete');tx.onabort=()=>r(tx.error?.name||'abort')});
   tx.objectStore('s').put('v',1);const del=m.delete('idb');
   out.transaction=await done;out.deleted=await Promise.race([del.then(()=>true,e=>e.name),new Promise(r=>setTimeout(()=>r('pending'),2000))]);
   if(out.deleted==='pending'){db.close();await del}
   try{db.transaction('s');out.heldDB='open'}catch(e){out.heldDB=e.name}
   try{const stale=f.open('after');out.staleFactory=['returned',await Promise.race([settle(stale),new Promise(r=>setTimeout(()=>r('pending'),2000))])]}catch(e){out.staleFactory=['threw',e.name]}
   const before=closed;for(let i=0;i<50&&closed===0;i++)await new Promise(r=>setTimeout(r,10));out.closeEvent=[before,closed,onclose,'onclose' in IDBDatabase.prototype];const fresh=await m.open('idb');out.newNames=(await fresh.indexedDB.databases()).map(x=>x.name);
   db.close();await m.delete('idb');
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-idb-close.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
