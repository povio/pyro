const fs=require('fs'),path=require('path'),net=require('net'),cp=require('child_process'),os=require('os');
const cache=path.join(os.homedir(),'Library/Caches/lightningpanda'),pw=require(path.join(cache,'round6-reference-158/census/runner/node_modules/playwright-core'));
(async()=>{const [label,bin]=process.argv.slice(2),s=net.createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const port=s.address().port;await new Promise(r=>s.close(r));
const proc=cp.spawn(bin,['serve','--port',String(port),'--host','127.0.0.1','--log-level','error'],{stdio:'ignore',env:{...process.env,WPT_CHROME_STATE_DIR:fs.mkdtempSync(path.join(os.tmpdir(),'qa-enter-'))}});let b;
try{const endpoint='http://127.0.0.1:'+port;let version;for(let i=0;i<100;i++){try{version=await(await fetch(endpoint+'/json/version')).json();break}catch{}await new Promise(r=>setTimeout(r,100))}
b=await pw.chromium.connectOverCDP(endpoint);const p=await b.newPage(),rows=[];
for(const ws of ['normal','pre-wrap'])for(const side of ['left','right'])for(const cmd of ['delete','forwardDelete'])for(const off of [0,1,2]){
 await p.setContent('<div contenteditable id="h"><span style="white-space:'+ws+'">ab&nbsp; </span><i> &nbsp;cd</i></div>');
 rows.push(await p.evaluate(({ws,side,cmd,off})=>{const h=document.querySelector('#h'),l=h.firstChild.firstChild,r=h.lastChild.firstChild,s=getSelection();h.focus();s.collapse(side==='left'?l:r,side==='left'?2+off:off);const snap=()=>({html:h.innerHTML,anchor:[s.anchorNode===l?'left':s.anchorNode===r?'right':s.anchorNode?.nodeName,s.anchorOffset],connected:[l.isConnected,r.isConnected]});const before=snap();document.execCommand(cmd);const after=snap(),undo=document.execCommand('undo');return {ws,side,cmd,off,before,after,undo,restored:snap()};},{ws,side,cmd,off}));}
fs.writeFileSync(path.join(__dirname,'styles-'+label+'.json'),JSON.stringify({version,rows},null,2))
}finally{if(b)await b.close();proc.kill()}})().catch(e=>{console.error(e);process.exitCode=1});
