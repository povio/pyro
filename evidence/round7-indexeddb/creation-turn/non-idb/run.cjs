const fs = require('node:fs');
const net = require('node:net');
const cp = require('node:child_process');
const assert = require('node:assert/strict');
const pw = require(process.env.PW_CORE);
(async () => {
  const [arm, binary] = process.argv.slice(2);
  const socket = net.createServer();
  await new Promise(resolve => socket.listen(0, '127.0.0.1', resolve));
  const port = socket.address().port;
  await new Promise(resolve => socket.close(resolve));
  const start = new Date().toISOString();
  const proc = cp.spawn(binary, ['serve','--host','127.0.0.1','--port',String(port)], {stdio:'ignore'});
  const exited = new Promise(resolve => proc.once('exit',resolve));
  try {
    const endpoint = 'http://127.0.0.1:' + port;
    for (let i=0;i<100;i++) {
      try { if ((await fetch(endpoint+'/json/version')).ok) break; } catch {}
      await new Promise(resolve => setTimeout(resolve,100));
    }
    const browser = await pw.chromium.connectOverCDP(endpoint);
    const page = await browser.newPage();
    const trace = await page.evaluate('('+fs.readFileSync('fixture.js','utf8')+')()');
    fs.writeFileSync(arm+'.json',JSON.stringify({arm,start,end:new Date().toISOString(),trace},null,2));
    assert.deepEqual(trace,JSON.parse(fs.readFileSync('expected.json')));
    await browser.close();
  } finally { proc.kill(); await exited; }
})().catch(error => { console.error(error); process.exitCode=1; });
