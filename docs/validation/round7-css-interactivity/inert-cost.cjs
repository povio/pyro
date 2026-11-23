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
      await page.setContent('<style>button{position:absolute;left:0;top:0;width:80px;height:40px}#start{left:90px}</style><button id="start">start</button><section><button id="target">needle</button></section>');
      const cdp=await page.context().newCDPSession(page);
      const samples={};
      for(const kind of ['ordinary','inert','override']){
        await page.evaluate(kind=>{
          const target=document.querySelector('#target');
          target.parentNode.style.interactivity=kind==='ordinary'?'auto':'inert';
          target.style.interactivity=kind==='override'?'auto':'';
          window.hits=0;target.onpointermove=()=>window.hits++;
          document.querySelector('#start').focus();getComputedStyle(target).interactivity;
        },kind);
        const times=[];let cpuStart;
        for(let i=0;i<5;i++){
          if(i===1)cpuStart=treeCpuMs(server.pid);
          const utc=new Date().toISOString(),start=performance.now();
          const read=await page.evaluate(()=>{
            const el=document.querySelector('#target'),start=document.querySelector('#start');
            let focused=0,found=0;
            for(let j=0;j<64;j++){start.focus();el.focus();focused+=document.activeElement===el;
              getSelection().removeAllRanges();found+=window.find('needle');}
            window.hits=0;
            return {focused,found};
          });
          for(let j=0;j<32;j++)await cdp.send('Input.dispatchMouseEvent',{type:'mouseMoved',x:5+j%8,y:8});
          read.hits=await page.evaluate(()=>window.hits);
          const ms=performance.now()-start;
          if(i)times.push({utc,ms,...read});
        }
        samples[kind]={times,cpuMs:treeCpuMs(server.pid)-cpuStart};
      }
      return samples;
    } finally {await b.close();}
  } finally {server.kill();}
}
(async()=>{for(const binary of process.argv.slice(2)) console.log(JSON.stringify({binary,startUtc:new Date().toISOString(),samples:await run(binary),endUtc:new Date().toISOString()}));})().catch(e=>{console.error(e);process.exitCode=1;});
