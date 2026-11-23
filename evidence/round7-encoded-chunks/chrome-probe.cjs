const path=require('path'),fs=require('fs'),http=require('http');
const cache=path.resolve(__dirname,'../..'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{let b;const h=http.createServer((req,res)=>res.end('<body>'));try{
 await new Promise(r=>h.listen(0,'127.0.0.1',r));b=await pw.chromium.launch({executablePath:path.join(cache,'round6-reference-158/Google Chrome.app/Contents/MacOS/Google Chrome'),headless:true});
 const p=await b.newPage();await p.goto('http://127.0.0.1:'+h.address().port);
 const result=p.waitForEvent('console');await p.evaluate(fs.readFileSync(path.join(__dirname,'probe.js'),'utf8'));fs.writeFileSync(path.join(__dirname,'chrome-direct.json'),(await result).text()+'\n');
 console.log(b.version());
}finally{if(b)await b.close();h.close()}})().catch(e=>{console.error(e);process.exitCode=1})
