const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),os=require('node:os'),cp=require('node:child_process');
const d=path.resolve(__dirname,'..'),cache=path.dirname(d),ref=path.join(cache,'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,binary]=process.argv.slice(2),requests=[],pages=[];
const server=http.createServer((req,res)=>{
 const u=new URL(req.url,'http://localhost'),policy=u.searchParams.get('policy');
 const id=u.searchParams.get('id');if(u.pathname==='/echo')requests.push({id,header:req.headers.referer??null,method:req.method,at:Date.now()});
 if(u.pathname==='/echo'){res.writeHead(200,{'Content-Type':'text/javascript','Cache-Control':'no-store'});res.end('/* ok */');return;}
 res.writeHead(200,{'Content-Type':'text/html','Cache-Control':'no-store',...(policy?{'Referrer-Policy':policy}:{})});
 res.end('<!doctype html><base href="/unrelated-base/"><body>ordinary navigation</body>');
});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,browser;try{
 const engines={base:path.join(d,'merged-aa0268/batch/visible'),head:path.join(d,'merged-aa0268/accepted158/candidate'),chrome:path.join(ref,'launch-158'),candidate:path.join(__dirname,'pyro-headers')};
 const state=fs.mkdtempSync(path.join(os.tmpdir(),'referrer-nav-'));proc=cp.spawn(binary,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:state,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<100;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 browser=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);
 const origin='http://127.0.0.1:'+port;
 for(const policy of ['strict-origin-when-cross-origin','origin','no-referrer']) for(const depth of [0,1,2,'url-srcdoc']){
  const ctx=await browser.newContext();try{
   const page=await ctx.newPage(),top=origin+'/container?policy='+policy;await page.goto(top+'#fragment');let frame=page.mainFrame(),source=top;
   const steps=depth==='url-srcdoc'?['url','srcdoc']:Array(depth).fill('srcdoc');
   for(const step of steps){
    await frame.evaluate(async({step,policy})=>{const f=document.createElement('iframe');if(step==='url')f.src='/child?policy='+policy+'#fragment';else f.srcdoc='<!doctype html><base href="/another-base/"><body>Inner document</body>';const loaded=new Promise(r=>f.onload=r);document.body.append(f);await loaded;},{step,policy});
    frame=frame.childFrames()[0];if(!frame)throw Error('Missing frame');if(step==='url')source=origin+'/child?policy='+policy;
   }
   for(const kind of ['preload','modulepreload']) for(const override of [null,'invalid','no-referrer','origin','unsafe-url']){
    const id=[policy,depth,kind,override].join('-');
    await frame.evaluate(async({kind,id,origin,override})=>{
      const link=document.createElement('link');link.rel=kind;link.as='script';link.href=origin+'/echo?id='+encodeURIComponent(id);
      if(override!==null)link.setAttribute('referrerpolicy',override);
      const load=new Promise((ok,err)=>{link.onload=ok;link.onerror=()=>err(Error('link error'))});document.head.append(link);
      await Promise.race([load,new Promise((_,reject)=>setTimeout(()=>reject(Error('load timeout')),3000))]);
    },{kind,id,origin,override});
    const request=requests.find(x=>x.id===id),actual=override===null||override==='invalid'?policy:override;
    const expected=actual==='no-referrer'?null:actual==='origin'?origin+'/':source;
    pages.push({id,kind,override,policy,depth,source,expected,received:!!request,header:request?.header??null,pass:!!request&&request.header===expected});
   }
  }finally{await ctx.close()}
 }
 fs.writeFileSync(path.join(__dirname,label+'-probe.json'),JSON.stringify({pages,requests},null,1));
 }finally{if(browser)await browser.close();if(proc)proc.kill();server.close()}
})().catch(e=>{console.error(e);process.exit(1)});
