const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const ws of ['normal','nowrap','pre-wrap'])for(const run of ['\u00a0'.repeat(6),'  ',' \u00a0 \u00a0'])for(const method of ['insertImage','insertHTML'])for(const offset of [1,run.length]){
 await p.setContent('<div id=h contenteditable style="white-space:'+ws+'"></div>');
 rows.push(await p.evaluate(({ws,run,method,offset})=>{const h=document.querySelector('#h');h.textContent='a'+run+'z';const original=h.firstChild,s=getSelection();h.focus();s.collapse(original,1+offset);const snap=()=>({html:h.innerHTML,caret:[s.anchorNode?.nodeName,s.anchorOffset],original:original.isConnected});const before=snap();document.execCommand(method,false,method==='insertHTML'?'<img src="x">':'x');const after=snap(),undo=document.execCommand('undo');return {ws,run,method,offset,before,after,undo,restored:snap()};},{ws,run,method,offset}));}
fs.writeFileSync(path.join(__dirname,'styles-'+label+'.json'),JSON.stringify({version,rows},null,2))
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
