const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
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
   const out={surface:[typeof AudioData,typeof AudioDecoder]};
   const fn=async e=>{
     const audio=e.data;if(!audio)return postMessage([typeof AudioData,typeof AudioDecoder]);
     const samples=new Uint8Array(audio.numberOfFrames);audio.copyTo(samples,{planeIndex:0});
     const shape=[audio.format,audio.timestamp,...samples];
     postMessage({audio,shape},[audio]);
   };
   const url=URL.createObjectURL(new Blob(['onmessage='+fn.toString()],{type:'text/javascript'}));
   const worker=new Worker(url);
   try{
     const ask=(v,t=[])=>new Promise((resolve,reject)=>{
       const timer=setTimeout(()=>reject(Error('worker timeout')),4000);
       worker.onmessage=e=>{clearTimeout(timer);resolve(e.data)};
       worker.onmessageerror=()=>{clearTimeout(timer);reject(Error('messageerror'))};worker.postMessage(v,t);
     });
     out.worker=await ask(null);
     if(typeof AudioData==='function'){
       const init={format:'u8',sampleRate:8000,numberOfChannels:1,numberOfFrames:3,timestamp:750,data:new Uint8Array([0,128,255])};
       const pcm=new AudioData(init);
       const first=await ask(pcm);out.cloned=[pcm.numberOfFrames,first.shape];first.audio.close();
       const second=await ask(pcm,[pcm]);const bytes=new Uint8Array(3);second.audio.copyTo(bytes,{planeIndex:0});
       out.transferred=[pcm.numberOfFrames,second.shape,...bytes];second.audio.close();
     }
   }finally{worker.terminate();URL.revokeObjectURL(url)}
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-transport.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
