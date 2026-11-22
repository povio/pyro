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
      const cdp=await page.context().newCDPSession(page);
      const samples={};
      for(const kind of ['normal','source','flex','grid']){
        await page.evaluate(kind=>{
          document.body.innerHTML='<button id="start">start</button><section id="owner"></section><button id="end">end</button>';
          const owner=document.querySelector('#owner');
          owner.style.cssText=({normal:'',source:'reading-flow:source-order',flex:'display:flex;reading-flow:flex-flow',grid:'display:grid;grid-template-columns:repeat(8,20px);reading-flow:grid-columns'})[kind];
          for(let i=0;i<48;i++){
            const button=document.createElement('button');button.id='b'+i;button.textContent=i;
            if((kind==='source'||kind==='flex')&&i===4)button.style.readingOrder='-1';
            if(kind==='flex'&&i===2)button.style.order='-1';
            owner.append(button);
          }
          getComputedStyle(owner).display;
        },kind);
        const times=[];let cpuStart;
        for(let i=0;i<5;i++){
          await page.evaluate(()=>document.getElementById('start').focus());
          if(i===1)cpuStart=treeCpuMs(server.pid);
          const utc=new Date().toISOString(),start=performance.now();
          let first;
          for(let j=0;j<64;j++){
            const modifiers=j<32?0:8;
            await cdp.send('Input.dispatchKeyEvent',{type:'rawKeyDown',key:'Tab',code:'Tab',windowsVirtualKeyCode:9,modifiers});
            await cdp.send('Input.dispatchKeyEvent',{type:'keyUp',key:'Tab',code:'Tab',windowsVirtualKeyCode:9,modifiers});
            if(!j)first=await page.evaluate(()=>document.activeElement.id);
          }
          const ms=performance.now()-start;
          const last=await page.evaluate(()=>document.activeElement.id);
          if(last!=='start') throw Error('did not return to start: '+last);
          if(i)times.push({utc,ms,first,last});
        }
        samples[kind]={times,cpuMs:treeCpuMs(server.pid)-cpuStart};
      }
      return samples;
    } finally {await b.close();}
  } finally {server.kill();}
}
(async()=>{for(const binary of process.argv.slice(2)) console.log(JSON.stringify({binary,startUtc:new Date().toISOString(),samples:await run(binary),endUtc:new Date().toISOString()}));})().catch(e=>{console.error(e);process.exitCode=1;});
