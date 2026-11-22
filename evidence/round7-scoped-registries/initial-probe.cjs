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
   const rows=[];
   const check=async(name,fn)=>{try{rows.push([name,await fn()])}catch(e){rows.push([name,{error:e.name,message:e.message}])}};
   const name='report-badge', G=class extends HTMLElement{}, S=class extends HTMLElement{};
   customElements.define(name,G);
   await check('global construction',()=>document.createElement(name) instanceof G);
   await check('document registry',()=>document.customElementRegistry===customElements);
   await check('valid name predicate',()=>{try{customElements.define('bad-/suffix',class extends HTMLElement{});return 'accepted'}catch(e){return e.name}});
   let scoped;
   await check('construct distinct registry',()=>{scoped=new CustomElementRegistry();return scoped!==customElements&&scoped instanceof CustomElementRegistry});
   if(!scoped)return rows;
   await check('separate definitions and promise',async()=>{
     const wait=scoped.whenDefined(name);scoped.define(name,S);return [await wait===S,scoped.get(name)===S,scoped.getName(S)===name,customElements.get(name)===G];
   });
   await check('element creation',()=>{
     const a=document.createElement(name,{customElementRegistry:scoped}), b=document.createElement(name,{customElementRegistry:null});
     return [a instanceof S,a.customElementRegistry===scoped,b.customElementRegistry===null,!(b instanceof G),document.createElement('div',{customElementRegistry:scoped}).customElementRegistry===scoped];
   });
   await check('shadow fragment',()=>{
     const root=document.createElement('div').attachShadow({mode:'open',customElementRegistry:scoped});
     root.innerHTML='<report-badge></report-badge>';return [root.customElementRegistry===scoped,root.firstChild instanceof S,root.firstChild.customElementRegistry===scoped];
   });
   await check('initialize null subtree',()=>{
     const host=document.createElement('div'),root=host.attachShadow({mode:'open',customElementRegistry:null});
     root.innerHTML='<report-badge></report-badge>';const el=root.firstChild,old=el.customElementRegistry;
     scoped.initialize(root);return [old===null,el.customElementRegistry===scoped,el instanceof S,root.customElementRegistry===scoped];
   });
   await check('connected late definition',()=>{
     const a=document.createElement('report-late',{customElementRegistry:scoped}),seen=[];document.body.append(a);
     class Late extends HTMLElement{static observedAttributes=['title'];constructor(){super();seen.push('constructed')}connectedCallback(){seen.push('connected')}attributeChangedCallback(){seen.push('attribute')}}
     a.setAttribute('title','a');scoped.define('report-late',Late);return [a instanceof Late,seen];
   });
   await check('inert document and import',()=>{
     const doc=document.implementation.createHTMLDocument();const orig=doc.createElement(name);
     const copy=document.importNode(orig,{customElementRegistry:scoped});return [doc.customElementRegistry===null,copy.customElementRegistry===scoped,copy instanceof S];
   });
   await check('default and scoped adoption',()=>{
     const frame=document.createElement('iframe');document.body.append(frame);const doc=frame.contentDocument,foreign=frame.contentWindow.customElements;
     const a=document.createElement('div'),b=document.createElement('div',{customElementRegistry:scoped});
     doc.adoptNode(a);doc.adoptNode(b);const r=[a.customElementRegistry===foreign,b.customElementRegistry===scoped];frame.remove();return r;
   });
   await check('same constructor in two registries',()=>{
     const c=class extends HTMLElement{},other=new CustomElementRegistry;scoped.define('both-report',c);other.define('both-report',c);
     return [scoped.get('both-report')===c,other.get('both-report')===c,customElements.get('both-report')===undefined];
   });
   return rows;});
 fs.writeFileSync(path.join(w,label+'-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
