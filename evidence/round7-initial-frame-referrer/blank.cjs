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
 for(const policy of ['origin','unsafe-url','no-referrer']){
  const ctx=await browser.newContext();try{
   const page=await ctx.newPage(), url=origin+'/container?policy='+policy+'#fragment';
   await page.goto(url);
   const rows=await page.evaluate(({origin,policy})=>{
    history.replaceState(null,'','/committed?policy='+policy+'#updated');
    const rows=[];
    for (const mode of ['implicit','explicit','pending-network']){
      const iframe=document.createElement('iframe');
      if(mode==='explicit')iframe.src='about:blank';if(mode==='pending-network')iframe.src='/later';
      document.body.append(iframe);
      const win=iframe.contentWindow;
      const custom=origin+'/explicit?query';
      rows.push({mode,parent:document.URL,referrer:win.document.referrer,
        custom:new win.Request(origin+'/echo',{referrer:custom}).referrer,
        foreign:new win.Request(origin+'/echo',{referrer:'https://example.org/foreign'}).referrer});
      iframe.remove();
    }
    return rows;
   },{origin,policy});
   pages.push(...rows.map(r=>({...r,policy})));
  }finally{await ctx.close()}
 }
 fs.writeFileSync(path.join(__dirname,label+'-blank.json'),JSON.stringify({pages,requests},null,1));
 }finally{if(browser)await browser.close();if(proc)proc.kill();server.close()}
})().catch(e=>{console.error(e);process.exit(1)});
