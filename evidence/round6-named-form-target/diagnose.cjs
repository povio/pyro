const http = require('node:http'), fs=require('node:fs'), cp=require('node:child_process'), net=require('node:net'), path=require('node:path');
const pw=require(path.resolve(__dirname,'../manifest-runner/bench/node_modules/playwright-core'));
const [label,bin]=process.argv.slice(2), requests=[];
const server=http.createServer(async(req,res)=>{
 if(req.url.startsWith('/echo')){
   const chunks=[];for await(const piece of req)chunks.push(piece);
   const text=Buffer.concat(chunks).toString();requests.push({method:req.method,type:req.headers['content-type'],text});
   res.writeHead(200,{'Content-Type':'text/plain;charset=UTF-8'});res.end(text);return;
 }
 res.writeHead(200,{'Content-Type':'text/html;charset=UTF-8'});res.end('<!doctype html><body>Parent page</body>');
});
const listen=s=>new Promise(r=>s.listen(0,'127.0.0.1',()=>r(s.address().port)));
(async()=>{
 const port=await listen(server),sock=net.createServer(),bp=await listen(sock);await new Promise(r=>sock.close(r));let proc,browser;
 try {
 if(bin==='chrome')browser=await pw.chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless:true});
 else{
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(bp),'--log-level','error','--load-resources','iframe'],{stdio:'ignore'});
 for(let i=0;i<100;i++){try{if((await fetch('http://127.0.0.1:'+bp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 browser=await pw.chromium.connectOverCDP('http://127.0.0.1:'+bp)}
 const pages=[];
 for(const src of [null,'','about:blank','/frame']){
 const page=await browser.newPage();await page.goto('http://127.0.0.1:'+port+'/parent');
 await page.evaluate(src=>{
  const f=document.createElement('iframe');f.name='normal-form-result';if(src!==null)f.src=src;
  window.trace=[];const snap=(kind)=>trace.push({kind,url:f.contentDocument?.URL,state:f.contentDocument?.readyState,body:f.contentDocument?.body?.textContent});
  f.addEventListener('load',()=>snap('load'));document.body.append(f);snap('after-append');
  const form=document.createElement('form');form.action='/echo';form.method='post';form.target=f.name;form.enctype='multipart/form-data';
  const input=document.createElement('input');input.name='sample';input.value='hello';form.append(input);document.body.append(form);
  form.submit();snap('after-submit');f.onload=()=>snap('late-load');
 },src);
 await page.waitForTimeout(500);pages.push({src,trace:await page.evaluate(()=>trace)});await page.close();
 }
 fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify({pages,requests},null,2));
 }finally{if(browser)await browser.close();if(proc)proc.kill();server.close()}
})().catch(e=>{console.error(e);process.exit(1)});
