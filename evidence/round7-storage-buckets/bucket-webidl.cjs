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
   const mgr=navigator.storageBuckets, out={surface:!!mgr};if(!mgr)return out;
   const type=async (fn,expected=TypeError)=>{let p;try{p=fn()}catch(e){return ['throw',e.name,e instanceof expected]}
     try{await p;return ['resolved']}catch(e){return ['rejected',e.name,e instanceof expected]}};
   const frame=document.createElement('iframe');document.body.appendChild(frame);
   const win=frame.contentWindow,other=win.navigator.storageBuckets;const bucket=await other.open('child');
   const readName=Object.getOwnPropertyDescriptor(StorageBucket.prototype,'name').get;
   out.foreign=[readName.call(bucket),await StorageBucket.prototype.persisted.call(bucket),
     (await StorageBucketManager.prototype.keys.call(other)).includes('child'),
     (await StorageBucketManager.prototype.open.call(other,'foreign')).name];
   out.illegal=[await type(()=>mgr.open.call({},'n')),await type(()=>mgr.keys.call({})),await type(()=>mgr.delete.call({},'n')),
     await type(()=>StorageBucket.prototype.expires.call({})),await type(()=>StorageBucket.prototype.setExpires.call({},1))];
   const order=[];const opts={get expires(){order.push('expires');return {valueOf(){order.push('to-number');return Date.now()+120000}}},
      get persisted(){order.push('persisted');return false},get quota(){order.push('quota');return 1000}};
   const opened=await mgr.open({toString(){order.push('name');return 'sequence'}},opts);
   out.options=[order,(await opened.estimate()).quota];
   out.bad=[await type(()=>mgr.open('bad',1)),await type(()=>mgr.open('bad',{quota:Object(2n)})),await type(()=>opened.setExpires({valueOf(){return 1n}}))];
   const OldError=win.TypeError;frame.remove();out.detached=await type(()=>other.keys(),OldError);
   for(const name of await mgr.keys())await mgr.delete(name);
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-bucket-webidl.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
