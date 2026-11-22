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
   const out={surface:typeof InterestEvent};if(typeof InterestEvent!=='function')return out;
   const host=document.body.appendChild(document.createElement("div")),root=host.attachShadow({mode:"closed"});
   const source=root.appendChild(document.createElement("button")),target=root.appendChild(document.createElement("span"));
   const ev=new InterestEvent("probe",{source,bubbles:true,composed:true});
   const trace=[],identity=e=>e.source===source?'source':e.source===host?'host':e.source===null?'null':'other';
   trace.push(identity(ev));for (const node of [target,root,host,document]) node.addEventListener("probe",e=>trace.push(identity(e)),{once:true});
   target.dispatchEvent(ev);trace.push(identity(ev));out.trace=trace;
   out.defaults=[new InterestEvent("x").source===null,ev.isTrusted,ev.relatedTarget===undefined,ev instanceof Event,ev instanceof InterestEvent];
   const plain=document.createElement("div"),manual=new InterestEvent("again",{source:plain});
   const descriptor=Object.getOwnPropertyDescriptor(InterestEvent.prototype,"source");
   out.brand=[manual.source===plain,typeof descriptor.get==="function",descriptor.set===undefined];
   try{new InterestEvent("x",{source:document.createTextNode("x")});out.invalid=false}catch(e){out.invalid=e.name==="TypeError"}
   function pathFor(event){
     const heard=[];const listeners=[target,root,host,document].map((el,i)=>{
       const fn=e=>heard.push([i, e instanceof InterestEvent ? identity(e) : 'ordinary']);
       el.addEventListener(event.type,fn);return [el,fn];
     });target.dispatchEvent(event);for(const [el,fn] of listeners)el.removeEventListener(event.type,fn);return heard;
   }
   out.distinct=pathFor(new InterestEvent('distinct',{source:plain,bubbles:true,composed:true}));
   out.nullSource=pathFor(new InterestEvent('nullsrc',{bubbles:true,composed:true}));
   out.ordinary=pathFor(new Event('ordinary',{bubbles:true,composed:true}));
   out.dictionary=[undefined,null,{},false,0,'text'].map(v=>{try{return new InterestEvent("dict",v).source===null?'null':'other'}catch(e){return e.name}});
   const frame=document.body.appendChild(document.createElement('iframe'));const child=frame.contentDocument;
   const foreign=child.createElement('button');child.body.appendChild(foreign);
   const fromChild=new InterestEvent('child-source',{source:foreign});
   out.frame=[fromChild.source===foreign,new frame.contentWindow.InterestEvent('outer-source',{source:host}).source===host];
   foreign.addEventListener('child-source',e=>out.frame.push(e.source===foreign),{once:true});foreign.dispatchEvent(fromChild);
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-event-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
