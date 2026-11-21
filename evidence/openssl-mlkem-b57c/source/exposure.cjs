const { chromium }=require('playwright-core');
const {spawn}=require('node:child_process');
const {createServer}=require('node:net');
async function main(){
  const binary=process.argv[2]; const enabled=process.argv[3]!=='off';
  const body= '<!doctype html><meta charset=utf-8><title>CPU binding exposure</title>';
  const server=Bun.serve({port:0,hostname:'127.0.0.1',fetch(r){
    if(new URL(r.url).pathname.endsWith('worker.js'))return new Response("postMessage({secure:isSecureContext,encapsulate:typeof crypto.subtle?.encapsulateBits,decapsulate:typeof crypto.subtle?.decapsulateBits})",{headers:{'content-type':'text/javascript'}});
    return new Response(body,{headers:{'content-type':'text/html'}});
  }});
  const sock=createServer();await new Promise(r=>sock.listen(0,'127.0.0.1',r));const port=sock.address().port;await new Promise(r=>sock.close(r));
  const proc=spawn(binary,['serve','--host','127.0.0.1','--port',String(port),'--cdp-max-connections','5'],{detached:true,stdio:'ignore'});
  let browser;
  try{
    const ep='http://127.0.0.1:'+port;for(let i=0;i<250;i++){if(await fetch(ep+'/json/version').then(r=>r.ok,()=>false))break;await Bun.sleep(40);}
    browser=await chromium.connectOverCDP(ep);
    const results=[];
    for(const secure of [false,true]){
      const ctx=await browser.newContext(secure?{}:{proxy:{server:'http://127.0.0.1:'+server.port}});
      const page=await ctx.newPage();await page.goto(secure?server.url.href:'http://normal-ml-probe.test/');
      const info=await page.evaluate(async()=>{
        const w=new Worker('/worker.js'); const worker=await new Promise((resolve,reject)=>{w.onmessage=e=>resolve(e.data);w.onerror=reject;});w.terminate();
        return {window:{secure:isSecureContext,encapsulate:typeof crypto.subtle?.encapsulateBits,decapsulate:typeof crypto.subtle?.decapsulateBits},worker};
      });results.push(info);await ctx.close();
    }
    console.log(JSON.stringify(results));
    for(let i=0;i<results.length;i++)for(const r of Object.values(results[i])){
      if(r.secure!==Boolean(i) || r.encapsulate!==(i&&enabled?'function':'undefined') || r.decapsulate!==(i&&enabled?'function':'undefined'))throw Error('unexpected exposure');
    }
  }finally{await browser?.close().catch(()=>{});try{process.kill(-proc.pid,'SIGKILL')}catch{};server.stop(true);}
}
main().catch(e=>{console.error(e);process.exitCode=1});
