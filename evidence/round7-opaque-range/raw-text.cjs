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
   const f=document.body.appendChild(document.createElement('textarea'));f.appendChild(document.createTextNode('a\nb'));
   const same=f.createValueRange(1,3);f.firstChild.data='a\r\nb';out.normalized=[f.value,f.defaultValue,offsets(same)];
   const el=document.body.appendChild(document.createElement('textarea'));el.appendChild(document.createTextNode('abc'));
   const r=el.createValueRange(1,2),span=el.appendChild(document.createElement('span'));span.textContent='descendant';out.nested=[el.value,el.defaultValue,offsets(r)];span.firstChild.data='modified';out.nestedEdit=[el.value,el.defaultValue,offsets(r)];
   const input=document.body.appendChild(document.createElement('input'));input.value='abcd';const rt=input.createValueRange(1,3);input.type='search';out.supportedType=[input.value,offsets(rt)];
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-raw-text.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
