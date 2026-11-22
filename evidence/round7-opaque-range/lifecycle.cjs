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
   const out={surface:typeof OpaqueRange};if(typeof OpaqueRange==='undefined')return out;
   const offsets=r=>[r.startOffset,r.endOffset,r.collapsed];
   for(const tag of ['textarea','input']){
     const values={};out[tag]=values;
     const p=document.createElement('div'),el=p.appendChild(document.createElement(tag));el.defaultValue='abcdef';
     const range=el.createValueRange(3,6);el.defaultValue='ab';values.shortDefault=[el.value,offsets(range)];
     el.value='abcdef';const rem=el.createValueRange(3,6);el.remove();values.detachedRemove=offsets(rem);
     const adopt=el.createValueRange(2,4),d=document.implementation.createHTMLDocument();d.adoptNode(el);values.detachedAdopt=offsets(adopt);
     const host=document.body.appendChild(document.createElement('div')); const shadow=host.attachShadow({mode:'closed'});
     const nested=shadow.appendChild(document.createElement(tag));nested.value='abc';const z=nested.createValueRange(1,2);host.remove();values.hostRemoval=offsets(z);
     const f=document.body.appendChild(document.createElement('form')),same=f.appendChild(document.createElement(tag));
     same.defaultValue='abc';same.setCustomValidity('custom');const noChange=same.createValueRange(1,2);f.reset();values.resetUnchanged=[offsets(noChange),same.validationMessage];
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-lifecycle.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
