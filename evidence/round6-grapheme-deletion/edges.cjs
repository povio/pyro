const fs=require('fs'),cp=require('child_process'),path=require('path'),net=require('net');
const pw=require(process.env.PLAYWRIGHT_CORE || 'playwright-core');
const [label,bin]=process.argv.slice(2);
(async()=>{
 const srv=net.createServer();await new Promise(r=>srv.listen(0,'127.0.0.1',r));const port=srv.address().port;await new Promise(r=>srv.close(r));let proc,br;
 try {
 if(bin==='chrome')br=await pw.chromium.launch({executablePath:process.env.CHROME_BIN,headless:true});
 else {proc=cp.spawn(bin,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{stdio:'ignore'});for(let i=0;i<120;i++){try{if((await fetch('http://127.0.0.1:'+port+'/json/version')).ok)break}catch{}await new Promise(r=>setTimeout(r,100));}br=await pw.chromium.connectOverCDP('http://127.0.0.1:'+port);}
 const page=await br.newPage(),rows=[];
 for(const [name,parts] of Object.entries({single:['😀'],acute:['e\u0301'],flag:['🇸🇮'],splitAcute:['e','\u0301'],splitFlag:['🇸','🇮'],splitFamily:['👨‍','👩‍👧‍👦'],twoModifiers:['👋🏽\u0301'],accentEmoji:['❤️\u0301'],selectorText:['A\ufe0f'],mathSelector:['漢\u{E0100}'],multiAccent:['a\u0301\u0302'],nonemojiJoin:['x‍y'],crlf:['\r\n']})) for(const key of ['Backspace','Delete']) {
 const total=parts.join('').length; for(const offset of new Set([0,Math.floor(total/2),total])){
 await page.setContent('<div id=h contenteditable></div>');
 await page.evaluate(({parts,offset})=>{const h=document.getElementById('h');const nodes=[];for(const [i,str] of parts.entries()){const span=h.appendChild(document.createElement('span'));nodes.push(span.appendChild(document.createTextNode((i===0?'X':'')+str+(i===parts.length-1?'Y':''))));}
 let at=1+offset,n;for(const a of nodes){if(at<=a.length){n=a;break;}at-=a.length;}h.focus();getSelection().collapse(n,at);window.record={}; h.onbeforeinput=e=>{const s=e.getTargetRanges()[0];window.record={span:s&&[nodes.indexOf(s.startContainer),s.startOffset,nodes.indexOf(s.endContainer),s.endOffset],type:e.inputType};};}, {parts,offset});await page.keyboard.press(key);
 rows.push({name,key,offset,...await page.evaluate(()=>({value:h.textContent,...window.record}))});
 }}
 fs.writeFileSync(path.join(__dirname,label+'.json'),JSON.stringify(rows,null,1));
 }finally{await br?.close();proc?.kill();}
})().catch(e=>{console.error(e);process.exitCode=1;});
