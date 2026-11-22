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
   for(const tag of ['textarea','input']){
     const form=document.body.appendChild(document.createElement('form')),el=form.appendChild(document.createElement(tag));
     const controls={};out[tag]=controls;
     for(const [label,mutate,clean] of [
       ['splice',()=>el.setRangeText('XYZ',2,4)],['spliceEqual',()=>el.setRangeText('cd',2,4)],['valueMiddle',()=>el.value='abXYZef'],
       ['valueSame',()=>el.value='abcdef'],['valueAll',()=>el.value='Q'],['reset',()=>form.reset()],
       ['defaultValue',()=>el.defaultValue='abXYZef',true],['type',()=>{if(tag==='input')el.type='search';}]
     ]){
       el.defaultValue=clean?'abcdef':'XY';form.reset();if(!clean)el.value='abcdef';
       const rs=Array.from({length:7},(_,i)=>el.createValueRange(i,i));mutate();
       controls[label]=rs.map(r=>r.startOffset);for(const r of rs)r.disconnect();
     }
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-boundaries.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
