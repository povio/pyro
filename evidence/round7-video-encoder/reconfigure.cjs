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
   const out={available:typeof VideoEncoder};if(typeof VideoEncoder==='function'){
     const cfg={codec:'avc1.42001e',width:64,height:48,framerate:30,bitrate:192000,latencyMode:'realtime',avc:{format:'avc'}};
     if(!(await VideoEncoder.isConfigSupported(cfg)).supported)return out;
     const rows=[],enc=new VideoEncoder({output(c,m){const d=m.decoderConfig;rows.push({timestamp:c.timestamp,type:c.type,config:d?{codedWidth:d.codedWidth,codec:d.codec,description:[...new Uint8Array(d.description)].slice(0,4)}:null});if(d?.description)new Uint8Array(d.description).fill(0)},error(e){throw e}});
     let i=0;for(const w of [64,64,80,80]){if(i===3)enc.reset();enc.configure({...cfg,width:w});const bytes=new Uint8Array(w*48*4);for(let o=0;o<bytes.length;o+=4){bytes[o+1]=190;bytes[o+3]=255}
       const f=new VideoFrame(bytes,{format:'RGBA',codedWidth:w,codedHeight:48,timestamp:i++*33333});enc.encode(f,{keyFrame:true});f.close();await enc.flush();}
     enc.close();out.rows=rows;
   }
   return out;
 });
 fs.writeFileSync(path.join(w,label+'-reconfigure.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
