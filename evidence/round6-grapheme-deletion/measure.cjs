const fs=require('fs'),cp=require('child_process'),path=require('path'),net=require('net');
const pw=require(process.env.PLAYWRIGHT_CORE || 'playwright-core');
const [label,bin]=process.argv.slice(2);
(async()=>{
 const srv=net.createServer();await new Promise(r=>srv.listen(0,'127.0.0.1',r));const port=srv.address().port;await new Promise(r=>srv.close(r));let proc,br;
 try {
 if(bin==='chrome')br=await pw.chromium.launch({executablePath:process.env.CHROME_BIN,headless:true});
 else {proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{stdio:'ignore'});for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+port+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100));}br=await pw.chromium.connectOverCDP('http://127.0.0.1:'+port);}
 const page=await br.newPage(),rows=[];
 for(const [name,cluster] of Object.entries({ascii:'a',acute:'e\u0301',flag:'🇸🇮',modifier:'👋🏽',family:'👨‍👩‍👧‍👦',variation:'❤️',keycap:'1️⃣',indic:'क्षि',thai:'ที่',jamo:'각'}))for(const key of ['Backspace','Delete']){
 await page.setContent('<div id=h contenteditable></div>');await page.evaluate(({cluster,key})=>{const h=document.getElementById('h');h.textContent='X'+cluster+'Y';h.focus();getSelection().collapse(h.firstChild,key==='Delete'?1:1+cluster.length);window.record={};h.onbeforeinput=e=>{const r=e.getTargetRanges()[0];window.record={span:r&&[r.startOffset,r.endOffset],target:e.inputType, trusted:e.isTrusted};};}, {cluster,key});await page.keyboard.press(key);
 rows.push({name,cluster,key,...await page.evaluate(()=>({value:h.textContent,caret:getSelection().focusOffset,...window.record}))});
 }
 fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify(rows,null,1));
 }finally{await br?.close();proc?.kill();}
})().catch(e=>{console.error(e);process.exitCode=1;});
