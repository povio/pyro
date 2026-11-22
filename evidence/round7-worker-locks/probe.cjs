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
   const out={window:!!navigator.locks};
   if(!out.window)return out;
   const workerSource = "(" + function(){
     let release, held;
     onmessage=async e=>{
       try {
         const m=e.data;
         if(m==='surface')return postMessage([typeof LockManager, typeof Lock, !!navigator.locks, navigator.locks===navigator.locks]);
         if(m==='release'){release();return;}
         if(m==='query'){const s=await navigator.locks.query();return postMessage([m,s.held.map(r=>[r.name,r.mode,r.clientId]),s.pending.map(r=>[r.name,r.mode,r.clientId])]);}
         if(m==='hold') {
           held=navigator.locks.request("ordinary",async lock=>{
             postMessage(['granted',lock.name,lock.mode,lock instanceof Lock]); await new Promise(r=>release=r);
           });await held;return postMessage('released');
         }
         if(m==='available')return postMessage(await navigator.locks.request('ordinary',{ifAvailable:true},x=>x===null));
       }catch(e){postMessage({error:e.name})}
     };
   }.toString()+")()";
   const blob=URL.createObjectURL(new Blob([workerSource],{type:'text/javascript'}));
   const work=new Worker(blob);
   const receive=(task)=>new Promise((resolve,reject)=>{
     let t;const done=(e)=>{clearTimeout(t);work.removeEventListener('message',done);resolve(e.data);};
     work.addEventListener('message',done);t=setTimeout(()=>{work.removeEventListener('message',done);reject(new Error('worker reply timeout'))},3000);task();
   });
   const ask=m=>receive(()=>work.postMessage(m));
   const until=async f=>{let val;for(let i=0;i<150;i++){val=await f();if(val)return val;await new Promise(r=>setTimeout(r,8))}throw Error('state timeout')};
   try{
     out.worker=await ask('surface');if(!out.worker[2])return out;
     let releaseWindow;const entered=new Promise(resolve=>{navigator.locks.request('ordinary',async()=>{
       resolve();await new Promise(r=>releaseWindow=r);
     })});await entered;out.whileHeld=await ask('available');
     const grant=receive(()=>work.postMessage('hold'));
     const pending=await until(async()=>{const q=await navigator.locks.query();return q.pending.find(r=>r.name==='ordinary')&&q});
     const windowId=pending.held.find(r=>r.name==='ordinary').clientId, workerId=pending.pending.find(r=>r.name==='ordinary').clientId;
     out.queues=[pending.held.filter(r=>r.name==='ordinary').length,pending.pending.filter(r=>r.name==='ordinary').length, !!workerId,!!windowId,windowId!==workerId];
     releaseWindow();out.granted=await grant;
     const q=await ask('query');out.workerQuery=[q[0],q[1].some(r=>r[0]==='ordinary'&&r[1]==='exclusive'&&r[2]===workerId),q[2].length];
     out.afterGrant=await navigator.locks.request('ordinary',{ifAvailable:true},lock=>lock===null);
     out.release=await ask('release');
     out.postRelease=await navigator.locks.request('ordinary',{ifAvailable:true},lock=>!!lock);
     out.beforeTerminate=await ask('hold');work.terminate();
     out.termination=await until(async()=>navigator.locks.request('ordinary',{ifAvailable:true},lock=>!!lock));
   }catch(e){out.error=e.message}
   finally{work.terminate();URL.revokeObjectURL(blob)}
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
