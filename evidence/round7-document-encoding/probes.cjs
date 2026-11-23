const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const fixtures={
 'header':['text/html; charset=iso-8859-1','<meta charset="utf-8"><p>',[0x80,0xe9],'</p>'],
 'meta':['text/html','<meta charset="windows-1252"><p>',[0x80,0xe9],'</p>'],
 'plain':['text/plain; charset=csisolatin1','',[0x80,0xe9],''],
 'shiftjis':['text/html','<meta charset="shift_jis"><p>',[0x82,0xa0],'</p>'],
 'bom':['text/html; charset=windows-1252','',[0xef,0xbb,0xbf],'<p>π</p>'],
 'utf8':['text/html; charset=utf-8','<p>',[0xcf,0x80],'</p>'],
};
const requests=[];
const server=http.createServer((req,res)=>{const id=req.url.split('?')[0].slice(1),item=fixtures[id];
 if(!item){res.setHeader('Content-Type','text/html');return res.end('<!doctype html><body></body>')}
 requests.push(req.url);res.setHeader('Content-Type',item[0]);res.end(Buffer.concat([Buffer.from(item[1]),Buffer.from(item[2]),Buffer.from(item[3])]));
});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-audio-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(async()=>{
   const out={};const ids=['header','meta','plain','shiftjis','bom','utf8'];
   const fetchFrame=(name,phase)=>new Promise(resolve=>{
     const f=document.createElement('iframe');const timer=setTimeout(()=>{f.remove();resolve({name,timeout:true})},6000);
     f.onload=()=>{clearTimeout(timer);const doc=f.contentDocument;const value={name,encoding:[doc.characterSet,doc.inputEncoding,doc.charset],text:doc.body?.textContent};f.remove();resolve(value)};
     f.src='/'+name+'?'+phase;document.body.appendChild(f);
   });
   out.series=[];for(const name of ids)out.series.push(await fetchFrame(name,'single'));
   out.parallel=await Promise.all(ids.map(id=>fetchFrame(id,'parallel')));
   out.xhr=await new Promise((resolve,reject)=>{const x=new XMLHttpRequest;x.open('GET','/plain?xhr');x.onload=()=>resolve(x.responseText);x.onerror=()=>reject(Error('xhr'));x.send()});
   out.bytes=[...new Uint8Array(await (await fetch('/plain?bytes')).arrayBuffer())];out.textDecoder=new TextDecoder('csisolatin1').decode(new Uint8Array(out.bytes));
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-probes.json'),JSON.stringify({values,requests},null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
