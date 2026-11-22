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
   const out={surface:typeof WebSocketStream};if(typeof WebSocketStream!=='function')return out;
   const url=location.origin.replace('http:','ws:'), reason={marker:'stop'};
   const ctrl=new AbortController();ctrl.abort(reason);const aborted=new WebSocketStream(url+'/echo',{signal:ctrl.signal});
   const same=async(p)=>{try{await p;return 'resolved'}catch(e){return e===reason?'original':e.name}};
   out.preAbort=await Promise.all([same(aborted.opened),same(aborted.closed)]);
   const post=new AbortController(),s=new WebSocketStream(url+'/echo',{signal:post.signal});const {readable,writable}=await s.opened;post.abort('later');const wr=writable.getWriter(),rd=readable.getReader();
   await wr.write('after opening');out.postOpen=(await rd.read()).value;await wr.close();const done=await s.closed;out.writerClose=[done.closeCode,done.reason,(await rd.read()).done];
   const remote=new WebSocketStream(url+'/remote');const rw=await remote.opened;const rc=await remote.closed;const fail=async f=>{try{await f();return 'ok'}catch(e){return e.name}};
   out.remote=[rc.closeCode,rc.reason,(await rw.readable.getReader().read()).done,await fail(()=>rw.writable.getWriter().ready)];
   const cancel=new WebSocketStream(url+'/echo');const pair=await cancel.opened;await pair.readable.cancel(new WebSocketError('cancel',{closeCode:4002,reason:'end'}));const cc=await cancel.closed;out.cancel=[cc.closeCode,cc.reason];

   return out;
 });
 fs.writeFileSync(path.join(w,label+'-lifecycle.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();ws.close();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
