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
      const samples = {};
      for (const count of [4, 32, 128]) {
        for (const kind of ['append', 'reorder', 'reinsert']) {
          const rows = [];
          for (let sample = 0; sample < 4; sample++) {
            await page.evaluate(count => {
              document.head.textContent = '';
              const make = id => {
                const sheet = document.createElement('style');
                sheet.textContent = '#target { --marker: s' + id + '; }';
                sheet.dataset.marker = 's' + id;
                return sheet;
              };
              for (let i = 0; i < count; i++) document.head.append(make(i));
              window.makeSheet = make;
              getComputedStyle(document.querySelector('#target')).getPropertyValue('--marker');
            }, count);
            const cpu = treeCpuMs(server.pid);
            const startUtc = new Date().toISOString();
            const start = performance.now();
            const result = await page.evaluate(({count, kind}) => {
              const head = document.head, target = document.querySelector('#target');
              let mismatches = 0, reads = 0;
              const check = () => {
                const value = getComputedStyle(target).getPropertyValue('--marker').trim();
                if (value !== head.lastElementChild.dataset.marker) mismatches++;
                reads++;
              };
              const begin = performance.now();
              for (let i = 0; i < 16; i++) {
                if (kind === 'append') head.append(window.makeSheet(count + i));
                else if (kind === 'reorder') head.append(head.firstElementChild);
                else {
                  const sheet = head.firstElementChild;
                  sheet.remove();
                  check();
                  head.insertBefore(sheet, head.children[Math.floor(count / 2)]);
                }
                check();
              }
              return {elapsedMs:performance.now()-begin, mismatches, reads};
            }, {count, kind});
            const wallMs = performance.now() - start;
            const endUtc = new Date().toISOString();
            if (sample) rows.push({startUtc,endUtc,wallMs,cpuMs:treeCpuMs(server.pid)-cpu,...result});
          }
          samples[kind + '-' + count] = rows;
        }
      }
      return samples;
    } finally {await b.close();}
  } finally {server.kill();}
}
(async()=>{for(const binary of process.argv.slice(2)) console.log(JSON.stringify({binary,startUtc:new Date().toISOString(),samples:await run(binary),endUtc:new Date().toISOString()}));})().catch(e=>{console.error(e);process.exitCode=1;});
