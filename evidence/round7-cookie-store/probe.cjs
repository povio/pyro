const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const sw=`self.addEventListener('install',e=>e.waitUntil(self.skipWaiting()));self.addEventListener('activate',e=>e.waitUntil(self.clients.claim()));
self.addEventListener('cookiechange',e=>e.waitUntil((async()=>{for(const c of await clients.matchAll({includeUncontrolled:true}))c.postMessage({type:'cookiechange',changed:e.changed.map(v=>v.name),deleted:e.deleted.map(v=>v.name)})})()));
self.addEventListener('message',e=>{if(e.data==='surface')e.ports[0].postMessage({store:typeof cookieStore,manager:typeof registration.cookies,extendable:typeof ExtendableCookieChangeEvent})});`;
const server=http.createServer((req,res)=>{
  const u=new URL(req.url,'http://localhost');
  if(u.pathname==='/sw.js'){res.setHeader('Content-Type','text/javascript');res.end(sw);return;}
  if(u.pathname==='/set-http')res.setHeader('Set-Cookie',['shared-http=network; Secure; Path=/; SameSite=Lax','private-http=backend; HttpOnly; Secure; Path=/; SameSite=Strict']);
  if(u.pathname==='/echo'){res.end(req.headers.cookie||'');return;}
  res.setHeader('Content-Type','text/html');res.end('<!doctype html><meta charset=utf-8><body>cookie store controls');
});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
try{const bin=explicit||path.join(ref,'launch-158'),tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-cookies-'));
proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const ctx=await b.newContext(),page=await ctx.newPage();await page.goto('http://127.0.0.1:'+port+'/alpha/page');
const out=await page.evaluate(async()=>{
 const rows={surface:[typeof cookieStore,typeof CookieStore,typeof CookieChangeEvent,typeof CookieStoreManager,typeof ExtendableCookieChangeEvent]};
 await fetch('/set-http');const header=await(await fetch('/echo')).text();rows.legacy=[document.cookie.includes('shared-http=network'),!document.cookie.includes('private-http='),header.includes('private-http=backend')];
 if(typeof cookieStore!=='object')return rows;
 const group=async(k,fn)=>{let timer;try{rows[k]=await Promise.race([fn(),new Promise((_,reject)=>{timer=setTimeout(()=>reject(Error('operation timed out')),8000)})])}catch(e){rows[k]={error:e.name,message:e.message}}finally{clearTimeout(timer)}};
 await group('basic',async()=>{await cookieStore.set('store-item','alpha');const c=await cookieStore.get('store-item');return [cookieStore===window.cookieStore,cookieStore instanceof CookieStore,cookieStore instanceof EventTarget,Object.keys(c),c.value,document.cookie.includes('store-item=alpha'),(await(await fetch('/echo')).text()).includes('store-item=alpha'),await cookieStore.get('private-http')];});
 await group('changes',async()=>{
  const wait=act=>new Promise(async(resolve,reject)=>{const timer=setTimeout(()=>{cookieStore.removeEventListener('change',f);reject(Error('no change event'))},2500);
    const f=e=>{const all=[...e.changed,...e.deleted];if(!all.some(c=>c.name==='event-cookie'))return;clearTimeout(timer);cookieStore.removeEventListener('change',f);resolve([e instanceof CookieChangeEvent,e.isTrusted,e.changed.map(c=>[c.name,c.value]),e.deleted.map(c=>[c.name,'value'in c]),e.changed===e.changed,Object.isFrozen(e.deleted)])};
    cookieStore.addEventListener('change',f);await act();});
  const a=await wait(()=>{document.cookie='event-cookie=from-doc; Path=/; Secure'});const b=await wait(()=>cookieStore.delete('event-cookie'));return [a,b];
 });
 await group('limits',async()=>{const failures=[];for(const x of [{name:'bad;name',value:'x'},{name:'ok',value:'bad;value'},{name:'ok',value:'x',path:'relative'},{name:'ok',value:'x',domain:'unrelated.invalid'}]){try{await cookieStore.set(x);failures.push('accepted')}catch(e){failures.push(e.name)}}return failures;});
 await group('window-url',async()=>{try{await cookieStore.getAll({url:'/other'});return 'accepted'}catch(e){return e.name}});
 await group('service-worker',async()=>{const reg=await navigator.serviceWorker.register('/sw.js');await navigator.serviceWorker.ready;const mgr=reg.cookies;await mgr.subscribe([{name:'sw-cookie'}]);const subs=(await mgr.getSubscriptions()).map(s=>[s.name,new URL(s.url).pathname]);
 const result=await new Promise(resolve=>{const f=e=>{if(e.data?.type==='cookiechange'&&e.data.changed.includes('sw-cookie')){clearTimeout(timer);navigator.serviceWorker.removeEventListener('message',f);resolve(e.data)}};const timer=setTimeout(()=>{navigator.serviceWorker.removeEventListener('message',f);resolve(null)},3000);navigator.serviceWorker.addEventListener('message',f);cookieStore.set('sw-cookie','changed');});
 await mgr.unsubscribe([{name:'sw-cookie'}]);const empty=await mgr.getSubscriptions();await reg.unregister();return [mgr===reg.cookies,mgr instanceof CookieStoreManager,subs,result,empty.length]});
 return rows;
});
if(out.basic){const isolated=await b.newContext();const p=await isolated.newPage();await p.goto('http://127.0.0.1:'+port+'/alpha/other');out.otherContext=await p.evaluate(async()=>await cookieStore.get('store-item')===null);await isolated.close();}
fs.writeFileSync(path.join(w,label+'-probe.json'),JSON.stringify(out,null,2));
}finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
