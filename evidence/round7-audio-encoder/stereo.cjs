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
   const out={available:typeof AudioEncoder};
   if(typeof AudioEncoder==='function'){
     const config={codec:'opus',sampleRate:44100,numberOfChannels:2,bitrate:128000},packets=[];
     out.support=(await AudioEncoder.isConfigSupported(config)).supported;
     if(!out.support)return out;
     const encoder=new AudioEncoder({output(c,m){packets.push([c,m])},error(e){throw e}});
     encoder.configure(config);
     for(let i=0;i<20;i++){
       const data=new Float32Array(2*882);
       for(let j=0;j<882;j++)for(let k=0;k<2;k++)data[2*j+k]=(k?0.2:0.4)*Math.sin(2*Math.PI*(k?660:330)*(i*882+j)/44100);
       const audio=new AudioData({format:'f32',data,sampleRate:44100,numberOfChannels:2,numberOfFrames:882,timestamp:20000*i});encoder.encode(audio);audio.close();
     }
     await encoder.flush();encoder.close();
     const samples=[[],[]],decoder=new AudioDecoder({output(d){out.rate=d.sampleRate;out.channels=d.numberOfChannels;for(let k=0;k<2;k++){const buf=new Float32Array(d.numberOfFrames);d.copyTo(buf,{planeIndex:k,format:'f32-planar'});samples[k].push(...buf)}d.close()},error(e){throw e}});
     decoder.configure(packets[0][1].decoderConfig);for(const [p] of packets)decoder.decode(p);await decoder.flush();decoder.close();out.packetCount=packets.length;
     out.tones=samples.map(channel=>{const clip=channel.slice(2400,-2400);let crossings=0,power=0;for(let i=0;i<clip.length;i++){power+=clip[i]*clip[i];if(i&&clip[i-1]<=0&&clip[i]>0)crossings++}return {samples:channel.length,rms:Math.sqrt(power/clip.length),frequency:out.rate*crossings/clip.length}});
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-stereo.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
