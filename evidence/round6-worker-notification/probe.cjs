const {spawn}=require("node:child_process");
const fs=require("node:fs"),net=require("node:net"),path=require("node:path");
const {chromium}=require("playwright-core");
const [label,bin]=process.argv.slice(2);
const script=`
async function report(op) {
  if (op==='permission') return Notification.permission;
  if (op==='surface') return [typeof Notification, typeof Notification.requestPermission];
  if (op==='response-module') {
    const response=new Response(Uint8Array.of(0,97,115,109,1,0,0,0),{headers:{'Content-Type':'application/wasm'}});
    const bytes=await response.arrayBuffer();
    const mod=await new Promise((resolve,reject)=>setTimeout(()=>{try{resolve(new WebAssembly.Module(bytes))}catch(e){reject(e)}},0));
    return [response.bodyUsed,mod instanceof WebAssembly.Module,WebAssembly.Module.imports(mod).length,WebAssembly.Module.exports(mod).length];
  }
  if (op==='sequence') {
    let primitive,closed=0;const marker={};
    try{new Notification('x',{actions:''});primitive='accepted'}catch(e){primitive=e.name}
    const actions={[Symbol.iterator](){return {next(){return {done:false,value:{get action(){throw marker}}}},return(){closed++;return {done:true}}}}};
    try{new Notification('x',{actions});return [primitive,false,closed]}catch(e){return [primitive,e===marker,closed]}
  }
  if (op==='module') {
    const mod=new WebAssembly.Module(Uint8Array.of(0,97,115,109,1,0,0,0));
    return [Object.prototype.toString.call(mod),WebAssembly.Module.imports(mod).length,WebAssembly.Module.exports(mod).length];
  }
  if (op==='service') {
    try{new Notification('ordinary');return 'constructed';}catch(e){return e.name;}
  }
  if (op==='order') {
    const seen=[],marker={};
    try{new Notification('order',{get body(){seen.push('body');return 'text'},get data(){seen.push('data');throw marker}})}
    catch(e){return [e===marker,seen.join(',')]}
  }
  if (op==='copied') {
    let reads=0; const n=new Notification('data',{get data(){reads++;return {a:3}}});
    return [reads,n.data.a,n.actions===n.actions,Object.isFrozen(n.actions)];
  }
  if (op==='data') {
    const input={items:[1,2]};
    const n=new Notification('ordinary',{data:input,icon:'icon.png',body:'hello'});
    input.items.push(3);
    return [n.title,n.body,n.data.items.join(','),n.icon===new URL('icon.png',location.href).href];
  }
}
async function answer(port,op) {try{port.postMessage({result:await report(op)});}catch(e){port.postMessage({error:e.name+': '+e.message});}}
`;
const http=Bun.serve({port:0,hostname:'127.0.0.1',fetch(r){
 const p=new URL(r.url).pathname;
 if(p==='/page')return new Response('<!doctype html>',{headers:{'Content-Type':'text/html'}});
 if(p==='/ctx/dedicated.js')return new Response(script+';onmessage=e=>answer(self,e.data)',{headers:{'Content-Type':'application/javascript'}});
 if(p==='/ctx/shared.js')return new Response(script+';onconnect=e=>{const p=e.ports[0];p.onmessage=e=>answer(p,e.data);p.start()}',{headers:{'Content-Type':'application/javascript'}});
 if(p==='/ctx/sw.js')return new Response(script+";oninstall=e=>e.waitUntil(self.skipWaiting());onactivate=e=>e.waitUntil(self.clients.claim());onmessage=e=>answer(e.ports[0],e.data)",{headers:{'Content-Type':'application/javascript'}});
 return new Response('',{status:404});}});
(async()=>{
 let child,browser;const results={};
 const port=await new Promise(resolve=>{const s=net.createServer();s.listen(0,'127.0.0.1',()=>{const p=s.address().port;s.close(()=>resolve(p));})});
 try {
 child=spawn(bin,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe','--cdp-max-connections','64'],{stdio:'ignore',detached:true});
 const cdp='http://127.0.0.1:'+port;
 for(let i=0;i<300;i++){try{if((await fetch(cdp+'/json/version')).ok)break;}catch{}await Bun.sleep(30)}
 browser=await chromium.connectOverCDP(cdp);
 const ctx=await browser.newContext(),p=await ctx.newPage(),session=await ctx.newCDPSession(p);
 const {targetInfo}=await session.send('Target.getTargetInfo');
 const browserContextId=targetInfo.browserContextId;
 const origin='http://127.0.0.1:'+http.port;
 await p.goto(origin+'/page');
 await p.evaluate(()=>{window.dw=new Worker('/ctx/dedicated.js');window.swPort=new SharedWorker('/ctx/shared.js').port;swPort.start()});
 const check=async(name,fn)=>{try{results[name]=await Promise.race([fn(),new Promise((_,j)=>setTimeout(()=>j(Error('timeout')),4500))])}catch(e){results[name]={error:e.message}}};
 const worker=(port,op)=>p.evaluate(([key,op])=>new Promise(resolve=>{
 const target=window[key];target.onmessage=e=>resolve(e.data);target.postMessage(op);
 }),[port,op]);
 for(const who of ['dw','swPort'])for(const op of ['surface','module','response-module','data','copied','order','sequence'])await check(who+'-'+op,()=>worker(who,op));
 for(const value of ['denied','granted','prompt']){
  await session.send('Browser.setPermission',{permission:{name:'notifications'},setting:value,origin,...(browserContextId?{browserContextId}:{})});
  await check('window-'+value,()=>p.evaluate(()=>Notification.permission));
  for(const who of ['dw','swPort'])await check(who+'-'+value,()=>worker(who,'permission'));
 }
 await check('service',()=>p.evaluate(async()=>{
   const reg=await navigator.serviceWorker.register('/ctx/sw.js',{scope:'/ctx/'});
   const active=reg.active||reg.installing||reg.waiting;
   if(active.state!=='activated')await new Promise(resolve=>active.addEventListener('statechange',()=>{if(active.state==='activated')resolve()}));
   const call=op=>new Promise(resolve=>{const c=new MessageChannel();c.port1.onmessage=e=>resolve(e.data);active.postMessage(op,[c.port2])});
   const r=[await call('surface'),await call('service'),await call('order')];await reg.unregister();return r;
 }));
 await ctx.close();
 }finally{fs.writeFileSync(path.join(__dirname,'probe-'+label+'.json'),JSON.stringify(results,null,2)+'\n');await browser?.close().catch(()=>{});if(child)try{process.kill(-child.pid,'SIGKILL')}catch{};http.stop(true);}
})().catch(e=>{console.error(e.message);process.exitCode=1});
