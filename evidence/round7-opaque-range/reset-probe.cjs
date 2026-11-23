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
   const out={};const form=document.body.appendChild(document.createElement("form"));form.id="sample-form";
   const control=document.body.appendChild(document.createElement("input"));control.setAttribute("form","sample-form");
   control.defaultValue="original";control.value="changed";control.setCustomValidity("sample");
   const box=form.appendChild(document.createElement("input"));box.type="checkbox";box.defaultChecked=true;box.checked=false;
   const select=form.appendChild(document.createElement("select"));select.innerHTML="<option selected>a</option><option>b</option>";select.selectedIndex=1;
   const file=form.appendChild(document.createElement('input'));file.type='file';
   const range=typeof control.createValueRange==='function'?control.createValueRange(2,4):null;
   const read=()=>[control.value,control.validationMessage,box.checked,select.selectedIndex,file.files.length,range?[range.startOffset,range.endOffset]:null];
   const seen=[];form.addEventListener('reset',e=>{seen.push(read());e.preventDefault()},{once:true});form.reset();out.cancelled=read();
   form.addEventListener('reset',()=>seen.push(read()),{once:true});form.reset();out.reset=read();out.handlers=seen;
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-reset-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
