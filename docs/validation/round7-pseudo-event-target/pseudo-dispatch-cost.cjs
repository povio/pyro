const path = require('node:path');
const net = require('node:net');
const { spawn, execFileSync } = require('node:child_process');
const { performance } = require('node:perf_hooks');
const { chromium } = require(path.join(process.env.HOME, 'Library/Caches/lightningpanda/round6-reference-158/census/runner/node_modules/playwright-core'));
function treeCpuMs(root) {
  const rows=execFileSync('ps',['-axo','pid=,ppid=,time='],{encoding:'utf8'}).trim().split('\n').map(line=>line.trim().split(/\s+/));
  const wanted=new Set([String(root)]);
  for (let n=0;n<rows.length;n++) {
    const count=wanted.size;
    for(const [pid,parent] of rows) if(wanted.has(parent)) wanted.add(pid);
    if(count===wanted.size)break;
  }
  return rows.reduce((total,[pid,_,clock])=>total+(wanted.has(pid)?clock.split(':').reduce((all,v)=>all*60+Number(v),0)*1000:0),0);
}
async function run(binary) {
  const listener = net.createServer();
  await new Promise(resolve => listener.listen(0,'127.0.0.1',resolve));
  const port = listener.address().port;
  await new Promise(resolve => listener.close(resolve));
  const server = spawn(binary,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{stdio:'ignore'});
  try {
    const url = 'http://127.0.0.1:'+port;
    let ready = false;
    for (let i=0;i<100;i++) {
      try {ready = (await fetch(url+'/json/version')).ok;} catch {}
      if (ready) break;
      await new Promise(resolve => setTimeout(resolve,50));
    }
    if (!ready) throw new Error('CDP did not start');
    const b=await chromium.connectOverCDP(url);
    try {
      const page=await b.newPage();
      await page.setContent('<!doctype html><div id="target"></div>');
      await page.setContent('<style>body{margin:0}div{width:64px;height:32px}#pseudo::before{display:block;content:"";width:24px;height:24px}</style><div id="ordinary"></div><div id="pseudo"></div>');
      await page.evaluate(()=>{
        window.count=0;window.generated=0;
        document.addEventListener('pointermove',e=>{window.count++;if(e.pseudoTarget)window.generated++});
      });
      const cdp=await page.context().newCDPSession(page);
      const samples={};
      for(const kind of ['ordinary','pseudo']){
        const times=[];let cpuStart;
        for(let i=0;i<5;i++){
          if(i===1)cpuStart=treeCpuMs(server.pid);
          await page.evaluate(()=>{window.count=0;window.generated=0});
          const utc=new Date().toISOString(),start=performance.now();
          for(let j=0;j<96;j++)await cdp.send('Input.dispatchMouseEvent',{type:'mouseMoved',x:5+(j%12),y:kind==='ordinary'?10:42});
          const ms=performance.now()-start;
          const read=await page.evaluate(()=>[window.count,window.generated]);
          if(read[0]!==96)throw Error('unexpected delivered pointermoves '+read);
          if(i)times.push({utc,ms,delivered:read[0],generated:read[1]});
        }
        samples[kind]={times,cpuMs:treeCpuMs(server.pid)-cpuStart};
      }
      return samples;
    } finally {await b.close();}
  } finally {server.kill();}
}
(async()=>{for(const binary of process.argv.slice(2)) console.log(JSON.stringify({binary,startUtc:new Date().toISOString(),samples:await run(binary),endUtc:new Date().toISOString()}));})().catch(e=>{console.error(e);process.exitCode=1;});
