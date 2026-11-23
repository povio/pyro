const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const server=http.createServer((req,res)=>{res.setHeader('Content-Type','text/html');if(req.url.startsWith('/child?')){res.end(`<!doctype html><script>
 const bc=new BroadcastChannel(location.search.slice(1));
 const bars=['locationbar','menubar','personalbar','scrollbars','statusbar','toolbar'];
 const objects=bars.map(k=>window[k]);
 bc.onmessage=(e)=>{if(e.data==='close')close();};
 bc.postMessage({print:matchMedia('print').matches,referrer:document.referrer,hasOpener:!!opener,hasReferrer:!!document.referrer,
 visible:objects.map(b=>b?.visible??null),brand:objects[0]?.constructor.name??null,
 identity:bars.every((k,i)=>objects[i]===window[k])&&new Set(objects).size===6,
 size:[innerWidth,innerHeight],position:[screenX,screenY]});
 <\/script>`)}else{res.end('<!doctype html><body>popup controls')};});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-svg-fixture-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(()=>{
 const bars=['locationbar','menubar','personalbar','scrollbars','statusbar','toolbar'];
 const root={visible:bars.map(k=>window[k]?.visible??null),distinct:new Set(bars.map(k=>window[k])).size,
 brand:window.locationbar?.constructor.name??null};
 const original=window.menubar;window.menubar='override';root.replaceable=window.menubar==='override';delete window.menubar;root.restored=window.menubar===original;
 return root;});
 const rows=[];
 for(const features of ['', 'popup=0', 'popup=yes,width=310,height=207,left=33,top=22',
 'toolbar=yes,location=yes,menubar=yes,resizable=yes,scrollbars=yes,status=yes','noopener=0,noreferrer=0',
 'noopener=1','noreferrer=yes','popup=yes,popup=no','popup=1junk','popup="yes"']){
   const row=await page.evaluate(async features=>{
     const key=crypto.randomUUID(),bc=new BroadcastChannel(key);
     let timer;
     const reply=new Promise(resolve=>{bc.onmessage=e=>resolve(e.data);timer=setTimeout(()=>resolve(null),2000)});
     const handle=open('/child?'+key,'_blank',features);
     const child=await reply;clearTimeout(timer);bc.postMessage('close');bc.close();if(handle)handle.close();
     return {features,returnedHandle:!!handle,child};
   },features);rows.push(row);
 }
 const frame=await page.evaluate(()=>{
   const e=document.createElement('iframe');document.body.append(e);const props=Array.from(['locationbar','menubar'],k=>e.contentWindow[k]);const before=props.map(x=>x?.visible??null);e.remove();
   return {before,after:props.map(x=>x?.visible??null)};
 });
 const session=await page.context().newCDPSession(page);
 await session.send('Emulation.setEmulatedMedia',{media:'print'});
 const reuse=await page.evaluate(async ()=>{
   const token=crypto.randomUUID(),name='normal-'+token;
   const results=[];let first;
   for(const suffix of ['a','b']){
     const key=token+suffix,bc=new BroadcastChannel(key);
     let timer;const reply=new Promise(ok=>{bc.onmessage=e=>ok(e.data);timer=setTimeout(()=>ok(null),2000)});
     const handle=open('/child?'+key,name,'popup=0');if(!first)first=handle;
     const child=await reply;clearTimeout(timer);bc.close();
     results.push({sameHandle:first===handle,child});
   }
   first?.close();return {openerPrint:matchMedia('print').matches,results};
 });
 const result={root:values,rows,frame,reuse};
 fs.writeFileSync(path.join(w,label+'-probe.json'),JSON.stringify(result,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
