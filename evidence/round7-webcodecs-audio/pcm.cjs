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
   const out={surface:typeof AudioDecoder};
   if(typeof AudioDecoder==='function'){
     for(const [kind,C,values] of [['u8',Uint8Array,[0,1,127,128,254,255]],['s16',Int16Array,[-32768,-1,0,1,8192,32767]],['s32',Int32Array,[-2147483648,-16777217,-1,0,16777217,2147483647]],['f32',Float32Array,[-1,-0.5,0,0.5,1,2]]]){
       const config={codec:'pcm-'+kind,sampleRate:44100,numberOfChannels:1},record={};
       const decoder=new AudioDecoder({output(d){
         const dest=new C(values.length);d.copyTo(dest,{planeIndex:0,format:kind});
         record.output={format:d.format,samples:[...dest],timestamp:d.timestamp};d.close();
       },error(e){record.error=e.name}});
       decoder.configure(config);decoder.decode(new EncodedAudioChunk({type:'key',timestamp:123,data:new C(values)}));await decoder.flush();decoder.close();out[kind]=record;
     }
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-pcm.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
