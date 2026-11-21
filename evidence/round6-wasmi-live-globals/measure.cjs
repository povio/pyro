const cp=require('node:child_process'),fs=require('node:fs'),os=require('node:os'),path=require('node:path'),net=require('node:net'),crypto=require('node:crypto');
const {chromium}=require(require.resolve('playwright-core',{paths:[os.homedir()+'/Povio/lightningpanda/bench']}));
const engines=JSON.parse(process.argv[2]),q=__dirname, corpus=path.join(os.homedir(),'Library/Caches/lightningpanda/round6-wasm-bench');
const files={audio:fs.readFileSync(corpus+'/modules/audio.wasm'), bridge:fs.readFileSync(q+'/fixtures/bridge.wasm'), gain:fs.readFileSync(q+'/fixtures/gain-driver.wasm')};
const expected=JSON.parse(fs.readFileSync(corpus+'/expected.json')).audio;
const server=Bun.serve({hostname:'127.0.0.1',port:0,fetch(req){const k=new URL(req.url).pathname.slice(1);return files[k]?new Response(files[k],{headers:{'content-type':'application/wasm'}}):new Response('<!DOCTYPE html>',{headers:{'content-type':'text/html'}})}});
async function trial(lab,bin,round){
 const port=await new Promise(res=>{const s=net.createServer();s.listen(0,'127.0.0.1',()=>{const n=s.address().port;s.close(()=>res(n))})});
 const child=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error','--load-resources','stylesheet','--load-resources','iframe','--cdp-max-connections','64'],{stdio:'ignore',detached:true,env:{...process.env,WPT_CHROME_STATE_DIR:q+'/chrome-state'}});
 const start=performance.now(),endpoint='http://127.0.0.1:'+port;
 let browser;
 try{
  for(let i=0;i<300;i++){try{if((await fetch(endpoint+'/json/version')).ok)break}catch{};await Bun.sleep(35)}
  browser=await chromium.connectOverCDP(endpoint);
  const ctx=await browser.newContext(),page=await ctx.newPage();await page.goto('http://127.0.0.1:'+server.port+'/');const startup=performance.now()-start;
  const ans=await page.evaluate(async ({expected,round})=>{
   const mods={};for(const k of ['audio','bridge','gain'])mods[k]=new WebAssembly.Module(await(await fetch('/'+k)).arrayBuffer());
   const audio=new WebAssembly.Instance(mods.audio).exports.run;let callbacks=0;
   const kernel=x=>{callbacks++;return audio(x)};
   const ordinary=new WebAssembly.Instance(mods.bridge,{h:{hook:kernel}}).exports.call;
   const stages={direct:audio,viaHost:ordinary};
   const offered=typeof WebAssembly.Global==='function';let global;
   if(offered){global=new WebAssembly.Global({value:'i32',mutable:true},0);const driver=new WebAssembly.Instance(mods.gain,{h:{g:global,kernel}}).exports.run;stages.liveGlobal=seed=>{global.value=seed;return driver()}}
   const output={},names=Object.keys(stages);
   for(let n=0;n<names.length;n++){
    const phase=names[(n+round)%names.length],call=stages[phase];const samples=[],batch=32;
    for(let r=-2;r<5;r++){const before=performance.now(),cb=callbacks;for(let i=0;i<batch;i++){const seed=i%8;
     if ((call(seed)>>>0)!==expected[seed])throw Error(phase+' oracle mismatch');
    }
    const elapsed=performance.now()-before;
    if(callbacks-cb!==(phase==='direct'?0:batch))throw Error('callback count mismatch');
    if(r>=0)samples.push(elapsed);
    }
    output[phase]={batch,samples};
   }
   return {stages:output,liveGlobalOffered:offered,callbacks};
  },{expected,round});
  await ctx.close();return {lab,round,startupMs:startup,load:os.loadavg(),...ans};
 }finally{await browser?.close().catch(()=>{});try{process.kill(-child.pid,'SIGKILL')}catch{}}
}
(async()=>{
 const rows=[],labs=Object.keys(engines);
 try{for(let round=0;round<5;round++)for(let j=0;j<labs.length;j++){const lab=labs[(j+round)%labs.length];rows.push(await trial(lab,engines[lab],round));fs.writeFileSync(q+'/application-results.json',JSON.stringify(rows,null,2)+'\n');console.log(round,lab)}}
 finally{server.stop(true)}
})().catch(e=>{console.error(e.message);process.exitCode=1});
