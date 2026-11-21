const http = require('node:http'), fs=require('node:fs'), cp=require('node:child_process'), net=require('node:net'), path=require('node:path');
const pw=require(path.resolve(__dirname,'../../round6-reference-158/census/runner/node_modules/playwright-core'));
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
 if(bin==='chrome')browser=await pw.chromium.launch({executablePath:path.resolve(__dirname,'../../round6-reference-158/Chrome-158-stable.app/Contents/MacOS/Google Chrome'),headless:true});
 else{
 proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(bp),'--log-level','error','--load-resources','iframe'],{stdio:'ignore'});
 for(let i=0;i<100;i++){try{if((await fetch('http://127.0.0.1:'+bp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 browser=await pw.chromium.connectOverCDP('http://127.0.0.1:'+bp)}
 const pages=[];
 for(const mode of ['back','forward','cancel','move','clear','other','image','br','dl','emoji']){
 const page=await browser.newPage();await page.goto('http://127.0.0.1:'+port+'/parent');
 await page.evaluate(mode=>{
 document.body.innerHTML='<div contenteditable id=a></div><div contenteditable id=b>xyz</div>';
 const a=document.querySelector('#a'), b=document.querySelector('#b');a.innerHTML=mode==='dl'?'<dl><dt>first</dt></dl><dl><dd>second</dd></dl>':mode==='image'?'a<img>b':mode==='br'?'a<br>b':mode==='emoji'?'a🌟b':'abcdef';
 a.focus();const text=mode==='dl'?a.querySelector('dd').firstChild:a.firstChild;const offset=mode==='dl'?0: mode==='emoji'?3:mode==='image'||mode==='br'?1:2;
 getSelection().collapse(text,offset);
 const point=(n,o)=>[n.nodeName,n.nodeType===3?n.data:n.innerHTML,o];const snapshot=e=>e.getTargetRanges().map(r=>({isStatic:r instanceof StaticRange,start:point(r.startContainer,r.startOffset),end:point(r.endContainer,r.endOffset)}));
 window.events=[];a.addEventListener('beforeinput',e=>{window.saved=e;events.push([e.type,e.inputType,snapshot(e)]);
 if(mode==='cancel')e.preventDefault();if(mode==='move')getSelection().collapse(text,4);if(mode==='clear')getSelection().removeAllRanges();if(mode==='other'){b.focus();getSelection().collapse(b.firstChild,1)}
 },{once:true});a.addEventListener('input',e=>events.push([e.type,e.inputType,snapshot(e)]),{once:true});
 },mode);
 await page.keyboard.press(['forward','clear','other','image','br'].includes(mode)?'Delete':'Backspace');
 pages.push({mode,...await page.evaluate(()=>({a:a.innerHTML,b:b.innerHTML,events,after:window.saved?.getTargetRanges().length}))});await page.close();}
 fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify({pages,requests},null,2));
 }finally{if(browser)await browser.close();if(proc)proc.kill();server.close()}
})().catch(e=>{console.error(e);process.exit(1)});
