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
  if(typeof VideoFrame!=='function')return {status:'VideoFrame unavailable'};
  const width=640,height=360,area=width*height;
  const outputs={};
  for(const format of ['RGBA','I420']){
   const source=new Uint8Array(format==='RGBA'?4*area:area+area/2);
   for(let i=0;i<source.length;i++) source[i]=format==='RGBA'&&i%4===3?255:(i*29+7)%256;
   const rgb=new Uint8Array(area*4), raw=new Uint8Array(source.length), times=[];
   let total=0,formatChecksum=0;
   for(let rep=0;rep<15;rep++){
    const start=performance.now(),frame=new VideoFrame(source,{format,codedWidth:width,codedHeight:height,timestamp:rep*33000});
    const copy=frame.clone();frame.close();await copy.copyTo(raw);await copy.copyTo(rgb,{format:'RGBA',colorSpace:'srgb'});copy.close();
    times.push(performance.now()-start);
    for(let i=rep;i<source.length;i+=1021) if(source[i]!==raw[i])throw new Error('raw copy mismatch');
    total+=rgb[4*((rep*457)%area)];
    if(rep===0) for(let j=0;j<rgb.length;j++)formatChecksum=(formatChecksum+((j%251)+1)*rgb[j])>>>0;
   }
   outputs[format]={width,height,samplesMs:times,rawBytes:raw.length,rgbBytes:rgb.length,checksum:formatChecksum,sampleSum:total};
  }
  return {status:'complete',outputs};
 });
 fs.writeFileSync(path.resolve(__dirname,label+'.json'),JSON.stringify(result,null,2))
 }finally{if(browser)await browser.close();if(proc)proc.kill()}
})().catch(e=>{console.error(e);process.exit(1)});
