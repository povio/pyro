const http=require('http'),net=require('net'),cp=require('child_process'),fs=require('fs'),path=require('path');
const cache=path.resolve(__dirname,'../..'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
const [label,exe]=process.argv.slice(2);const listen=s=>new Promise(resolve=>s.listen(0,'127.0.0.1',()=>resolve(s.address().port)));
(async()=>{const server=http.createServer((req,res)=>res.end('<body>')),port=await listen(server);let proc,b;try{
 if(label==='chrome')b=await pw.chromium.launch({executablePath:path.join(cache,'round6-reference-158/Google Chrome.app/Contents/MacOS/Google Chrome'),headless:true});
 else{const sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));proc=cp.spawn(exe,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error'],{stdio:'ignore'});for(let i=0;i<100;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,80));} b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp)}
 const page=await b.newPage();await page.goto('http://127.0.0.1:'+port);
 const dir=path.join(cache,'wptci/wpt-full/webcodecs');
 const setup=fs.readFileSync(path.join(dir,'videoDecoder-codec-specific-setup.js'),'utf8').split('function createVideoDecoder')[0];
 const fixtures=require('vm').runInNewContext(setup+';[AV1_DATA,H264_AVC_DATA,H264_ANNEXB_DATA]',{});
 for(const f of fixtures)f.file=fs.readFileSync(path.join(dir,f.src)).toString('base64');
 const results=await page.evaluate(async(fixtures)=>{
   const result={};
   for(const f of fixtures) {
     const data=Uint8Array.from(atob(f.file),c=>c.charCodeAt(0)),c={...f.config,rotation:90,flip:true};
     const slice=o=>data.subarray(o.offset,o.offset+o.size);
     if(c.description)c.description=slice(c.description);
     const samples=f.chunks.map((h,i)=>new EncodedVideoChunk({data:slice(h),type:i===0?'key':'delta',
       timestamp:5000000000+i*1000,duration:40+i}));
     const row={supported:(await VideoDecoder.isConfigSupported(c)).supported,rounds:[],errors:[],dequeue:[]};
     result[f.src]=row;
     if(!row.supported)continue;
     let output=[],copies=[];
     const decoder=new VideoDecoder({error:e=>row.errors.push(e.name),output:frame=>{
       const record={timestamp:frame.timestamp,duration:frame.duration,geometry:[frame.codedWidth,frame.codedHeight,frame.displayWidth,frame.displayHeight],orientation:[frame.rotation,frame.flip],format:frame.format};
       const opt={format:'RGBA',rect:{x:80,y:60,width:2,height:2}};
       const pixel=new Uint8Array(frame.allocationSize(opt));copies.push(frame.copyTo(pixel,opt).then(()=>{record.pixel=Array.from(pixel.slice(0,4))}));
       output.push(record);frame.close();
     }});
     decoder.ondequeue=e=>row.dequeue.push([e.isTrusted,decoder.decodeQueueSize]);
     decoder.configure(c);
     for(let round=0;round<2;round++){
       const start=performance.now();
       for(const sample of samples)decoder.decode(sample);
       let pending=decoder.decodeQueueSize;
       await decoder.flush();await Promise.all(copies);
       row.rounds.push({pending,elapsedMs:performance.now()-start,output});output=[];copies=[];
     }
     decoder.reset();row.resetState=decoder.state;
     decoder.configure(c);decoder.decode(samples[0]);const pending=decoder.flush();decoder.reset();
     try{await pending;row.abort='unexpected resolution'}catch(e){row.abort=e.name}
     row.outputAfterReset=output.length;decoder.close();row.closedState=decoder.state;
   }
   return result;
 },fixtures);
 fs.writeFileSync(path.join(__dirname,label+'-film.json'),JSON.stringify({version:b.version(),results},null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
