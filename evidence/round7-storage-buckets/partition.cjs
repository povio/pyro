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
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const ownerContext=await b.newContext();const page=await ownerContext.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const a='http://127.0.0.1:'+port, bSite='http://localhost:'+port;
 const supported=await page.evaluate(()=>!!navigator.storageBuckets);
 const values={surface:supported};
 if (supported) {
   await page.evaluate(async()=>{await navigator.storageBuckets.open('partition-proof')});
   const frame=async(parent,url)=>{await parent.evaluate(async url=>{await new Promise((resolve,reject)=>{const f=document.createElement('iframe');f.onload=resolve;f.onerror=reject;f.src=url;document.body.appendChild(f)})},url);return parent.childFrames().find(f=>f.url()===url)};
   const check=f=>f.evaluate(async()=>(await navigator.storageBuckets.keys()).includes('partition-proof'));
   const top=page.mainFrame(),same=await frame(top,a+'/same'),middle=await frame(top,bSite+'/b'),nested=await frame(middle,a+'/nested');
   values.top=await check(top);values.same=await check(same);values.crossOrigin=await check(middle);values.nested=await check(nested);
   const ctx=await b.newContext();try{const p=await ctx.newPage();await p.goto(a+'/different-context');values.separateContext=await check(p)}finally{await ctx.close()}
   await page.evaluate(()=>navigator.storageBuckets.delete('partition-proof'));
 }
 fs.writeFileSync(path.join(w,label+'-partition.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
