const http=require('http'),net=require('net'),cp=require('child_process'),fs=require('fs'),path=require('path');
const cache=path.resolve(__dirname,'../..'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
const [label,exe]=process.argv.slice(2);const listen=s=>new Promise(resolve=>s.listen(0,'127.0.0.1',()=>resolve(s.address().port)));
(async()=>{const server=http.createServer((req,res)=>res.end('<body>')),port=await listen(server);let proc,b;try{
 if(label==='chrome')b=await pw.chromium.launch({executablePath:path.join(cache,'round6-reference-158/Google Chrome.app/Contents/MacOS/Google Chrome'),headless:true});
 else{const sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));proc=cp.spawn(exe,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error'],{stdio:'ignore'});for(let i=0;i<100;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,80));} b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp)}
 const page=await b.newPage();await page.goto('http://127.0.0.1:'+port);
 const results=await page.evaluate(async()=>{
   const image=new ImageData(new Uint8ClampedArray([1,23,45,255]),1,1,{colorSpace:'display-p3'});
   const chunk=new EncodedVideoChunk({type:'delta',timestamp:100,data:new Uint8Array([9,10])});
   const side=new Uint8Array([55]);const channel=new MessageChannel();
   const result=new Promise((resolve,reject)=>{
     channel.port2.onmessageerror=()=>reject(Error('unexpected messageerror'));
     channel.port2.onmessage=({data})=>resolve({
       chunkAlias:data.chunk===data.list[0], imageAlias:data.image===data.list[1],
       pixelAlias:data.image.data===data.bytes, colorSpace:data.image.colorSpace,
       pixels:Array.from(data.image.data),side:data.side[0],sideDetached:side.byteLength===0
     });
   });
   channel.port1.postMessage({chunk,image,list:[chunk,image],bytes:image.data,side},{transfer:[side.buffer]});
   image.data[1]=99;const rows=await result;channel.port1.close();channel.port2.close();return rows
 });fs.writeFileSync(path.join(__dirname,label+'-mixed.json'),JSON.stringify({version:b.version(),results},null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
