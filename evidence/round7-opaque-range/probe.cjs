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
   const el=document.body.appendChild(document.createElement('textarea'));el.value='a🌱bcdef';
   const pair=r=>[r.startOffset,r.endOffset,r.collapsed], r=el.createValueRange(1,5);
   out.prototype=[r instanceof AbstractRange,r instanceof OpaqueRange,r instanceof Range,typeof r.startContainer,r.startContainer===null,typeof r.endContainer];
   out.initial=pair(r);el.setRangeText('Z',0,1);out.sameSize=pair(r);el.setRangeText('XYZ',0,1);out.expanded=pair(r);
   const old=el.value;el.value=old;out.sameValue=pair(r);el.value='new';out.replaced=pair(r);
   let r2=el.createValueRange(1,3);r.disconnect();out.explicit=[pair(r),pair(r2)];
   const doc=document.implementation.createHTMLDocument();doc.adoptNode(el);out.adopt=pair(r2);
   const input=document.body.appendChild(document.createElement('input'));input.value='abcdef';
   const r3=input.createValueRange(5,2);out.backwards=pair(r3);input.type='number';out.typeChange=pair(r3);
   const fail=fn=>{try{fn();return 'ok'}catch(e){return e.name}};out.errors=[fail(()=>input.createValueRange(0,0)),fail(()=>el.createValueRange(0,99)),fail(()=>new OpaqueRange())];
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
