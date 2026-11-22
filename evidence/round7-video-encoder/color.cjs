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
 const bytes=[...fs.readFileSync(path.join(w,'smpte170m-red.h264'))];
 const values=await page.evaluate(async bytes=>{
   const out={};for(const [name,fields] of [['source',{}],['empty',{colorSpace:{}}],['override',{colorSpace:{primaries:'bt709',transfer:'bt709',matrix:'bt709',fullRange:false}}]]){
     const cfg={codec:'avc1.42c01e',codedWidth:64,codedHeight:48,...fields},r=out[name]={};
     r.supported=(await VideoDecoder.isConfigSupported(cfg)).supported;if(!r.supported)continue;
     const dec=new VideoDecoder({output(f){
       const c=document.createElement('canvas');c.width=64;c.height=48;const ctx=c.getContext('2d');ctx.drawImage(f,0,0);
       r.colorSpace=f.colorSpace.toJSON();r.format=f.format;r.pixel=[...ctx.getImageData(20,20,1,1).data];f.close();
     },error(e){r.error=e.name}});
     try{dec.configure(cfg);dec.decode(new EncodedVideoChunk({type:'key',timestamp:12345,data:new Uint8Array(bytes)}));await dec.flush()}catch(e){r.error=e.name}finally{if(dec.state!=='closed')dec.close()}
   }return out;
 },bytes);
 fs.writeFileSync(path.join(w,label+'-color.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
