const fs=require('fs'), path=require('path'),cp=require('child_process'),net=require('net');
const pw=require('playwright-core');
const [label,bin]=process.argv.slice(2);
(async()=>{
 let browser,proc;
 try {
 if(bin==='chrome')browser=await pw.chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless:true});
 else {
 const server=net.createServer();await new Promise(r=>server.listen(0,'127.0.0.1',r));const port=server.address().port;await new Promise(r=>server.close(r));
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{stdio:'ignore'});
 const url='http://127.0.0.1:'+port;
 for(let i=0;i<100;i++){try{if((await fetch(url+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 browser=await pw.chromium.connectOverCDP(url)}
 const page=await browser.newPage();await page.goto('data:text/html,<html><body></body></html>');
 const result=await page.evaluate(()=>{
 const w=640,h=360;const c=new OffscreenCanvas(w,h).getContext('2d',{colorSpace:'srgb',willReadFrequently:true});
 const out={};for(const space of ['srgb','display-p3']){
  const buf=new Uint8ClampedArray(w*h*4);
  for(let i=0;i<buf.length;i+=4){const index=i>>>2;buf.set([64+index%73,84+index%61,92+index%49,255],i)}
  const img=new ImageData(buf,w,h,{colorSpace:space});const times=[],checks=[];
  for(let rep=0;rep<11;rep++){
    const start=performance.now();c.putImageData(img,0,0);const result=c.getImageData(0,0,w,h,{colorSpace:space});times.push(performance.now()-start);
    let checksum=0;for(let i=0;i<result.data.length;i+=1021)checksum+=result.data[i];checks.push(checksum);
  }
  out[space]={samplesMs:times,checks};
 }
 return out;
 });
 fs.writeFileSync(path.resolve(__dirname,label+'.json'),JSON.stringify(result,null,2))
 }finally{if(browser)await browser.close();if(proc)proc.kill()}
})().catch(e=>{console.error(e);process.exit(1)});
