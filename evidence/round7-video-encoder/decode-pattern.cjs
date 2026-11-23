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
 const input=JSON.parse(fs.readFileSync(process.argv[4],'utf8'));
 const values=await page.evaluate(async input=>{
   const out={};
   for(const format of ['avc','annexb']){
     if(!input[format]?.supported)continue;
     const rows=[],errors=[],decoder=new VideoDecoder({output(f){
       const canvas=document.createElement('canvas');canvas.width=f.displayWidth;canvas.height=f.displayHeight;const ctx=canvas.getContext('2d');ctx.drawImage(f,0,0);
       const colors=[0.25,0.75].map(x=>[...ctx.getImageData(Math.floor(x*canvas.width),Math.floor(canvas.height/2),1,1).data]);
       rows.push({timestamp:f.timestamp,width:f.codedWidth,height:f.codedHeight,colors,format:f.format,colorSpace:f.colorSpace.toJSON()});f.close();
     },error(e){errors.push(e.name)}});
     try{for(const p of input[format].packets){
       if(p.config){const cfg={...p.config};if(cfg.description)cfg.description=new Uint8Array(cfg.description);decoder.configure(cfg)}
       decoder.decode(new EncodedVideoChunk({type:p.type,timestamp:p.timestamp,...(p.duration==null?{}:{duration:p.duration}),data:new Uint8Array(p.data)}));
     }
     await decoder.flush();}catch(e){errors.push(e.name)}finally{if(decoder.state!=='closed')decoder.close()}out[format]={rows,errors};
   }
   return out;
 },input);
 fs.writeFileSync(path.join(w,label+'-decoded.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
