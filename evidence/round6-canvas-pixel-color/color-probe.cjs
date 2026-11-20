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
 const result=await page.evaluate(async()=>{
 const spaces=['srgb','display-p3','srgb-linear','display-p3-linear'];
 const output={};
 for(const sourceSpace of spaces) {
   const ctx=new OffscreenCanvas(3,1).getContext('2d',{colorSpace:'srgb'});
   try{
    const source=new ImageData(new Uint8ClampedArray([80,149,105,255,104,137,129,128,255,0,0,255]),3,1,{colorSpace:sourceSpace});
    ctx.putImageData(source,0,0,0,0,2,1);
    output[sourceSpace]={};
    for(const destSpace of spaces){
     try{output[sourceSpace][destSpace]=Array.from(ctx.getImageData(0,0,3,1,{colorSpace:destSpace}).data)}
     catch(e){output[sourceSpace][destSpace]=e.name}
    }
    const bitmap=await createImageBitmap(source,1,0,1,1);
    ctx.clearRect(0,0,3,1);ctx.drawImage(bitmap,0,0);bitmap.close();
    output[sourceSpace].croppedBitmap=Array.from(ctx.getImageData(0,0,1,1,{colorSpace:'srgb'}).data);
   }catch(e){output[sourceSpace]=e.name}
 }
 return output;
 });
 fs.writeFileSync(path.resolve(__dirname,label+'.json'),JSON.stringify(result,null,2))
 }finally{if(browser)await browser.close();if(proc)proc.kill()}
})().catch(e=>{console.error(e);process.exit(1)});
