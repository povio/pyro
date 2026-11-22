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
   const append=(el,parent=document.body)=>parent.appendChild(el);
   const host=append(document.createElement("div"));host.id="host";
   const shadow=host.attachShadow({mode:"closed",referenceTarget:"inside"});
   const inside=append(document.createElement("div"),shadow);inside.id="inside";
   const receivers=[...["button","a","area"].map(tag=>document.createElement(tag)),document.createElementNS("http://www.w3.org/2000/svg","a")];
   const out={};
   for(const el of receivers){
     append(el);const vals=[];out[el.constructor.name]=vals;
     vals.push("interestForElement" in Object.getPrototypeOf(el));
     el.setAttribute("interestfor","host");vals.push(el.interestForElement===host);
     shadow.referenceTarget="absent";vals.push(el.interestForElement===host);shadow.referenceTarget="inside";
     el.interestForElement=inside;vals.push(el.interestForElement===null);
     el.interestForElement=host;vals.push(el.getAttribute('interestfor')==='' && el.interestForElement===host);
     el.setAttribute('interestfor','');vals.push(el.interestForElement===null);
     el.interestForElement=host;const copy=el.cloneNode(true);vals.push(copy.interestForElement===null);
     el.interestForElement=null;vals.push(!el.hasAttribute('interestfor'));
     try{el.interestForElement={};vals.push(false)}catch(e){vals.push(e.name==='TypeError')}
   }
   const button=append(document.createElement("button"),shadow);
   button.interestForElement=host;out.ancestor=button.interestForElement===host;
   const fragment=document.createDocumentFragment(),a=append(document.createElement("a"),fragment),other=append(document.createElement("div"),fragment);
   a.interestForElement=other;out.detached=[a.interestForElement===other];document.body.appendChild(other);out.detached.push(a.interestForElement===null);
   out.scope=["interestForElement" in document.createElement('div'),"interestForElement" in document.createElement('input')];
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
