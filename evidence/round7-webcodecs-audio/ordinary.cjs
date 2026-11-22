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
   const out={surface:[typeof AudioData,typeof AudioDecoder,typeof EncodedAudioChunk]};
   const sample=new Int16Array([8192,-16384,24576,0,-8192,16384]);
   const config={codec:'pcm-s16',sampleRate:48000,numberOfChannels:2};
   const bytes=new Uint8Array(sample.buffer),chunk=new EncodedAudioChunk({type:'key',timestamp:7000,data:bytes});
   const copied=new Uint8Array(chunk.byteLength);chunk.copyTo(copied);out.chunk=[chunk.timestamp,chunk.type,...copied];
   function read(data){
     const samples=[];
     for(let channel=0;channel<data.numberOfChannels;channel++){
       const opts={planeIndex:channel,format:'f32-planar'},buf=new Float32Array(data.allocationSize(opts)/4);
       data.copyTo(buf,opts);samples.push([...buf]);
     }
     return {rate:data.sampleRate,channels:data.numberOfChannels,frames:data.numberOfFrames,timestamp:data.timestamp,duration:data.duration,samples};
   }
   if(typeof AudioData==='function'){
     const pcm=new AudioData({format:'s16',numberOfChannels:2,numberOfFrames:3,sampleRate:48000,timestamp:7000,data:bytes});
     out.pcm=read(pcm);const clone=pcm.clone(),moved=structuredClone(pcm,{transfer:[pcm]});
     out.transfer=[pcm.numberOfFrames,read(clone),read(moved)];clone.close();moved.close();
   }
   if(typeof AudioDecoder==='function'){
     out.support=(await AudioDecoder.isConfigSupported(config)).supported;
     out.unknown=(await AudioDecoder.isConfigSupported({...config,codec:'unknown'})).supported;
     if(out.support){
       const outputs=[],errors=[],decoder=new AudioDecoder({output(d){outputs.push(read(d));d.close()},error(e){errors.push(e.name)}});
       decoder.configure(config);decoder.decode(chunk);await decoder.flush();out.decoded=outputs;
       decoder.reset();out.reset=decoder.state;decoder.close();out.closed=[decoder.state,errors];
     }
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-ordinary.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
