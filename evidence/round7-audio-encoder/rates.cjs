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
     const base={codec:'opus',sampleRate:8000,numberOfChannels:1,bitrateMode:'constant'};
     out.variants={};
     for(const [key,opt] of [['omitted',{}],['empty',{opus:{}}],['explicit',{opus:{complexity:4}}]]){
       const r=await AudioEncoder.isConfigSupported({...base,bitrate:32000,...opt});
       out.variants[key]={supported:r.supported,opus:r.config.opus??null};
     }
     for(const [name,bitrate] of [['normal',32000],['high',256000]]){
       const cfg={...base,bitrate,opus:{frameDuration:120000}};
       const supported=(await AudioEncoder.isConfigSupported(cfg)).supported,entry=out[name]={supported,bitrate};
       if(supported){
         const packets=[],errors=[],enc=new AudioEncoder({output(c){packets.push([c.byteLength,c.duration])},error(e){errors.push(e.name)}});
         try{
           enc.configure(cfg);
           for(let i=0;i<4;i++){const data=Float32Array.from({length:960},(_,j)=>0.4*Math.sin(2*Math.PI*440*(i*960+j)/8000));
             const d=new AudioData({format:'f32',sampleRate:8000,numberOfChannels:1,numberOfFrames:960,timestamp:i*120000,data});enc.encode(d);d.close();}
           await enc.flush();
         }catch(e){errors.push(e.name)}finally{if(enc.state!=='closed')enc.close()}
         entry.packets=packets;entry.errors=errors;entry.deliveredBitrate=packets.length?8e6*packets.reduce((n,p)=>n+p[0],0)/packets.reduce((n,p)=>n+p[1],0):null;
       }
     }
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-rates.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
