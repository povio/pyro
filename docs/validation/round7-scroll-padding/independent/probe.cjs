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
   const out={};
   const el=(parent,style)=>{const e=document.createElement("div");parent.appendChild(e);e.style.cssText=style;return e};
   const reset=()=>{document.body.innerHTML="";document.documentElement.style.cssText="";document.body.style.cssText="margin:0";window.scrollTo(0,0)};
   for(const [name,axis,align] of [["percent","", "center"],["near-move","","nearest"],["near-stay","","nearest"],["vertical-rl","writing-mode:vertical-rl;direction:rtl","start"],["rtl","direction:rtl","end"]]){
     reset();const s=el(document.body,"width:240px;height:160px;overflow:scroll;scrollbar-width:none;position:relative;scroll-padding:calc(10% + 4px) 5% 15% calc(10% + 2px);"+axis);
     const far=axis.includes("rtl");
     const c=el(s,"position:relative;width:800px;height:600px");
     const t=el(c,"position:absolute;width:20px;height:20px;top:"+(name==="near-stay"?60:260)+"px;"+(far?"right:":"left:")+(name==="near-stay"?100:350)+"px");
     t.scrollIntoView({block:align,inline:align});out[name]=[s.scrollLeft,s.scrollTop,s.clientWidth,s.clientHeight];
   }
   reset();const o=el(document.body,"position:relative;width:300px;height:240px;overflow:scroll;scrollbar-width:none;scroll-padding:12px 0 0 18px");
   const slab=el(o,"width:1400px;height:1200px;position:relative");
   const i=el(slab,"position:absolute;top:400px;left:450px;width:180px;height:120px;overflow:scroll;scrollbar-width:none;scroll-padding:8px 0 0 14px");
   const ic=el(i,"width:700px;height:700px;position:relative");const it=el(ic,"position:absolute;left:300px;top:250px;width:20px;height:20px");it.scrollIntoView({block:"start",inline:"start"});out.nested=[i.scrollLeft,i.scrollTop,o.scrollLeft,o.scrollTop];
   for(const who of ["root","body"]){reset();document.body.style.cssText="margin:0;width:4000px;height:4000px";(who==="root"?document.documentElement:document.body).style.scrollPadding="40px 0 0 32px";const t=el(document.body,"position:absolute;left:1500px;top:1500px;width:20px;height:20px");t.scrollIntoView({block:"start",inline:"start"});out[who]=[scrollX,scrollY]}
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
