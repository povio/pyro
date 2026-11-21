const {chromium}=require('playwright-core');
const {spawn}=require('node:child_process');
const {createServer}=require('node:net');
const {readFileSync}=require('node:fs');
const {homedir}=require('node:os');
const wpt=process.env.WPT_CHECKOUT, launcher=process.env.CHROME_LAUNCHER;
if (!wpt || !launcher) throw new Error('Set WPT_CHECKOUT and CHROME_LAUNCHER');
async function main() {
 const certs=wpt+'/tools/certs/web-platform.test';
 const site=Bun.serve({port:0,hostname:'127.0.0.1',tls:{cert:readFileSync(certs+'.pem'),key:readFileSync(certs+'.key')},fetch:()=>new Response('<!doctype html><title>CPU tensor roundtrip</title>',{headers:{'content-type':'text/html'}})});
 const free=createServer();await new Promise(r=>free.listen(0,'127.0.0.1',r));const port=free.address().port;await new Promise(r=>free.close(r));
 const proc=spawn(launcher,['serve','--port',String(port)],{detached:true,stdio:'ignore'});
 let browser;
 try{
  const ep='http://127.0.0.1:'+port;
  for(let i=0;i<250;i++){if(await fetch(ep+'/json/version').then(r=>r.ok,()=>false))break;await Bun.sleep(40);}
  browser=await chromium.connectOverCDP(ep);
  const ctx=await browser.newContext({viewport:{width:800,height:600},deviceScaleFactor:1});
  const page=await ctx.newPage();await page.goto('https://web-platform.test:'+site.port+'/');
  const result=await page.evaluate(async()=>{
    const result={origin:location.origin,secure:isSecureContext,exposed:'ml' in navigator,contexts:[]};
    if(!navigator.ml)return result;
    for(const option of ['accelerated','deviceType']){
      const members=[];const options={};Object.defineProperty(options,option,{get(){members.push(option);return option==='accelerated'?false:'cpu';}});
      const context=await navigator.ml.createContext(options);
      const record={option,members,reportedAccelerated:context.accelerated??null,reportedDevices:context.devices??null,reportedDeviceType:context.deviceType??null};result.contexts.push(record);
      try{
        if (!members.includes(option)||context.accelerated===true)continue;
        const build=new MLGraphBuilder(context);
        const x=build.input('x',{dataType:'float32',shape:[1,2]});
        const w=build.constant({dataType:'float32',shape:[3,2]},new Float32Array([1,2,-1,1,.5,-2]));
        const b=build.constant({dataType:'float32',shape:[3]},new Float32Array([-1,.5,2]));
        const graph=await build.build({y:build.relu(build.gemm(x,w,{bTranspose:true,c:b}))});
        const inp=await context.createTensor({dataType:'float32',shape:[1,2],writable:true});
        const out=await context.createTensor({dataType:'float32',shape:[1,3],readable:true});
        record.predictions=[];
        for(const row of [[2,3],[-1,2],[1,-2]]){
          context.writeTensor(inp,new Float32Array(row));context.dispatch(graph,{x:inp},{y:out});
          record.predictions.push(Array.from(new Float32Array(await context.readTensor(out))));
        }
        record.outputShape=out.shape;
        inp.destroy();out.destroy();graph.destroy();
      }finally{context.destroy();}
    }
    return result;
  });
  console.log(JSON.stringify({browser:browser.version(),...result}));await ctx.close();
 }finally{await browser?.close().catch(()=>{});try{process.kill(-proc.pid,'SIGKILL')}catch{};site.stop(true);}
}
main().catch(e=>{console.error(e);process.exitCode=1});
