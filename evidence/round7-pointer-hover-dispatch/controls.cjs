const fs = require('node:fs');
const {spawn} = require('node:child_process');
const net = require('node:net');
const {chromium} = require(process.env.PLAYWRIGHT_CORE);

async function launch(binary) {
  if (binary === 'chrome') {
    return {browser: await chromium.launch({executablePath: process.env.CHROME_BIN, headless:true})};
  }
  const alloc = net.createServer();
  await new Promise(resolve => alloc.listen(0, '127.0.0.1', resolve));
  const port = alloc.address().port;
  await new Promise(resolve => alloc.close(resolve));
  const proc = spawn(binary, ['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'], {stdio:'ignore'});
  const url = 'http://127.0.0.1:' + port;
  for (let i=0;i<100;i++) {
    try {
      if ((await fetch(url+'/json/version')).ok) return {browser: await chromium.connectOverCDP(url), proc};
    } catch {}
    await new Promise(resolve=>setTimeout(resolve,70));
  }
  proc.kill(); throw new Error('Browser did not become ready');
}
(async()=>{
  const [label,binary] = process.argv.slice(2);
  const {browser,proc} = await launch(binary);
  try {
    const page = await browser.newPage();
    await page.setContent('<body style="margin:0"><div id="host" style="position:absolute;left:30px;top:30px;width:60px;height:60px"></div></body>');
    await page.evaluate(()=>{
      const host = document.querySelector('#host');
      const root = host.attachShadow({mode:'open'});
      root.innerHTML='<div id="inside" style="width:60px;height:60px;background:green"></div>';
      window.rows=[];
      for (const type of ['pointerenter','pointerleave','pointerover','pointerout','pointermove','pointerdown','pointerup','mouseenter','mouseleave','mousemove','mousedown','mouseup']) {
        root.firstChild.addEventListener(type, e=>rows.push([e.type,e.button,e.buttons,e.composed,e.isTrusted]));
      }
    });
    await page.mouse.move(5,5); await page.mouse.move(40,40);
    await page.mouse.down({button:'right'}); await page.mouse.move(42,42);
    await page.mouse.up({button:'right'}); await page.mouse.move(5,5);
    const rows = await page.evaluate(()=>window.rows);
    fs.writeFileSync(label+'.json',JSON.stringify(rows,null,1)+'\n');
    console.log(label,rows.length);
  } finally {await browser.close(); if(proc)proc.kill();}
})().catch(e=>{console.error(e);process.exitCode=1;});
