const { chromium } = require('playwright-core');
const { spawn } = require('node:child_process');
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const urls = JSON.parse(fs.readFileSync(path.join(__dirname, 'urls.json')));
const url = 'css/css-animations/display-none-prevents-starting-in-subtree.html';
if (!urls.includes(url)) throw Error('not in positive manifest');
const server = http.createServer((req, res) => {
  const pathname = new URL(req.url, 'http://localhost').pathname.slice(1);
  if (![url, 'resources/testharness.js', 'resources/testharnessreport.js', 'css/css-animations/support/testcommon.js'].includes(pathname)) { res.writeHead(404).end(); return; }
  res.setHeader('Content-Type', pathname.endsWith('.js') ? 'application/javascript' : 'text/html');
  res.end(fs.readFileSync(path.join(process.env.WPT_DIR, pathname)));
});
(async () => {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const socket = http.createServer();
  await new Promise(resolve => socket.listen(0, '127.0.0.1', resolve));
  const port = socket.address().port;
  await new Promise(resolve => socket.close(resolve));
  const child = spawn(process.argv[2], ['serve','--host','127.0.0.1','--port',String(port)], {stdio:'ignore'});
  try {
    const endpoint = `http://127.0.0.1:${port}`;
    for (let i=0;i<200;i++) { try { if ((await fetch(endpoint+'/json/version')).ok) break; } catch {} await new Promise(resolve=>setTimeout(resolve,25)); }
    const browser = await chromium.connectOverCDP(endpoint);
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${server.address().port}/${url}`);
    const result = await page.evaluate(async () => {
      const sheet = document.createElement('style');
      sheet.textContent = '@keyframes laneFade { to { opacity:.25 } } .laneTarget {animation: laneFade 80s linear infinite} .laneHidden { display:none }';
      document.head.append(sheet);
      const root = document.createElement('section');
      root.innerHTML = '<section><i class="laneTarget"></i></section><i class="laneTarget"></i>';
      document.body.append(root);
      const [left,right] = root.querySelectorAll('i');
      const [leftRun,rightRun] = [left,right].map(el=>el.getAnimations()[0]);
      await Promise.all([leftRun.ready,rightRun.ready]);
      left.parentElement.className = 'laneHidden';
      const ancestor = [leftRun.playState,rightRun.playState,right.getAnimations()[0]===rightRun,root.getAnimations({subtree:true}).length];
      left.parentElement.className = '';
      const replacement = left.getAnimations()[0];
      await replacement.ready;
      left.classList.add('laneHidden');
      const own = [replacement.playState,rightRun.playState,right.getAnimations()[0]===rightRun,root.getAnimations({subtree:true}).length];
      return {ancestor,own,freshAfterShow:replacement!==leftRun};
    });
    console.log(JSON.stringify(result));
    await browser.close();
  } finally { child.kill('SIGKILL'); server.close(); }
})().catch(error=>{console.error(error);process.exit(1)});
