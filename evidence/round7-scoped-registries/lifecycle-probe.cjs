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
 const values=await page.evaluate(()=>{
   const rows=[];
   function check(name,fn){try{rows.push([name,fn()])}catch(e){rows.push([name,{error:e.name,message:e.message}])}}
   check('retained scoped definitions after normal frame removal',()=>{
     const frame=document.createElement('iframe');document.body.append(frame);
     const home=frame.contentWindow,reg=new home.CustomElementRegistry();
     const ctor=home.eval("(class extends HTMLElement {})");
     reg.define('retained-label',ctor);
     const instance=document.createElement('retained-label',{customElementRegistry:reg});
     frame.remove();
     return [reg.get('retained-label')===ctor,instance.customElementRegistry===reg,instance instanceof ctor];
   });
   check('registry remains usable when the retained instance is created in its own realm',()=>{
     const frame=document.createElement('iframe');document.body.append(frame);
     const home=frame.contentWindow,reg=new home.CustomElementRegistry(),ctor=home.eval("(class extends HTMLElement {})");
     reg.define('retained-when-adopted',ctor);const el=home.document.createElement('retained-when-adopted',{customElementRegistry:reg});
     document.adoptNode(el);frame.remove();
     return [reg.get('retained-when-adopted')===ctor,el.customElementRegistry===reg,el instanceof ctor];
   });
   check('detached foreign host receives a later shadow flag',()=>{
     const target=open('about:blank','registry-metadata'),reg=new CustomElementRegistry();
     try{
       const host=target.document.createElement('detached-panel',{customElementRegistry:reg});
       class Panel extends HTMLElement{static disabledFeatures=['shadow']}
       reg.define('detached-panel',Panel);
       let error;try{host.attachShadow({mode:'open'})}catch(e){error=e.name}
       return [host.customElementRegistry===reg,error??null];
     }finally{target.close()}
   });
   return rows;
 });
 fs.writeFileSync(path.join(w,label+'-lifecycle.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
