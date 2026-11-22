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
   const out={surface:"referenceTarget" in ShadowRoot.prototype};
   const el=(tag,parent=document.body)=>parent.appendChild(document.createElement(tag));
   const host=el("div");host.id="host";
   const sh=host.attachShadow({mode:"closed",referenceTarget:"nested"});
   const nested=el("div",sh);nested.id="nested";
   const inner=nested.attachShadow({mode:"open",referenceTarget:"target"});
   const list=el("datalist",inner);list.id="target";
   const input=el("input");input.setAttribute("list","host");
   out.nestedList=input.list===host;
   inner.referenceTarget="absent";out.missingList=input.list===null;
   inner.referenceTarget="target";list.id="new";out.renamedList=input.list===null;
   inner.referenceTarget="new";out.restoredList=input.list===host;
   const wrong=el("div");wrong.id="wrong";input.setAttribute("list","wrong");out.wrongType=input.list===null;
   const local=el("input",inner);local.setAttribute("list","new");out.localList=local.list===list;
   const control=el("input",inner);control.id="control";inner.referenceTarget="control";
   const label=el("label");label.htmlFor="host";
   out.label=[label.control===host,Array.from(control.labels).includes(label)];
   const form=el("form",inner);form.id="form";inner.referenceTarget="form";input.setAttribute("form","host");
   out.form=[input.form===host,Array.from(form.elements).includes(input)];
   const invoker=el("button");invoker.commandForElement=host;invoker.setAttribute("command","--probe");
   let fired=0;form.addEventListener("command",()=>fired++);invoker.click();
   out.command=[invoker.commandForElement===host,fired];
   const observed=[];class Face extends HTMLElement {
     static formAssociated=true;
     constructor(){super();this.internals=this.attachInternals()}
     formAssociatedCallback(f){observed.push(f===host?'host':f===form?'inner':f===null?'null':'other')}
   }
   customElements.define("x-probe-control",Face);const face=el("x-probe-control");face.setAttribute("form","host");
   out.face=[face.internals.form===host,observed];
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
