const http = require('node:http'), fs=require('node:fs'), cp=require('node:child_process'), net=require('node:net'), path=require('node:path');
const pw=require(path.resolve(__dirname,'../manifest-runner/bench/node_modules/playwright-core'));
const [label,bin]=process.argv.slice(2), requests=[];
const zlib=require('node:zlib');
const text=Array.from({length:6500},(_,i)=>`Transaction ${i} delivered ${(i*3701)%1099} records from service ${i%23}.\n`).join('');
const compressed=zlib.brotliCompressSync(Buffer.from(text));
const server=http.createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end('<!doctype html><body>Streaming check</body>')});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{
 const port=await listen(server),sock=net.createServer(),bp=await listen(sock);await new Promise(r=>sock.close(r));let proc,browser;
 try {
 if(bin==='chrome')browser=await pw.chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless:true});
 else{
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(bp),'--log-level','error','--load-resources','iframe'],{stdio:'ignore'});
 for(let i=0;i<100;i++){try{if((await fetch('http://127.0.0.1:'+bp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 browser=await pw.chromium.connectOverCDP('http://127.0.0.1:'+bp)}
 const pages=[];
 const page=await browser.newPage();await page.goto('http://127.0.0.1:'+port+'/');
 for(const width of [1,7,1024,16384,compressed.length]){
  const item=await page.evaluate(async ({encoded,text,width})=>{
   let ds;
   try { ds=new DecompressionStream("brotli"); } catch(e){return {width,error:e.name}}
   const packed=Uint8Array.from(encoded), chunks=[];
   for(let i=0;i<packed.length;i+=width)chunks.push(packed.subarray(i,i+width));
   const stream=new ReadableStream({start(c){for(const chunk of chunks)c.enqueue(chunk);c.close()}}).pipeThrough(ds);
   const reader=stream.getReader(), decoder=new TextDecoder(),parts=[];let total=0;
   for(;;){const {done,value}=await reader.read();if(done)break;total+=value.length;parts.push(decoder.decode(value,{stream:true}));}
   parts.push(decoder.decode());return {width,encoded:encoded.length,decoded:total,matches:parts.join('')===text};
  },{encoded:[...compressed],text,width});pages.push(item);
 }
 const progressive=await page.evaluate(async ({encoded,text})=>{
  let ds;try{ds=new DecompressionStream('brotli')}catch(e){return {error:e.name}}
  const packed=Uint8Array.from(encoded),mid=Math.floor(packed.length/2);
  const r=ds.readable.getReader(),w=ds.writable.getWriter(),pending=r.read();
  const firstWrite=w.write(packed.subarray(0,mid));
  const first=await Promise.race([pending,new Promise(resolve=>setTimeout(()=>resolve(null),3000))]);
  if(!first)return {beforeLastChunk:false};
  await firstWrite;const last=w.write(packed.subarray(mid));const close=w.close();
  const parts=[first.value];for(;;){const next=await r.read();if(next.done)break;parts.push(next.value)}
  await last;await close;
  const decoder=new TextDecoder(),actual=parts.map(p=>decoder.decode(p,{stream:true})).join('')+decoder.decode();
  return {beforeLastChunk:first.value.length,decoded:actual.length,matches:actual===text};
 },{encoded:[...compressed],text});pages.push({progressive});await page.close();
 fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify({pages,requests},null,2));
 }finally{if(browser)await browser.close();if(proc)proc.kill();server.close()}
})().catch(e=>{console.error(e);process.exit(1)});
