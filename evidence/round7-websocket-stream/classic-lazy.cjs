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
   const frame=document.body.appendChild(document.createElement('iframe'));const global=frame.contentWindow;
   const original=Object.getOwnPropertyDescriptor(global,'Promise');let reads=0;const out={};
   Object.defineProperty(global,'Promise',{configurable:true,get(){reads++;throw Error('unexpected Promise access')}});
   try{
     out.constructors=[typeof global.WebSocket,typeof global.CloseEvent];
     const close=new global.CloseEvent('close',{code:1000,reason:'unchanged'});out.event=[close.code,close.reason];
   }finally{Object.defineProperty(global,'Promise',original);frame.remove()}
   out.promiseReads=reads;
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-classic-lazy.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();ws.close();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
