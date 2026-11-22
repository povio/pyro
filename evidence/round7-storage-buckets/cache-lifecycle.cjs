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
   const buckets=navigator.storageBuckets,out={surface:!!buckets};if(!buckets)return out;
   const bucket=await buckets.open('catalog'),cs=bucket.caches;const e=async()=>[await bucket.estimate(),await navigator.storage.estimate()];
   const err=async f=>{try{await f();return 'ok'}catch(e){return e.name}};
   const [b0,s0]=await e();const c=await cs.open('keep');await c.put('asset',new Response('x'.repeat(32768)));
   const [b1,s1]=await e();out.used=[b1.usage>b0.usage,s1.usage>s0.usage,!!b1.usageDetails?.caches,!!s1.usageDetails?.caches];
   await cs.delete('keep');const [b2]=await e();
   out.unlink=[!(await cs.keys()).includes('keep'),(await (await c.match('asset')).text()).length,b2.usage<b1.usage];
   out.orphanWrite=await err(()=>c.put('extra',new Response('orphan')));
   const fresh=await cs.open('keep');out.reopened=[!!await fresh.match('asset'),!!await fresh.match('extra')];
   await buckets.delete('catalog');const newBucket=await buckets.open('catalog');
   out.stale=[await err(()=>cs.keys()),await err(()=>cs.open('after')),await err(()=>c.put('old',new Response('x'))),(await newBucket.caches.keys()).length];
   await buckets.delete('catalog');
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-cache-lifecycle.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
