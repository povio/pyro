const http=require('http'),net=require('net'),cp=require('child_process'),fs=require('fs'),path=require('path');
const cache=path.resolve(__dirname,'..'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
const [label,exe]=process.argv.slice(2);const listen=s=>new Promise(resolve=>s.listen(0,'127.0.0.1',()=>resolve(s.address().port)));
(async()=>{const wpt=path.join(cache,'wptci/wpt-full/css/cssom-view');
const server=http.createServer((req,res)=>{const u=req.url;if(u.startsWith('/support/')){res.setHeader('content-type','image/png');res.end(fs.readFileSync(path.join(wpt,u)));return}res.setHeader('content-type','text/html');res.end(fs.readFileSync(path.join(wpt,'image-x-y-zoom.html'),'utf8').replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi,''))}),port=await listen(server);let proc,b;try{
 if(label==='chrome')b=await pw.chromium.launch({executablePath:path.join(cache,'round6-reference-158/Google Chrome.app/Contents/MacOS/Google Chrome'),headless:true});
 else{const sock=net.createServer(),cdp=await listen(sock);await new Promise(r=>sock.close(r));proc=cp.spawn(exe,['serve','--host','127.0.0.1','--port',String(cdp),'--log-level','error'],{stdio:'ignore'});for(let i=0;i<100;i++){try{if((await fetch('http://127.0.0.1:'+cdp+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,80));} b=await pw.chromium.connectOverCDP('http://127.0.0.1:'+cdp)}
 const page=await b.newPage();await page.goto('http://127.0.0.1:'+port);
 const results=await page.evaluate('('+fs.readFileSync(path.join(__dirname,'zoom-direct.js'),'utf8')+')()');fs.writeFileSync(path.join(__dirname,'zoom-direct-'+label+'.json'),JSON.stringify({version:b.version(),results},null,2));
 }finally{if(b)await b.close();if(proc)proc.kill();server.close()}})().catch(e=>{console.error(e);process.exitCode=1});
