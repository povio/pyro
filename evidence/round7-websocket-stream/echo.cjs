const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const {wsServer:WebSocketServer}=require(path.join(ref,'runner/node_modules/playwright-core/lib/utilsBundle.js'));
const server=http.createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end('<!doctype html><body></body>')});
const ws=new WebSocketServer({server,perMessageDeflate:false});
ws.on('connection',(peer,req)=>{
  peer.on('error',()=>{});
  if(req.url==='/remote')return setTimeout(()=>{if(peer.readyState===1)peer.close(4001,'server done')},25);
  if(req.url==='/greet'){peer.send('one');peer.send('two');}
  peer.on('message',(data,bin)=>peer.send(data,{binary:bin}));
});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-svg-fixture-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(async()=>{
   const out={surface:[typeof WebSocketStream,typeof WebSocketError]};if(typeof WebSocketStream!=='function')return out;
   const url=location.origin.replace('http:','ws:')+'/echo',socket=new WebSocketStream(url,{protocols:['demo']});
   out.identity=[socket.opened===socket.opened,socket.closed===socket.closed,socket.url===url];
   const {readable,writable,protocol,extensions}=await socket.opened;
   out.streams=[readable instanceof ReadableStream,writable instanceof WritableStream,protocol,extensions];
   const wr=writable.getWriter(),rd=readable.getReader();
   const results=[];const chunks=['hello',new Uint8Array([11,22,33]).subarray(1),new Uint8Array(0),''];
   for(const chunk of chunks){await wr.write(chunk);const r=await rd.read();const tag=Object.prototype.toString.call(r.value);const data=typeof r.value==='string'?r.value:r.value instanceof ArrayBuffer?Array.from(new Uint8Array(r.value)):r.value instanceof Blob?Array.from(new Uint8Array(await r.value.arrayBuffer())):Array.from(r.value);results.push([r.done,tag,data])}
   out.echo=results;socket.close({closeCode:4000,reason:'done'});const close=await socket.closed;out.close=[close.closeCode,close.reason,(await rd.read()).done];

   return out;
 });
 fs.writeFileSync(path.join(w,label+'-echo.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();ws.close();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
