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
   const makeWorker=()=>new Worker(URL.createObjectURL(new Blob(["("+function(){
      let unlock;onmessage=async ({data})=>{try{
        if(data==='surface'){postMessage(!!navigator.locks);return}
        if(data==='unlock'){unlock();return}
        await navigator.locks.request('order',async lock=>{postMessage({state:'held',mode:lock.mode});await new Promise(resolve=>unlock=resolve)});
        postMessage({state:'settled'});
      }catch(e){postMessage({state:'reject',name:e.name})}}
   }.toString()+")()"],{type:'text/javascript'})));
   const worker=makeWorker();
   const next=(start)=>new Promise((resolve,reject)=>{
     const handler=e=>{clearTimeout(t);worker.removeEventListener('message',handler);resolve(e.data)};
     const t=setTimeout(()=>{worker.removeEventListener('message',handler);reject(Error('timeout'))},3000);
     worker.addEventListener('message',handler);start?.();
   });
   const waitPending=async n=>{for(let i=0;i<150;i++){const q=await navigator.locks.query();if(q.pending.filter(x=>x.name==='order').length===n)return q;await new Promise(r=>setTimeout(r,8))}throw Error('queue-timeout')};
   let releaseFirst,releaseSecond;
   try{
     out.worker=await next(()=>worker.postMessage('surface'));if(!out.worker)return out;
     const hold=resolve=>async ()=>{await new Promise(r=>resolve(r))};
     let readyA,readyB;const a=new Promise(r=>readyA=r),b=new Promise(r=>readyB=r);
     const first=navigator.locks.request('order',{mode:'shared'},hold(fn=>{releaseFirst=fn;readyA()}));
     const second=navigator.locks.request('order',{mode:'shared'},hold(fn=>{releaseSecond=fn;readyB()}));await Promise.all([a,b]);
     const grant=next(()=>worker.postMessage('hold'));await waitPending(1);
     let entered=false;const late=navigator.locks.request('order',{mode:'shared'},()=>{entered=true});
     const q=await waitPending(2);out.ordered=[q.held.filter(x=>x.name==='order').map(x=>x.mode),q.pending.filter(x=>x.name==='order').map(x=>x.mode)];
     releaseFirst();await first;out.oneSharedRemaining=(await navigator.locks.query()).held.filter(x=>x.name==='order').map(x=>x.mode);
     releaseSecond();await second;out.grant=await grant;out.queuedWaits=!entered;
     const ac=new AbortController();let abortedCallback=false;const aborted=navigator.locks.request('order',{signal:ac.signal},()=>{abortedCallback=true}).then(()=>false,e=>e==='reason');
     ac.abort('reason');out.aborted=[await aborted,abortedCallback];const settled=next(()=>worker.postMessage('unlock'));await settled;await late;out.late=entered;
     out.stealSetup=await next(()=>worker.postMessage('hold'));const broken=next();
     await navigator.locks.request('order',{steal:true},lock=>{out.steal=lock.name});
     out.broken=await broken;worker.postMessage('unlock');out.clean=(await navigator.locks.query()).held.filter(x=>x.name==='order').length;
   }catch(e){out.error=e.message}
   finally{releaseFirst?.();releaseSecond?.();worker.terminate()}
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-queue-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
