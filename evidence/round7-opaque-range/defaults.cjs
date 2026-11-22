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
   for(const tag of ['input','textarea']){
     const s={};out[tag]=s;const element=document.body.appendChild(document.createElement(tag));element.defaultValue='abc';const r=element.createValueRange(1,2);
     const oldChild=element.firstChild;const mo=new MutationObserver(()=>{});mo.observe(element,{childList:true});
     element.defaultValue='abc';s.sameDefault=[r.startOffset,r.endOffset];s.childReplacement=[element.firstChild===oldChild,mo.takeRecords().length];mo.disconnect();
     if(tag==='textarea'){element.firstChild.replaceData(1,1,'b');s.sameData=[r.startOffset,r.endOffset];}
     const form=document.body.appendChild(document.createElement('form'));form.appendChild(element);element.defaultValue='abc';
     const r2=element.createValueRange(1,2);element.value='abc';element.defaultValue='different';s.dirtyDefault=[element.value,r2.startOffset,r2.endOffset];
   }
   const empty=document.body.appendChild(document.createElement('textarea'));empty.defaultValue='abc';
   const er=empty.createValueRange(1,3);empty.defaultValue='';out.empty=[empty.firstChild===null,empty.value,er.startOffset,er.endOffset];
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-defaults.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
