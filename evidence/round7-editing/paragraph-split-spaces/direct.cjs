const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const ws of ['normal','pre-wrap','pre-line'])for(const marker of ['|  abc',' | abc','  |abc','  |\u00a0b','\u00a0 |abc','  |','a  |b'])for(const value of ['X',' ']){
 await p.setContent('<div id=h contenteditable style="white-space:'+ws+'"><div></div></div>');
 rows.push(await p.evaluate(({ws,marker,value})=>{const h=document.querySelector('#h'),orig=h.firstChild;orig.textContent=marker.replace('|','');const t=orig.firstChild,s=getSelection();h.focus();s.collapse(t,marker.indexOf('|'));const point=n=>{let a=[];while(n&&n!==h){if(!n.parentNode)return null;a.unshift([...n.parentNode.childNodes].indexOf(n));n=n.parentNode}return a};const snap=()=>({html:h.innerHTML,anchor:[point(s.anchorNode),s.anchorOffset],focus:[point(s.focusNode),s.focusOffset],connected:[orig.isConnected,t.isConnected]});const before=snap(),ok=document.execCommand('insertText',false,value),typed=snap(),undo=document.execCommand('undo');return {ws,marker,value,before,ok,typed,undo,restored:snap()};},{ws,marker,value}));}
fs.writeFileSync(path.join(__dirname,'direct-'+label+'.json'),JSON.stringify({version,rows},null,2))
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
