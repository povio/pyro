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
   const m=navigator.storageBuckets,out={surface:[typeof StorageBucketManager,typeof StorageBucket,!!m]};if(!m)return out;
   const err=async f=>{try{await f();return 'ok'}catch(e){return e.name}};
   out.identity=[m===navigator.storageBuckets, m instanceof StorageBucketManager,StorageBucketManager.length,StorageBucket.length];
   const now=Date.now(),dt=now+600000,first=await m.open('a1',{quota:65536,expires:dt}),other=await m.open('z');
   const again=await m.open('a1');out.reopen=[first!==again,first.name,again.name,await again.expires()===dt,(await first.estimate()).quota];
   out.methods=[typeof first.durability,typeof first.persist,await first.persisted(),typeof first.getDirectory];
   out.names=await m.keys();
   out.nameLimits=[await err(()=>m.open('_bad')),await err(()=>m.delete('-bad')),await err(()=>m.open('a'.repeat(64))),await err(()=>m.open('a'.repeat(65))),await err(()=>m.open(Symbol()))];
   const invalids=[0,-1,NaN,Infinity,2**53,1n];out.quotaInvalid=[];for(const q of invalids)out.quotaInvalid.push(await err(()=>m.open('policy',{quota:q})));
   out.expiryInvalid=[await err(()=>m.open('expired',{expires:now-1000})),await err(()=>first.setExpires(now-1000))];
   await m.delete('a1');const replacement=await m.open('a1',{quota:131072});out.generations=[await err(()=>first.persisted()),await err(()=>again.estimate()),await replacement.expires(),(await replacement.estimate()).quota];
   await m.delete('missing');out.sorted=(await m.keys()).every((n,i,a)=>!i||a[i-1]<=n);
   for(const n of await m.keys())await m.delete(n);
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-catalog.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
