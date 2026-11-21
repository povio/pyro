const fs=require('node:fs'),path=require('node:path'),cp=require('node:child_process'),http=require('node:http'),net=require('node:net');
const pw=require(process.env.PLAYWRIGHT_CORE);
const [label,bin]=process.argv.slice(2);
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{let browser,proc;const server=http.createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end('<!doctype html><title>Inert HTML document control</title>')});
 try{
 const port=await listen(server);
 if(bin==='chrome')browser=await pw.chromium.launch({executablePath:process.env.CHROME_BIN,headless:true});
 else{const sock=net.createServer(),bp=await listen(sock);await new Promise(r=>sock.close(r));proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(bp),'--log-level','error'],{stdio:'ignore'});const host='http://127.0.0.1:'+bp;for(let i=0;i<120;i++){try{if((await fetch(host+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100));}browser=await pw.chromium.connectOverCDP(host)}
 const page=await browser.newPage();await page.goto('http://127.0.0.1:'+port+'/');
 const results=await page.evaluate(async()=>{
  const results={};
  const parse = html => new DOMParser().parseFromString(html, 'text/html');
  const markup = '<script>window.importExecutions++</script>';
  window.importExecutions = 0;
  for (const patched of [false, true]) {
    const doc = parse(patched ? '<body><main><?marker name="s"></main><template for="s">'+markup+'</template>' : '<body><main>'+markup+'</main>');
    const script = doc.querySelector('main script');
    results[patched ? 'patchedScriptFound' : 'ordinaryScriptFound'] = !!script;
    if (script) document.body.appendChild(document.importNode(script,true));
    results[patched ? 'afterPatchedImport' : 'afterOrdinaryImport'] = window.importExecutions;
  }
  const template = '<template for="h"><meta id="m"></template>';
  const head = '<head><?marker name="h"></head>';
  results.bodyToHead = !!parse(head + '<body>'+template).head.querySelector('#m');
  results.nestedBodyToHead = !!parse(head + '<body><div>'+template+'</div>').head.querySelector('#m');
  return results;
 });
 fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify(results,null,2));console.log(label,JSON.stringify(results));
 }finally{if(browser)await browser.close();if(proc)proc.kill();server.close()}
})().catch(e=>{console.error(e);process.exitCode=1});
