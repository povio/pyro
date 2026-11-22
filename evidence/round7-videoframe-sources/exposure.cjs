const http=require('node:http'),fs=require('node:fs'),net=require('node:net'),path=require('node:path'),cp=require('node:child_process'),os=require('node:os');
const w=__dirname,ref=path.join(path.dirname(w),'round6-reference-158/census'),pw=require(path.join(ref,'runner/node_modules/playwright-core'));
const [label,explicit]=process.argv.slice(2);
const server=http.createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end('<!doctype html><title>Exposure</title>');});
const insecureHost=Object.values(os.networkInterfaces()).flat().find(a=>a.family==="IPv4"&&!a.internal)?.address;
if(!insecureHost)throw Error("No local IPv4 interface for insecure HTTP origin");
const local=http.createServer(server.listeners("request")[0]);
const localPort=()=>new Promise(r=>local.listen(0,insecureHost,()=>r(local.address().port)));
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{const port=await listen(server),otherPort=await localPort(),sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));let proc,b;
 try{const bin=explicit||path.join(ref,'launch-158');
 const tmp=fs.mkdtempSync(path.join(os.tmpdir(),'r7-audio-'));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:tmp,WPT_CHROME_REFERENCE_MODE:'wpt-stable'}});
 for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp);const page=await b.newPage();const values={};
 for(const [context,host] of [['secure','127.0.0.1'],['insecure',insecureHost]]){
   await page.goto('http://'+host+':'+(context==='secure'?port:otherPort)+'/surface');
   values[context]=await page.evaluate(()=>{
     const row={origin:location.origin,secureContext:isSecureContext,interfaces:{}};
     for(const key of ['AudioPlaybackStats','ImageDecoder','ImageTrackList','ImageTrack']){
       row.interfaces[key]={present:key in globalThis,type:typeof globalThis[key]};
     }
     return row;
   });
 }
 fs.writeFileSync(path.join(w,label+'-exposure.json'),JSON.stringify(values,null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close();local.close()}})().catch(e=>{console.error(e);process.exitCode=1});
