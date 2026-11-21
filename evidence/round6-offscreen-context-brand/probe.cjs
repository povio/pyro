const fs=require('node:fs'),path=require('node:path'),cp=require('node:child_process'),http=require('node:http'),net=require('node:net');
const pw=require(path.resolve(__dirname,'../../../round6-reference-158/census/runner/node_modules/playwright-core'));
const [label,bin]=process.argv.slice(2);
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{let browser,proc;const server=http.createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end('<!doctype html><title>Crypto API control</title>')});
 try{
 const port=await listen(server);
 if(bin==='chrome')browser=await pw.chromium.launch({executablePath:path.resolve(__dirname,'../../../round6-reference-158/Google Chrome.app/Contents/MacOS/Google Chrome'),headless:true});
 else{const sock=net.createServer(),bp=await listen(sock);await new Promise(r=>sock.close(r));proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(bp),'--log-level','error'],{stdio:'ignore'});const host='http://127.0.0.1:'+bp;for(let i=0;i<120;i++){try{if((await fetch(host+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100));}browser=await pw.chromium.connectOverCDP(host)}
 const page=await browser.newPage();await page.goto('http://127.0.0.1:'+port+'/');
 const results=await page.evaluate(async()=>{
  const results={};
  const c=document.createElement('canvas');c.width=2;c.height=2;
  const el=c.getContext('2d'),o=new OffscreenCanvas(2,2),x=o.getContext('2d');
  const off=OffscreenCanvasRenderingContext2D.prototype,on=CanvasRenderingContext2D.prototype;
  results.brands=[Object.getPrototypeOf(x)===off,x instanceof OffscreenCanvasRenderingContext2D,x instanceof CanvasRenderingContext2D,el instanceof OffscreenCanvasRenderingContext2D,Object.prototype.toString.call(x),x.canvas===o];
  results.prototypes=[Object.getPrototypeOf(off)===Object.prototype,off.constructor===OffscreenCanvasRenderingContext2D,typeof off.drawFocusIfNeeded,typeof on.drawFocusIfNeeded,off.fillRect===on.fillRect];
  const invoke=(fn,receiver)=>{try{fn.call(receiver,0,0,1,1);return 'accepted'}catch(e){return e.name}};
  results.crossBorrow=[invoke(off.fillRect,el),invoke(on.fillRect,x)];
  x.fillStyle='rgb(255,0,0)';x.fillRect(0,0,2,2);
  const dest=new OffscreenCanvas(2,2),dx=dest.getContext('2d');dx.drawImage(o,0,0);
  const pixel=t=>Array.from(t.getImageData(0,0,1,1).data);
  results.copied=pixel(dx);const image=o.transferToImageBitmap();el.drawImage(image,0,0);results.transferred=pixel(el);results.reset=pixel(x);
  const encoded=await dest.convertToBlob({type:'image/png'});const b=await createImageBitmap(encoded);x.drawImage(b,0,0);results.blob=[encoded.type,pixel(x)];b.close();image.close();
  o.width=3;results.resize=[x.canvas.width,pixel(x)];
  const wsrc="onmessage=()=>{const o=new OffscreenCanvas(2,2),x=o.getContext('2d');x.fillStyle='lime';x.fillRect(0,0,1,1);postMessage([typeof CanvasRenderingContext2D, Object.getPrototypeOf(x)===OffscreenCanvasRenderingContext2D.prototype,x.canvas===o,Array.from(x.getImageData(0,0,1,1).data)])}";
  const url=URL.createObjectURL(new Blob([wsrc],{type:'text/javascript'})),w=new Worker(url);
  try{results.worker=await new Promise((resolve,reject)=>{w.onmessage=e=>resolve(e.data);w.onerror=e=>reject(new Error(e.message));w.postMessage(1)})}finally{w.terminate();URL.revokeObjectURL(url)}
  return results;
 });
 fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify(results,null,2));console.log(label,JSON.stringify(results));
 }finally{if(browser)await browser.close();if(proc)proc.kill();server.close()}
})().catch(e=>{console.error(e);process.exitCode=1});
