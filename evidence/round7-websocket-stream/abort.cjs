const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const {wsServer:WebSocketServer}=require(path.join(ref,'runner/node_modules/playwright-core/lib/utilsBundle.js'));
const seen={};const server=http.createServer((req,res)=>{if(req.url==='/counts'){res.end(JSON.stringify(seen));return}res.setHeader('Content-Type','text/html');res.end('<!doctype html><body></body>')});
const ws=new WebSocketServer({noServer:true,perMessageDeflate:false});
server.on('upgrade',(req,socket,head)=>{seen[req.url]=(seen[req.url]||0)+1;
 const accept=()=>{if(!socket.destroyed)ws.handleUpgrade(req,socket,head,peer=>ws.emit('connection',peer,req))};
 if(req.url.startsWith('/delay-'))setTimeout(accept,250);else accept();
});
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
   const out={surface:typeof WebSocketStream};if(typeof WebSocketStream!=='function')return out;
   const base=location.origin.replace('http:','ws:');const custom={why:1};
   const state=async(socket,act)=>{
     const observe=p=>p.then(()=>({resolved:true}),e=>e);const outcomes=Promise.all([observe(socket.opened),observe(socket.closed)]);await act();const [a,b]=await outcomes;
     return [a.name,b.name,a===b,a===custom,b===custom,a instanceof DOMException,b instanceof DOMException];
   };
   const p=new AbortController();p.abort(custom);out.pre=await state(new WebSocketStream(base+'/pre',{signal:p.signal}),async()=>{});
   const a=new AbortController();out.mid=await state(new WebSocketStream(base+'/delay-abort',{signal:a.signal}),async()=>{await new Promise(r=>setTimeout(r,50));a.abort(custom)});
   const closing=new WebSocketStream(base+'/delay-close');out.close=await state(closing,async()=>closing.close());
   const counts=await (await fetch('/counts')).json();out.preConnections=counts['/pre']||0;
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-abort.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();ws.close();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
