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
      const supported=await page.evaluate(()=>CSS.supports('reading-flow','source-order'));
      const samples={};
      for(const kind of (supported?['ordinary','reading']:['ordinary'])){
        const times=[];let cpuStart;
        for(let i=0;i<6;i++){
          if(i===1)cpuStart=treeCpuMs(server.pid);
          const utc=new Date().toISOString(),start=performance.now();
          const ok=await page.evaluate(kind=>{
            const host=document.getElementById('target');
            host.textContent='';
            for(let j=0;j<64;j++)host.append(document.createElement('div'));
            let valid=true;
            for(let round=0;round<8;round++){
              for(const el of host.children){
                if(kind==='ordinary') el.style.order=String(round);
                else {el.style.readingFlow=round%2?'grid-rows':'source-order';el.style.readingOrder=String(round);}
              }
              for(const el of host.children){
                const cs=getComputedStyle(el);
                valid=valid && (kind==='ordinary'?cs.order===String(round):
                  cs.readingOrder===String(round)&&cs.readingFlow===(round%2?'grid-rows':'source-order'));
              }
            }
            return valid;
          },kind);
          if(!ok)throw Error('cascade result mismatch');
          if(i)times.push({utc,ms:performance.now()-start});
        }
        samples[kind]={times,cpuMs:treeCpuMs(server.pid)-cpuStart};
      }
      return samples;
    } finally {await b.close();}
  } finally {server.kill();}
}
(async()=>{for(const binary of process.argv.slice(2)) console.log(JSON.stringify({binary,startUtc:new Date().toISOString(),samples:await run(binary),endUtc:new Date().toISOString()}));})().catch(e=>{console.error(e);process.exitCode=1;});
