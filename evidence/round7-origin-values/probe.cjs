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
   const out={surface:typeof Origin};if(typeof Origin!=='function')return out;
   const fresh=new Origin(),fresh2=new Origin(),copy=Origin.from(fresh),base=Origin.from('https://site.example:443/path'),
         same=Origin.from(new URL('https://site.example/another')),port=Origin.from('https://site.example:9000/'),
         sub=Origin.from('https://a.site.example/'),http=Origin.from('http://site.example/');
   out.opaque=[fresh.opaque,fresh.isSameOrigin(fresh),fresh.isSameOrigin(fresh2),copy.isSameOrigin(fresh),copy.isSameSite(fresh)];
   out.tuple=[base.opaque,base.isSameOrigin(same),base.isSameOrigin(port),base.isSameSite(port),base.isSameSite(sub),base.isSameSite(http)];
   const a=document.createElement('a');a.href='/test';const area=document.createElement('area');area.href='https://site.example/x';
   out.links=[Origin.from(a).isSameOrigin(Origin.from(self)),Origin.from(area).isSameOrigin(base)];
   let coercions=0;const bad={toString(){++coercions;return location.href}},errors=[];
   for(const input of [bad,document.createElement('div'),new MessageEvent('message',{origin:location.origin}),location,null,1]){
      try{Origin.from(input);errors.push('accepted')}catch(e){errors.push(e.name)}
   }
   out.invalid=[errors,coercions];
   const proxy=open('about:blank','origin-control');
   if(proxy){
     let traps=0;const wrapped=new Proxy(proxy,{get(){++traps;return undefined}});
     let proxyError;try{Origin.from(wrapped);proxyError='accepted'}catch(e){proxyError=e.name}
     try{out.popup=[Origin.from(proxy).isSameOrigin(Origin.from(self)),proxyError,traps]}catch(e){out.popup=[e.name,proxyError,traps]}
     proxy.close();
   }
   out.message=await new Promise(resolve=>{const f=e=>{if(e.data==='origin-fixture'){removeEventListener('message',f);try{resolve(Origin.from(e).isSameOrigin(Origin.from(self)))}catch(err){resolve(err.name)}}};addEventListener('message',f);postMessage('origin-fixture','*');setTimeout(()=>{removeEventListener('message',f);resolve('timeout')},2500)});
   const childURL=URL.createObjectURL(new Blob(["onmessage=()=>postMessage([Origin.from(self).opaque,Origin.from(self).isSameOrigin(Origin.from(self))])"],{type:'text/javascript'}));
   out.worker=await new Promise(resolve=>{const w=new Worker(childURL);const t=setTimeout(()=>{w.terminate();resolve('timeout')},2500);w.onmessage=e=>{clearTimeout(t);w.terminate();resolve(e.data)};w.onerror=()=>{clearTimeout(t);w.terminate();resolve('error')};w.postMessage(1)});URL.revokeObjectURL(childURL);
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-probe.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
