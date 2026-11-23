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
   async function visit(mode){
     const bucket=await navigator.storageBuckets.open('handoff');const req=bucket.indexedDB.open('history',1);
     req.onupgradeneeded=()=>req.result.createObjectStore('s');
     const db=await new Promise((r,j)=>{req.onsuccess=()=>r(req.result);req.onerror=()=>j(req.error)});
     const tx=db.transaction('s',mode==='read'?'readonly':'readwrite'),store=tx.objectStore('s');
     const done=new Promise((r,j)=>{tx.oncomplete=r;tx.onabort=()=>j(tx.error)});
     const read=store.get('k');let previous;await new Promise((r,j)=>{read.onsuccess=()=>{previous=read.result??null;if(mode!=='read')store.put(mode,'k');r()};read.onerror=()=>j(read.error)});
     await done;db.close();const dir=await bucket.getDirectory(),fh=await dir.getFileHandle('entry',{create:mode==='window'}),old=await (await fh.getFile()).text();
     if(mode!=='read'){const wr=await fh.createWritable();await wr.write(mode);await wr.close()}
     return [previous,old];
   }
   out.before=await visit('window');const url=URL.createObjectURL(new Blob([visit.toString()+";onmessage=async()=>{try{postMessage(await visit('worker'))}catch(e){postMessage({error:e.name})}}"],{type:'text/javascript'}));const wk=new Worker(url);
   try{out.worker=await new Promise(r=>{const t=setTimeout(()=>r('timeout'),4000);wk.onmessage=e=>{clearTimeout(t);r(e.data)};wk.postMessage(1)})}finally{wk.terminate();URL.revokeObjectURL(url)}
   out.after=await visit('read');await m.delete('handoff');
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-worker-data.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
