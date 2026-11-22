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
 const source=JSON.parse(fs.readFileSync(process.argv[4],'utf8'));
 const values=await page.evaluate(async payload=>{
   const cfg=payload.packets.find(p=>p.config)?.config;
   if(!cfg)throw Error('No decoder config in encoded output');
   if(cfg.description)cfg.description=new Uint8Array(cfg.description);
   const a=[],buffers=[],errors=[];const decoder=new AudioDecoder({output(frame){
     const buf=new Float32Array(frame.numberOfFrames);frame.copyTo(buf,{planeIndex:0,format:'f32-planar'});buffers.push(buf);a.push([frame.timestamp,frame.duration,frame.numberOfChannels,frame.sampleRate,frame.numberOfFrames]);frame.close();
   },error(e){errors.push(e.name)}});
   decoder.configure(cfg);
   for(const p of payload.packets)decoder.decode(new EncodedAudioChunk({type:p.type,timestamp:p.timestamp,...(p.duration==null?{}:{duration:p.duration}),data:new Uint8Array(p.data)}));
   await decoder.flush();decoder.close();
   const samples=buffers.flatMap(b=>[...b]);const middle=samples.slice(2400,-2400);
   let peak=0,power=0,crossings=0;for(let i=0;i<middle.length;i++){const x=middle[i];power+=x*x;peak=Math.max(peak,Math.abs(x));if(i&&middle[i-1]<=0&&x>0)crossings++}
   return {errors,frames:a,totalSamples:samples.length,peak,rms:Math.sqrt(power/middle.length),frequency:crossings*48000/middle.length};
 },source);
 fs.writeFileSync(path.join(w,label+'-tone-decoded.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
