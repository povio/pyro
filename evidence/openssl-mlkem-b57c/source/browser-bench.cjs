const {chromium}=require('playwright-core');
const {spawn}=require('node:child_process');
const {createServer}=require('node:net');
async function main(){
 const socket=createServer();await new Promise(r=>socket.listen(0,'127.0.0.1',r));const port=socket.address().port;await new Promise(r=>socket.close(r));
 const web=Bun.serve({hostname:'127.0.0.1',port:0,fetch:()=>new Response('<!doctype html><title>ML-KEM timing</title>',{headers:{'content-type':'text/html'}})});
 const child=spawn(process.argv[2],['serve','--host','127.0.0.1','--port',String(port),'--cdp-max-connections','4'],{detached:true,stdio:'ignore'});
 let browser;
 try{
  const ep='http://127.0.0.1:'+port;for(let n=0;n<200;n++){if(await fetch(ep+'/json/version').then(r=>r.ok,()=>false))break;await Bun.sleep(50);}
  browser=await chromium.connectOverCDP(ep);const ctx=await browser.newContext();const page=await ctx.newPage();await page.goto(web.url.href);
  const data=await page.evaluate(async()=>{
   const out=[];
   for(const level of [512,768,1024]){
    const name='ML-KEM-'+level, samples=[], n=32;
    for(let batch=0;batch<6;batch++){
     const keys=[],boxes=[], start=performance.now();
     for(let i=0;i<n;i++)keys.push(await crypto.subtle.generateKey(name,false,['encapsulateBits','decapsulateBits']));
     const generated=performance.now();
     for(const key of keys)boxes.push(await crypto.subtle.encapsulateBits(name,key.publicKey));
     const encapsulated=performance.now();
     for(let i=0;i<n;i++){
      const shared=await crypto.subtle.decapsulateBits(name,keys[i].privateKey,boxes[i].ciphertext);
      const a=new Uint8Array(shared),b=new Uint8Array(boxes[i].sharedKey);
      if(a.length!==32 || a.length!==b.length || !a.every((v,j)=>v===b[j]))throw new Error('agreement failed');
     }
     const finished=performance.now();
     if(batch)samples.push({keygen:(generated-start)*1000/n,encapsulate:(encapsulated-generated)*1000/n,decapsulateAndCompare:(finished-encapsulated)*1000/n});
    }
    const median=k=>samples.map(s=>s[k]).sort((a,b)=>a-b)[2];
    out.push({name,iterationsPerBatch:n,measuredBatches:samples.length,medianMeanUs:{keygen:median('keygen'),encapsulate:median('encapsulate'),decapsulateAndCompare:median('decapsulateAndCompare')},samples});
   }
   return out;
  });console.log(JSON.stringify(data));await ctx.close();
 }finally{await browser?.close().catch(()=>{});try{process.kill(-child.pid,'SIGKILL')}catch{}web.stop(true);}
}
main().catch(e=>{console.error(e);process.exitCode=1});
