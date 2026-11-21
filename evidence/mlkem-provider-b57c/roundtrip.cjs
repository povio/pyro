const {chromium}=require('playwright-core');
const {spawn}=require('node:child_process');
const {createServer}=require('node:net');
const {readFileSync}=require('node:fs');
const {homedir}=require('node:os');
const wpt=process.env.WPT_CHECKOUT;
if(!wpt || !process.argv[2])throw Error('Set WPT_CHECKOUT and pass the Chrome launcher');
async function main() {
 const certs=wpt+'/tools/certs/web-platform.test';
 const site=Bun.serve({port:0,hostname:'127.0.0.1',tls:{cert:readFileSync(certs+'.pem'),key:readFileSync(certs+'.key')},fetch:()=>new Response('<!doctype html><title>ML-KEM key agreement</title>',{headers:{'content-type':'text/html'}})});
 const free=createServer();await new Promise(r=>free.listen(0,'127.0.0.1',r));const port=free.address().port;await new Promise(r=>free.close(r));
 const proc=spawn(process.argv[2],['serve','--port',String(port)],{detached:true,stdio:'ignore'});
 let browser;
 try{
  const ep='http://127.0.0.1:'+port;
  for(let i=0;i<250;i++){if(await fetch(ep+'/json/version').then(r=>r.ok,()=>false))break;await Bun.sleep(40);}
  browser=await chromium.connectOverCDP(ep);
  const ctx=await browser.newContext({viewport:{width:800,height:600},deviceScaleFactor:1});
  const page=await ctx.newPage();await page.goto('https://web-platform.test:'+site.port+'/');
  const result=await page.evaluate(async()=>{
    const result={origin:location.origin,secure:isSecureContext,methods:['encapsulateBits','decapsulateBits','encapsulateKey','decapsulateKey'].map(name=>[name,typeof crypto.subtle[name]]),variants:[]};
    for(const name of ['ML-KEM-512','ML-KEM-768','ML-KEM-1024']){
      try{
        const keys=await crypto.subtle.generateKey(name,false,['encapsulateBits','decapsulateBits']);
        const {sharedKey,ciphertext}=await crypto.subtle.encapsulateBits(name,keys.publicKey);
        const recovered=await crypto.subtle.decapsulateBits(name,keys.privateKey,ciphertext);
        result.variants.push({name,publicUsages:keys.publicKey.usages,privateUsages:keys.privateKey.usages,ciphertextBytes:ciphertext.byteLength,sharedBytes:sharedKey.byteLength,match:sharedKey.byteLength===recovered.byteLength && new Uint8Array(sharedKey).every((x,i)=>x===new Uint8Array(recovered)[i])});
      } catch(e){result.variants.push({name,error:e.name,message:e.message});}
    }
    return result;
  });
  console.log(JSON.stringify({browser:browser.version(),...result}));await ctx.close();
 }finally{await browser?.close().catch(()=>{});try{process.kill(-proc.pid,'SIGKILL')}catch{};site.stop(true);}
}
main().catch(e=>{console.error(e);process.exitCode=1});
