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
      for(const kind of ['ordinary','functional','rotating','signed']) {
        const times=[]; let cpuStart;
        for(let i=0;i<6;i++) {
          if(i===1)cpuStart=treeCpuMs(server.pid);
          const utc=new Date().toISOString();
          const start=performance.now();
          const ok=await page.evaluate(kind=>{
            const el=document.getElementById('target');
            el.setAttribute('style', kind==='ordinary' ? 'color:red;display:block' :
                kind==='functional' || kind==='rotating' ? 'color:rgb(10,20,30);transform:translateX(3px);--pad:2px;padding:var(--pad)' :
                'z-index:clamp(-1, 1 / sign(-0em), 1);display:block');
            for(let j=0;j<3000;j++) {
              if (kind==='rotating') {
                const names=['width','height','left','top','fontSize','lineHeight','letterSpacing','borderTopWidth'];
                el.style[names[j%names.length]]=(j%13+1)+'px';
              } else el.style.opacity=j%2 ? '0.5' : '0.75';
              el.style.marginLeft=(j%10)+'px';
            }
            return el.style.marginLeft==='9px';
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
