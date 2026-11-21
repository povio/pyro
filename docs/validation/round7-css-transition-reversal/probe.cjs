const {chromium} = require('playwright-core');
const {spawn} = require('node:child_process');
const fs = require('node:fs');
const http = require('node:http');
(async () => {
  const server = http.createServer((req,res) => res.end('<!doctype html><body></body>'));
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = process.argv[3];
  const child = spawn(process.argv[2], ['serve','--port',port], {stdio:'ignore'});
  try {
    const endpoint = 'http://127.0.0.1:'+port;
    for (let tries=0; tries<100; tries++) {
      try { if ((await fetch(endpoint+'/json/version')).ok) break; } catch {}
      await new Promise(resolve => setTimeout(resolve,50));
    }
    const browser = await chromium.connectOverCDP(endpoint);
    const page = await browser.newPage();
    await page.goto('http://127.0.0.1:'+server.address().port);
    page.on('console', msg => console.log(msg.text()));
    await page.evaluate(fs.readFileSync(__dirname+'/control.js','utf8'));
    await browser.close();
  } finally { child.kill(); server.close(); }
})().catch(error => {console.error(error); process.exitCode=1;});
