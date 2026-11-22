const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=path.dirname(__dirname),ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const server=http.createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end('<!doctype html><body style="margin:0">');});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-audio-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();await page.goto('http://127.0.0.1:'+port+'/page');
 const values=await page.evaluate(async()=>{
   async function trace(){
     const results={};for(const codec of ['av01.0.04M.08','vp8']){
       const events=[],cfg={codec,width:640,height:480,displayWidth:800,displayHeight:600};
       const encoder=new VideoEncoder({output(){events.push(['output'])},error(e){events.push(['error',e.name,e.message,encoder.state,encoder.encodeQueueSize])}});
       events.push(['support',(await VideoEncoder.isConfigSupported(cfg)).supported]);
       encoder.configure(cfg);events.push(['configure returned',encoder.state]);
       await encoder.flush().then(()=>events.push(['flush resolved']),e=>events.push(['flush rejected',e.name,e.message,encoder.state,encoder.encodeQueueSize]));
       await new Promise(r=>setTimeout(r,10));results[codec]=events;
     }return results;
   }
   const out={window:await trace()};
   const url=URL.createObjectURL(new Blob(['onmessage=async()=>{postMessage(await ('+trace.toString()+')())}'],{type:'text/javascript'}));
   const worker=new Worker(url);try{out.worker=await new Promise((r,j)=>{worker.onmessage=e=>r(e.data);worker.onerror=e=>j(Error(e.message));worker.postMessage(0)})}finally{worker.terminate();URL.revokeObjectURL(url)}
   return out;
 });
 fs.writeFileSync(path.join(__dirname,label+'-handled.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
