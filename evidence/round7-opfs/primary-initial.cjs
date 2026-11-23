const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const workerCode=`onmessage=async()=>{
  try{
    const root=await navigator.storage.getDirectory(),dir=await root.getDirectoryHandle('normal-fixture'),handle=await dir.getFileHandle('bytes');
    const initial=Array.from(new Uint8Array(await (await handle.getFile()).arrayBuffer()));
    const sync=await handle.createSyncAccessHandle();const out=new Uint8Array(4);
    const written=sync.write(Uint8Array.of(70,71),{at:1});
    const read=sync.read(out,{at:0});const length=sync.getSize();sync.truncate(3);sync.flush();sync.close();
    let closed;try{sync.getSize();closed='open'}catch(e){closed=e.name}
    postMessage({initial,written,read,out:Array.from(out),length,closed,final:Array.from(new Uint8Array(await (await handle.getFile()).arrayBuffer())),api:typeof FileSystemSyncAccessHandle});
  }catch(e){postMessage({error:e.name,message:e.message})}
}`;
const makeServer=()=>http.createServer((req,res)=>{
 if(req.url==='/worker.js'){res.setHeader('Content-Type','text/javascript');res.end(workerCode);}
 else{res.setHeader('Content-Type','text/html');res.end('<!doctype html><meta charset=utf-8><body>filesystem fixture');}
});
const server=makeServer(),otherServer=makeServer();
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),secondPort=await listen(otherServer),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-svg-fixture-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const primary=await b.newContext();const page=await primary.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values={};
 values.surface=await page.evaluate(()=>({secure:isSecureContext,getDirectory:typeof navigator.storage.getDirectory,handles:['FileSystemHandle','FileSystemFileHandle','FileSystemDirectoryHandle','FileSystemWritableFileStream','FileSystemSyncAccessHandle'].map(k=>typeof globalThis[k])}));
 if(values.surface.getDirectory==='function'){
   values.main=await page.evaluate(async()=>{
     const root=await navigator.storage.getDirectory(),again=await navigator.storage.getDirectory();
     const dir=await root.getDirectoryHandle('normal-fixture',{create:true}),handle=await dir.getFileHandle('bytes',{create:true});
     const read=async()=>Array.from(new Uint8Array(await (await handle.getFile()).arrayBuffer()));
     const stream=await handle.createWritable();const isWritable=stream instanceof WritableStream&&stream instanceof FileSystemWritableFileStream;
     await stream.write(Uint8Array.of(1,2,3,4));const beforeClose=await read();await stream.close();const first=await read();
     const discard=await handle.createWritable({keepExistingData:true});await discard.write('discard');await discard.abort();const aborted=await read();
     const edit=await handle.createWritable({keepExistingData:true});await edit.seek(1);await edit.write(new Blob([Uint8Array.of(8,9)]));await edit.truncate(4);await edit.close();
     const inner=await dir.getDirectoryHandle('inner',{create:true}),leaf=await inner.getFileHandle('leaf',{create:true});
     const entries=[];for await(const [name,entry] of dir)entries.push([name,entry.kind]);entries.sort();
     const names=[];for(const [what,fn] of [['mismatch',()=>dir.getDirectoryHandle('bytes')],['invalid',()=>dir.getFileHandle('a/b')],['absent',()=>dir.getFileHandle('unknown')],['nonempty',()=>dir.removeEntry('inner')]]){
       try{await fn();names.push([what,'accepted'])}catch(e){names.push([what,e.name])}
     }
     const path=await root.resolve(leaf);await dir.removeEntry('inner',{recursive:true});
     return {root:[root.name,root.kind,await root.isSameEntry(again)],identity:await handle.isSameEntry(await dir.getFileHandle('bytes')),isWritable,beforeClose,first,aborted,after:await read(),path,stalePath:await root.resolve(leaf),entries,names,file:[(await handle.getFile()).name,(await handle.getFile()) instanceof File]};
   });
   values.worker=await page.evaluate(()=>new Promise((resolve,reject)=>{const w=new Worker('/worker.js');const t=setTimeout(()=>{w.terminate();reject(Error('worker timed out'))},5000);w.onmessage=e=>{clearTimeout(t);w.terminate();resolve(e.data)};w.onerror=e=>{clearTimeout(t);w.terminate();reject(Error(e.message))};w.postMessage(1)}));
   await page.reload();values.reload=await page.evaluate(async()=>{const dir=await (await navigator.storage.getDirectory()).getDirectoryHandle('normal-fixture');return Array.from(new Uint8Array(await (await (await dir.getFileHandle('bytes')).getFile()).arrayBuffer()))});
   const absent=async p=>p.evaluate(async()=>{const root=await navigator.storage.getDirectory();try{await root.getDirectoryHandle('normal-fixture');return false}catch(e){return e.name==='NotFoundError'}});
   const same=await page.context().newPage();await same.goto('http://127.0.0.1:'+port+'/same');values.sameContext=!(await absent(same));await same.close();
   const foreign=await page.context().newPage();await foreign.goto('http://127.0.0.1:'+secondPort+'/other');values.otherOrigin=await absent(foreign);await foreign.close();
   const isolated=await b.newContext();const p=await isolated.newPage();await p.goto('http://127.0.0.1:'+port+'/fresh');values.otherContext=await absent(p);await isolated.close();
 }
 fs.writeFileSync(path.join(w,label+'-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close();otherServer.close()}})().catch(e=>{console.error(e);process.exitCode=1});
