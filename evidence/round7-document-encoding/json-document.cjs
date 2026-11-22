const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const jsonText='{"label":"雪 € <b> &amp;"}',types={
 app:'application/json',text:'text/json',plus:'application/problem+json',
 upper:'APPLICATION/JSON',legacy:'application/json; charset=windows-1251',utf8:'application/json; charset=UTF-8',
 bom:'application/json',plain:'text/plain; charset=windows-1251'};
const server=http.createServer((req,res)=>{const id=req.url.slice(1);if(id==='page'||id==='utf8-parent'){res.setHeader('Content-Type',id==='page'?'text/html; charset=windows-1251':'text/html; charset=utf-8');return res.end('<!doctype html><body></body>')}
 res.setHeader('Content-Type',types[id]||'text/plain');res.end(id==='plain'?Buffer.from([0xdf,0xf0]):Buffer.from((id==='bom'?'\uFEFF':'')+jsonText));});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-audio-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const read=async()=>page.evaluate(async()=>{
   const out={parentCharset:document.characterSet};
   for(const id of ['app','text','plus','upper','legacy','utf8','bom','plain']){
     const f=document.createElement('iframe');const loaded=new Promise(r=>f.onload=r);f.src='/'+id;document.body.append(f);await loaded;
     const d=f.contentDocument;out[id]={encoding:d.characterSet,type:d.contentType,text:d.body.textContent,literalMarkup:d.body.querySelector('b')===null};f.remove();
   }
   return out;
 });
 const legacyParent=await read();await page.goto('http://127.0.0.1:'+port+'/utf8-parent');const utf8Parent=await read();const values={legacyParent,utf8Parent};
 fs.writeFileSync(path.join(w,label+'-json-document.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
