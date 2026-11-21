const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2),received=[],text=JSON.parse(fs.readFileSync(path.join(w,'expected.json')));
const server=http.createServer((req,res)=>{
 const url=new URL(req.url,'http://localhost');received.push({path:url.pathname,accept:req.headers.accept,dest:req.headers['sec-fetch-dest']});
 function send(type,body){res.writeHead(200,{'Content-Type':type,'Cache-Control':'no-store'});res.end(body);}
 if(url.pathname==='/root.mjs')return send('text/javascript','import text from "./same" with {type:"text"}; export default text;');
 if(url.pathname==='/same'){
  const count=received.filter(r=>r.path==='/same').length;
  if(count===1)return send('text/html;charset=windows-1252',Buffer.concat([Buffer.from([239,187,191]),Buffer.from(text)]));
  return send('text/javascript','globalThis.executions++;export default 7;');
 }
 if(url.pathname==='/bytes')return send('application/octet-stream;charset=utf-16le',Buffer.from([255,254,97,0]));
 if(url.pathname==='/plain')return send('text/plain','plain words');
 return send('text/html','<!doctype html><meta charset="utf-8"><body>Text import sample');
});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158'),tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-text-module-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const rows=await page.evaluate(async expected=>{
  globalThis.executions=0;const out=[];const check=async(name,fn)=>{try{let result=await fn();out.push([name,result])}catch(e){out.push([name,false,e.name,e.message])}};
  await check('static value preserves UTF-8 data, CRLF, quotes, NUL and U+2028',async()=> (await import('/root.mjs')).default===expected);
  await check('dynamic text is an inert default-only module',async()=>{const m=await import('/same',{with:{type:'text'}});return m.default===expected&&Object.keys(m).join(',')==='default'&&executions===0});
  await check('repeated text import shares module identity',async()=> (await import('/same',{with:{type:'text'}}))===(await import('/same',{with:{type:'text'}})));
  await check('ordinary JS at same URL has its own type cache',async()=>{const m=await import('/same');return m.default===7&&executions===1});
  await check('text remains immutable after JS import',async()=> (await import('/same',{with:{type:'text'}})).default===expected);
  await check('UTF-16 BOM never switches text-module decoding',async()=> (await import('/bytes',{with:{type:'text'}})).default==='��a\0');
  await check('ordinary JS import retains MIME validation',async()=>{try{await import('/plain');return false}catch(e){return e instanceof TypeError}});
  return out;
 },text);
 fs.writeFileSync(path.join(w,label+'-probe.json'),JSON.stringify({rows,received},null,1));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
