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
      const samples={};
      for(const kind of ['ordinaryCssom','typedWriteRead','declaredIteration']) {
        const times=[]; let cpuStart;
        for(let i=0;i<6;i++) {
          if(i===1)cpuStart=treeCpuMs(server.pid);
          const utc=new Date().toISOString();
          const start=performance.now();
          const ok=await page.evaluate(kind=>{
            document.body.innerHTML='<section></section>';
            const host=document.querySelector('section');
            host.innerHTML='<i></i>'.repeat(100);
            let sum=0;
            for (let j=0; j<20; j++) {
              for (const el of host.children) {
                if (kind === 'ordinaryCssom') {
                  el.style.width = (20 + j) + 'px';
                  sum += el.style.width.length;
                } else if (kind === 'typedWriteRead') {
                  const map = el.attributeStyleMap;
                  map.set('opacity', CSS.percent(20 + j));
                  sum += String(map.get('opacity')).length;
                } else {
                  el.style.cssText = 'width:2px; --shade:navy; background-size:10px auto; height:3px';
                  for (const [name, values] of el.attributeStyleMap) sum += name.length + values.length;
                }
              }
            }
            return sum>0;
          },kind);
          if(!ok)throw Error('write verification failed');
          if(i)times.push({utc,ms:performance.now()-start});
        }
        samples[kind]={times,cpuMs:treeCpuMs(server.pid)-cpuStart};
      }
      return samples;
    } finally {await b.close();}
  } finally {server.kill();}
}
(async()=>{for(const binary of process.argv.slice(2)) console.log(JSON.stringify({binary,startUtc:new Date().toISOString(),samples:await run(binary),endUtc:new Date().toISOString()}));})().catch(e=>{console.error(e);process.exitCode=1;});
