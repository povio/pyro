const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
async function runCases(){
   const root=await navigator.storage.getDirectory(),file=await root.getFileHandle('task-bytes',{create:true});
   const out=[];
   for(const method of ['write','writer','command'])for(const depth of [0,1,4,32]){
     const stream=await file.createWritable(),view=new Uint8Array([7,1,8]).subarray(1,2);
     let settled=false, writer=null;
     if(method==='writer')writer=stream.getWriter();
     const promise=(method==='writer'?writer.write(view):method==='command'?stream.write({type:'write',position:0,data:new DataView(view.buffer,1,1)}):stream.write(view));
     promise.then(()=>{settled=true});
     for(let step=0;step<depth;step++)await Promise.resolve();
     const settledBeforeChange=settled;view[0]=9;
     await promise;if(writer)await writer.close();else await stream.close();
     out.push([method,depth,settledBeforeChange,...new Uint8Array(await (await file.getFile()).arrayBuffer())]);
   }
   const validation=[];
   for(const type of ['write','seek','truncate']){
     const f=await file.createWritable();try{await f.write({type});validation.push([type,'accepted'])}catch(e){validation.push([type,e.name])}
   }
   const snapshot=await file.getFile();await root.removeEntry(file.name);
   let stale;try{stale=Array.from(new Uint8Array(await snapshot.arrayBuffer()))}catch(e){stale=e.name}
   return {out,validation,stale};
}
const workerCode='onmessage=async()=>postMessage(await ('+runCases.toString()+')())';
const makeServer=()=>http.createServer((req,res)=>{
 if(req.url==='/worker.js'){res.setHeader('Content-Type','text/javascript');res.end(workerCode);}
 else{res.setHeader('Content-Type','text/html');res.end('<!doctype html><meta charset=utf-8><body>filesystem fixture');}
});
const server=makeServer();
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-svg-fixture-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const primary=await b.newContext();const page=await primary.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const window=await page.evaluate(runCases);
 const worker=await page.evaluate(()=>new Promise((resolve,reject)=>{
   const w=new Worker('/worker.js');const t=setTimeout(()=>{w.terminate();reject(Error('worker timeout'))},12000);
   w.onmessage=e=>{clearTimeout(t);w.terminate();resolve(e.data)};w.onerror=e=>{clearTimeout(t);w.terminate();reject(Error(e.message))};w.postMessage(1);
 }));
 const values={window,worker};
 fs.writeFileSync(path.join(w,label+'-task.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
