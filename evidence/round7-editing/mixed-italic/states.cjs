const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const reverse of [false,true])for(const example of ['foo<i>ba[r</i>b]az','foo bar <i>baz [qoz</i> quz] sic']){
 await p.setContent('<div contenteditable id=h>'+example.replace(/[\[\]]/g,'')+'</div>');rows.push(await p.evaluate(({reverse,example})=>{const h=document.querySelector('#h'),i=h.querySelector('i'),t=i.firstChild,r=i.nextSibling,s=getSelection(),first=example.startsWith('foo<i>')?2:4,last=example.startsWith('foo<i>')?1:4;h.focus();s.setBaseAndExtent(...(reverse?[r,last,t,first]:[t,first,r,last]));const snap=()=>({html:h.innerHTML,state:document.queryCommandState('italic'),text:s.toString(),direction:s.direction});const before=snap();document.execCommand('italic');const after=snap();return {example,reverse,before,after};},{reverse,example}));}
fs.writeFileSync(path.join(__dirname,'states-'+label+'.json'),JSON.stringify({version,rows},null,2));
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
