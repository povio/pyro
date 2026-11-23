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
 const origin='http://127.0.0.1:'+port;
 const other=await page.context().newPage();await other.goto(origin+'/peer');
 const ctx=await b.newContext();const isolated=await ctx.newPage();await isolated.goto(origin+'/isolated');
 const values={};
 await page.evaluate(async()=>{await new Promise(r=>{navigator.locks.request('context-guard',async()=>{r();await new Promise(release=>self.releaseGuard=release)})})});
 for (const [name,p] of [['owner',page],['peer',other],['otherContext',isolated]]){
   values[name]=await p.evaluate(async()=>{const q=await navigator.locks.query(); const granted=await navigator.locks.request('context-guard',{ifAvailable:true},lock=>!!lock);return [q.held.some(x=>x.name==='context-guard'),granted]});
 }
 await page.evaluate(()=>self.releaseGuard());await ctx.close();
 fs.writeFileSync(path.join(w,label+'-context-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
