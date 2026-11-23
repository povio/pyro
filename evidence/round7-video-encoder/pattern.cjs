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
   const out={available:typeof VideoEncoder};
   if(typeof VideoEncoder==='function'){
     for(const format of ['avc','annexb']){
       const cfg={codec:'avc1.42001e',width:64,height:48,bitrate:192000,framerate:30,latencyMode:'realtime',avc:{format}};
       const support=await VideoEncoder.isConfigSupported(cfg);const row=out[format]={supported:support.supported};if(!support.supported)continue;
       const packets=[],errors=[];const encoder=new VideoEncoder({output(c,m){
         const bytes=new Uint8Array(c.byteLength);c.copyTo(bytes);let config;
         if(m.decoderConfig){const d=m.decoderConfig;config={...d};
           if(d.description)config.description=[...new Uint8Array(d.description)]}
         packets.push({type:c.type,timestamp:c.timestamp,duration:c.duration,data:[...bytes],config});
       },error(e){errors.push(e.name)}});
       encoder.configure(cfg);let i=0;
       for(const [w,h] of [[64,48],[80,48]]){
         if(i)encoder.configure({...cfg,width:w,height:h});
         for(let j=0;j<5;j++,i++){
           const data=new Uint8Array(w*h*4);
           for(let y=0;y<h;y++)for(let x=0;x<w;x++){const idx=4*(y*w+x);data[idx+(x<w/2?i%3:(i+1)%3)]=230;data[idx+3]=255}
           const frame=new VideoFrame(data,{format:'RGBA',codedWidth:w,codedHeight:h,timestamp:100000+i*33333,duration:33333});
           encoder.encode(frame,{keyFrame:j===0||j===3});frame.close();
         }
         await encoder.flush();
       }
       row.state=[encoder.state,encoder.encodeQueueSize];row.errors=errors;row.packets=packets;encoder.close();
     }
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-pattern.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
